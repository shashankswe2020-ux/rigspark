use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Debug, thiserror::Error)]
#[error("workspace: {0}")]
pub struct WorkspaceError(pub String);
impl From<std::io::Error> for WorkspaceError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}
impl From<serde_json::Error> for WorkspaceError {
    fn from(error: serde_json::Error) -> Self {
        Self(error.to_string())
    }
}
impl From<crate::memory::MemoryError> for WorkspaceError {
    fn from(error: crate::memory::MemoryError) -> Self {
        Self(error.to_string())
    }
}
pub fn relative_path(raw: &str) -> Result<String, WorkspaceError> {
    if raw.is_empty() || raw == "." {
        return Ok(String::new());
    }
    if raw.len() > 1024
        || raw.split('/').count() > 40
        || raw.starts_with('/')
        || raw.contains(['\\', '\0', ':'])
        || raw.split('/').any(|part| {
            // A trailing dot also refuses "." and "..".
            part.is_empty() || part.ends_with('.') || part.trim() != part
        })
    {
        return Err(WorkspaceError("invalid relative path".into()));
    }
    Ok(raw.into())
}
pub fn denied(raw: &str) -> bool {
    let lower = raw.to_lowercase();
    let segments: Vec<_> = lower.split('/').collect();
    let name = segments.last().copied().unwrap_or("");
    segments.iter().any(|part| {
        [
            ".git", ".ssh", ".aws", ".gnupg", ".kube", ".docker", ".config",
        ]
        .contains(part)
    }) || [
        ".env",
        ".npmrc",
        ".pypirc",
        ".netrc",
        ".git-credentials",
        ".dockercfg",
        "credentials",
    ]
    .contains(&name)
        || [".env.", "id_rsa", "id_ed25519", "id_ecdsa", "id_dsa"]
            .iter()
            .any(|prefix| name.starts_with(prefix))
        || [".pem", ".key", ".p12", ".pfx", ".asc"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}
fn ignored(name: &str) -> bool {
    [
        "node_modules",
        ".git",
        "dist",
        "build",
        "out",
        "coverage",
        ".next",
        ".nuxt",
        ".cache",
        ".venv",
        "venv",
        "__pycache__",
        ".staging",
        ".turbo",
    ]
    .contains(&name)
}
fn binary(path: &str) -> bool {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_lowercase();
    [
        "png", "jpg", "jpeg", "gif", "webp", "ico", "bmp", "pdf", "zip", "gz", "tar", "tgz", "bz2",
        "7z", "rar", "exe", "dll", "so", "dylib", "bin", "o", "a", "class", "jar", "wasm", "woff",
        "woff2", "ttf", "otf", "eot", "mp3", "mp4", "mov", "avi", "mkv", "wav", "flac", "sqlite",
        "db",
    ]
    .contains(&extension.as_str())
}
fn hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}
struct Root {
    path: PathBuf,
    identity: same_file::Handle,
    directory: crate::secure_fs::Directory,
}
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceRoot {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LineRange {
    pub start_line: usize,
    pub end_line: usize,
}
#[derive(Debug, Clone, Serialize)]
pub struct FileSnapshot {
    pub path: String,
    pub content: String,
    pub hash: String,
    pub size: usize,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<LineRange>,
}
#[derive(Debug, Clone, Serialize)]
pub struct TreeEntry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub results: Vec<SearchResult>,
    pub next_cursor: Option<String>,
    pub scan_truncated: bool,
}
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub name: String,
    pub path: String,
    pub size: u64,
}
pub struct WorkspaceService {
    roots: BTreeMap<String, Root>,
}
impl Default for WorkspaceService {
    fn default() -> Self {
        Self::new()
    }
}
impl WorkspaceService {
    pub fn new() -> Self {
        Self {
            roots: BTreeMap::new(),
        }
    }
    pub fn register(&mut self, path: &Path) -> Result<WorkspaceRoot, WorkspaceError> {
        if self.roots.len() >= 8 {
            return Err(WorkspaceError("root limit reached".into()));
        }
        let path = path.canonicalize()?;
        if !path.is_dir() {
            return Err(WorkspaceError("root is not a directory".into()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        let identity = same_file::Handle::from_path(&path)?;
        let directory = crate::secure_fs::Directory::open(&path)?;
        self.roots.insert(
            id.clone(),
            Root {
                path,
                identity,
                directory,
            },
        );
        Ok(WorkspaceRoot { id, name })
    }
    pub fn revoke(&mut self, id: &str) {
        self.roots.remove(id);
    }
    fn root(&self, id: &str) -> Result<&Root, WorkspaceError> {
        let root = self
            .roots
            .get(id)
            .ok_or_else(|| WorkspaceError("unknown or revoked root".into()))?;
        if fs::symlink_metadata(&root.path)?.file_type().is_symlink()
            || same_file::Handle::from_path(&root.path)? != root.identity
        {
            return Err(WorkspaceError("workspace root changed".into()));
        }
        Ok(root)
    }
    fn resolve(
        &self,
        id: &str,
        path: &str,
        allow_missing: bool,
    ) -> Result<PathBuf, WorkspaceError> {
        let relative = relative_path(path)?;
        if denied(&relative) {
            return Err(WorkspaceError("path is not accessible".into()));
        }
        let root = self.root(id)?;
        let mut current = root.path.clone();
        let parts: Vec<_> = relative
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        for (index, part) in parts.iter().enumerate() {
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink()
                        || index + 1 < parts.len() && !metadata.is_dir()
                    {
                        return Err(WorkspaceError("symlink or non-directory ancestor".into()));
                    }
                }
                Err(error)
                    if allow_missing
                        && index + 1 == parts.len()
                        && error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if !current.starts_with(&root.path) {
            return Err(WorkspaceError("path escaped root".into()));
        }
        Ok(current)
    }
    fn text(&self, id: &str, path: &str, maximum: u64) -> Result<String, WorkspaceError> {
        self.resolve(id, path, false)?;
        let bytes = self
            .root(id)?
            .directory
            .read(Path::new(path), maximum, false)?;
        if bytes.len() as u64 > maximum || bytes.contains(&0) {
            return Err(WorkspaceError("binary or oversized file".into()));
        }
        String::from_utf8(bytes).map_err(|_| WorkspaceError("file is not UTF-8".into()))
    }
    fn replace_text(
        &self,
        id: &str,
        path: &str,
        text: &str,
        create: bool,
    ) -> Result<(), WorkspaceError> {
        self.resolve(id, path, create)?;
        self.root(id)?
            .directory
            .write(Path::new(path), text.as_bytes(), create, !create)?;
        Ok(())
    }
    fn remove_text(&self, id: &str, path: &str) -> Result<(), WorkspaceError> {
        self.resolve(id, path, false)?;
        self.root(id)?.directory.remove(Path::new(path))?;
        Ok(())
    }
    pub fn read(
        &self,
        id: &str,
        path: &str,
        mut range: Option<LineRange>,
    ) -> Result<FileSnapshot, WorkspaceError> {
        let relative = relative_path(path)?;
        if relative.is_empty() || binary(&relative) {
            return Err(WorkspaceError("text file required".into()));
        }
        let full = self.text(id, &relative, 256 * 1024)?;
        let content = if let Some(range) = &mut range {
            if range.start_line == 0 || range.end_line < range.start_line {
                return Err(WorkspaceError("invalid line range".into()));
            }
            range.end_line = range.end_line.min(full.split('\n').count());
            full.split('\n')
                .skip(range.start_line - 1)
                .take(range.end_line.saturating_sub(range.start_line) + 1)
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            full
        };
        let size = content.len();
        let hash = hash(&content);
        Ok(FileSnapshot {
            path: relative,
            content,
            hash,
            size,
            truncated: false,
            range,
        })
    }
    pub fn create_root(&mut self, path: &Path) -> Result<WorkspaceRoot, WorkspaceError> {
        crate::memory::owned_directory(path)?;
        self.register(path)
    }
    pub fn git_context(&self, id: &str, mode: &str) -> Result<serde_json::Value, WorkspaceError> {
        self.git_context_with(id, mode, &crate::workspace_git::NativeGitRunner)
    }
    pub fn git_context_with(
        &self,
        id: &str,
        mode: &str,
        runner: &dyn crate::workspace_git::GitRunner,
    ) -> Result<serde_json::Value, WorkspaceError> {
        if !["status", "diff"].contains(&mode) {
            return Err(WorkspaceError("invalid git context mode".into()));
        }
        let root = &self.root(id)?.path;
        let label = format!("git {mode}");
        let output = match runner.output(root, mode) {
            Ok(output) => output,
            Err(_) => {
                return Ok(
                    serde_json::json!({"kind":"git","mode":mode,"label":label,"available":false,"content":"","hash":"","size":0,"truncated":false,"reason":"git-failed"}),
                );
            }
        };
        self.root(id)?;
        let success = output.success;
        let bytes = output.bytes;
        let raw = String::from_utf8_lossy(&bytes);
        let truncated = raw.chars().count() > 102400;
        let content = raw.chars().take(102400).collect::<String>();
        let available = success && !content.trim().is_empty();
        Ok(
            serde_json::json!({"kind":"git","mode":mode,"label":label,"available":available,"content":if available{content.as_str()}else{""},"hash":if available{hash(&content)}else{String::new()},"size":if available{content.len()}else{0},"truncated":truncated,"reason":if !success{Some("git-failed")}else if !available{Some("no-changes")}else{None}}),
        )
    }
    pub fn tree(&self, id: &str, path: &str) -> Result<Vec<TreeEntry>, WorkspaceError> {
        let relative = relative_path(path)?;
        self.resolve(id, &relative, false)?;
        let root = self.root(id)?;
        let directory = if relative.is_empty() {
            root.directory.dir()?.try_clone()?
        } else {
            root.directory.subdir(Path::new(&relative))?
        };
        let mut entries = Vec::new();
        for (count, entry) in directory.entries()?.enumerate() {
            if count >= 2000 {
                break;
            }
            let entry = entry?;
            let kind = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            if kind.is_symlink() || denied(&path) || kind.is_dir() && ignored(&name) {
                continue;
            }
            if kind.is_dir() || kind.is_file() {
                entries.push(TreeEntry {
                    name,
                    path,
                    kind: if kind.is_dir() { "dir" } else { "file" }.into(),
                    size: if kind.is_file() {
                        Some(entry.metadata()?.len())
                    } else {
                        None
                    },
                });
            }
        }
        entries.sort_by(|left, right| left.kind.cmp(&right.kind).then(left.name.cmp(&right.name)));
        Ok(entries)
    }
    pub fn search(
        &self,
        id: &str,
        query: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<SearchPage, WorkspaceError> {
        self.root(id)?;
        let limit = limit.clamp(1, 100);
        let offset = cursor
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| WorkspaceError("invalid cursor".into()))?;
        let needle = crate::sessions::cap(&query.trim().to_lowercase(), 256);
        let mut pending = vec![String::new()];
        let mut matches = Vec::new();
        let mut scanned = 0;
        let mut truncated = false;
        while let Some(path) = pending.pop() {
            for entry in self.tree(id, &path)? {
                scanned += 1;
                if scanned >= 20000 || matches.len() >= 500 {
                    truncated = true;
                    break;
                }
                if entry.kind == "dir" {
                    pending.push(entry.path);
                } else if !binary(&entry.path) && entry.path.to_lowercase().contains(&needle) {
                    matches.push(entry);
                }
            }
            if truncated {
                break;
            }
        }
        matches.sort_by(|left, right| left.path.cmp(&right.path));
        let next_cursor = (offset.saturating_add(limit) < matches.len())
            .then(|| offset.saturating_add(limit).to_string());
        Ok(SearchPage {
            results: matches
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|entry| SearchResult {
                    name: entry.name,
                    path: entry.path,
                    size: entry.size.unwrap_or(0),
                })
                .collect(),
            next_cursor,
            scan_truncated: truncated,
        })
    }
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Hunk {
    pub start: usize,
    pub end: usize,
    pub lines: Vec<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
pub enum EditOperation {
    Create {
        path: String,
        text: String,
    },
    Update {
        path: String,
        #[serde(rename = "baseHash")]
        base_hash: String,
        hunks: Vec<Hunk>,
    },
    Delete {
        path: String,
        #[serde(rename = "baseHash")]
        base_hash: String,
    },
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditProposal {
    pub workspace_id: String,
    pub operations: Vec<EditOperation>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlannedFile {
    path: String,
    op: String,
    before_hash: String,
    result_hash: String,
    original: Option<String>,
    new_text: Option<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditReview {
    pub proposal_id: String,
    #[serde(skip)]
    workspace_id: String,
    #[serde(skip)]
    files: Vec<PlannedFile>,
    #[serde(rename = "files")]
    pub reviewed_files: Vec<ReviewFile>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFile {
    pub path: String,
    pub op: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_hash: Option<String>,
    pub result_hash: String,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<DiffHunk>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Serialize)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}
#[derive(Debug, Serialize)]
pub struct DiffLine {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
}
fn diff_review(file: &PlannedFile, operation: &EditOperation) -> ReviewFile {
    let line = |kind: &str, text: &str| DiffLine {
        kind: kind.into(),
        text: text.into(),
    };
    let mut added = 0;
    let mut removed = 0;
    let hunks = match operation {
        EditOperation::Create { text, .. } => {
            let lines: Vec<_> = text.split('\n').map(|text| line("add", text)).collect();
            added = lines.len();
            vec![DiffHunk {
                header: format!("@@ +1,{added} @@"),
                lines,
            }]
        }
        EditOperation::Delete { .. } => {
            let lines: Vec<_> = file
                .original
                .as_deref()
                .unwrap_or("")
                .split('\n')
                .map(|text| line("del", text))
                .collect();
            removed = lines.len();
            vec![DiffHunk {
                header: format!("@@ -1,{removed} @@"),
                lines,
            }]
        }
        EditOperation::Update { hunks, .. } => {
            let base: Vec<_> = file.original.as_deref().unwrap_or("").split('\n').collect();
            let mut ordered = hunks.clone();
            ordered.sort_by_key(|hunk| hunk.start);
            ordered
                .iter()
                .map(|hunk| {
                    let start = hunk.start - 1;
                    let deleted = hunk.end - start;
                    added += hunk.lines.len();
                    removed += deleted;
                    let mut lines: Vec<_> = base[start.saturating_sub(3)..start]
                        .iter()
                        .map(|text| line("context", text))
                        .collect();
                    lines.extend(base[start..hunk.end].iter().map(|text| line("del", text)));
                    lines.extend(hunk.lines.iter().map(|text| line("add", text)));
                    lines.extend(
                        base[hunk.end..(hunk.end + 3).min(base.len())]
                            .iter()
                            .map(|text| line("context", text)),
                    );
                    DiffHunk {
                        header: format!(
                            "@@ -{},{deleted} +{},{} @@",
                            hunk.start,
                            hunk.start,
                            hunk.lines.len()
                        ),
                        lines,
                    }
                })
                .collect()
        }
    };
    ReviewFile {
        path: file.path.clone(),
        op: file.op.clone(),
        base_hash: (file.op != "create").then(|| file.before_hash.clone()),
        result_hash: file.result_hash.clone(),
        added,
        removed,
        hunks,
        warnings: if file.op == "update" && added + removed > 2000 {
            vec!["large change; review carefully".into()]
        } else {
            Vec::new()
        },
    }
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordFile {
    path: String,
    op: String,
    before_hash: String,
    after_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    original: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyRecord {
    schema_version: u8,
    application_id: String,
    workspace_id: String,
    created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    root_identity: Option<String>,
    files: Vec<RecordFile>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyResult {
    pub application_id: String,
    pub files: Vec<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevertResult {
    pub application_id: String,
    pub reverted: Vec<String>,
    pub skipped: Vec<String>,
}
impl WorkspaceService {
    fn root_identity(&self, id: &str) -> Result<String, WorkspaceError> {
        let root = self.root(id)?;
        let metadata = fs::metadata(&root.path)?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            format!(
                "{}:{}:{}",
                root.path.display(),
                metadata.dev(),
                metadata.ino()
            )
        };
        #[cfg(not(unix))]
        let identity = format!("{}:{:?}", root.path.display(), metadata.created()?);
        Ok(hash(&identity))
    }
    pub fn recover(
        &self,
        workspace_id: &str,
        records: &Path,
    ) -> Result<Vec<RevertResult>, WorkspaceError> {
        let identity = self.root_identity(workspace_id)?;
        let guard = self.edit_lock(workspace_id, records)?;
        let mut results = Vec::new();
        for (index, entry) in fs::read_dir(records)?.enumerate() {
            if index >= 10000 {
                return Err(WorkspaceError("edit journal count exceeds limit".into()));
            }
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_suffix(".pending.json") else {
                continue;
            };
            if uuid::Uuid::parse_str(id).is_err() {
                return Err(WorkspaceError("invalid pending journal name".into()));
            }
            let raw = crate::state::secure_read(&entry.path(), 16 * 1024 * 1024, true)
                .map_err(|error| WorkspaceError(error.to_string()))?;
            let record: ApplyRecord = serde_json::from_str(&raw)?;
            if record.root_identity.as_deref() != Some(&identity) {
                continue;
            }
            if record.schema_version != 1
                || record.application_id != id
                || record.files.is_empty()
                || record.files.len() > 20
            {
                return Err(WorkspaceError("invalid recovery journal".into()));
            }
            for file in &record.files {
                if !["create", "update"].contains(&file.op.as_str())
                    || file.op == "update"
                        && file.original.as_ref().is_none_or(|text| {
                            text.len() > 512 * 1024 || hash(text) != file.before_hash
                        })
                {
                    return Err(WorkspaceError("invalid recovery bytes".into()));
                }
            }
            let mut reverted = Vec::new();
            let mut skipped = Vec::new();
            for file in record.files.iter().rev() {
                guard
                    .assert_owned()
                    .map_err(|error| WorkspaceError(error.to_string()))?;
                let path = self.resolve(workspace_id, &file.path, true)?;
                if file.op == "create" && !path.try_exists()? {
                    continue;
                }
                let current = self.text(workspace_id, &file.path, 512 * 1024);
                if current
                    .as_ref()
                    .is_ok_and(|text| hash(text) == file.before_hash)
                {
                    continue;
                }
                if !current.is_ok_and(|text| hash(&text) == file.after_hash) {
                    skipped.push(file.path.clone());
                    continue;
                }
                if file.op == "create" {
                    self.remove_text(workspace_id, &file.path)?;
                } else {
                    self.replace_text(
                        workspace_id,
                        &file.path,
                        file.original
                            .as_deref()
                            .ok_or_else(|| WorkspaceError("missing original bytes".into()))?,
                        false,
                    )?;
                }
                reverted.push(file.path.clone());
            }
            if skipped.is_empty() {
                fs::rename(entry.path(), records.join(format!("{id}.recovered.json")))?;
            }
            results.push(RevertResult {
                application_id: id.into(),
                reverted,
                skipped,
            });
        }
        guard
            .release()
            .map_err(|error| WorkspaceError(error.to_string()))?;
        Ok(results)
    }
    pub fn review(&self, proposal: &EditProposal) -> Result<EditReview, WorkspaceError> {
        self.root(&proposal.workspace_id)?;
        if proposal.operations.is_empty() || proposal.operations.len() > 20 {
            return Err(WorkspaceError("invalid edit operation count".into()));
        }
        let mut seen = HashSet::new();
        let mut files = Vec::new();
        let mut reviewed_files = Vec::new();
        for operation in &proposal.operations {
            let path = match operation {
                EditOperation::Create { path, .. }
                | EditOperation::Update { path, .. }
                | EditOperation::Delete { path, .. } => relative_path(path)?,
            };
            if path.is_empty()
                || denied(&path)
                || binary(&path)
                || !seen.insert(path.to_lowercase())
            {
                return Err(WorkspaceError("unsafe or duplicate edit path".into()));
            }
            let (op, original, new_text) = match operation {
                EditOperation::Create { text, .. } => {
                    let target = self.resolve(&proposal.workspace_id, &path, true)?;
                    if fs::symlink_metadata(target).is_ok() {
                        return Err(WorkspaceError("create target exists".into()));
                    }
                    ("create", None, Some(text.clone()))
                }
                EditOperation::Delete { base_hash, .. } => {
                    let snapshot = self.read(&proposal.workspace_id, &path, None)?;
                    if snapshot.hash != *base_hash {
                        return Err(WorkspaceError("stale edit base".into()));
                    }
                    ("delete", Some(snapshot.content), None)
                }
                EditOperation::Update {
                    base_hash, hunks, ..
                } => {
                    let snapshot = self.read(&proposal.workspace_id, &path, None)?;
                    if snapshot.hash != *base_hash {
                        return Err(WorkspaceError("stale edit base".into()));
                    }
                    if hunks.is_empty() || hunks.len() > 100 {
                        return Err(WorkspaceError("invalid hunk count".into()));
                    }
                    let base: Vec<_> = snapshot.content.split('\n').collect();
                    let mut cursor = 0;
                    let mut output = Vec::new();
                    let mut ordered = hunks.clone();
                    ordered.sort_by_key(|hunk| hunk.start);
                    for hunk in ordered {
                        if hunk.start == 0
                            || hunk.start - 1 < cursor
                            || hunk.start > base.len() + 1
                            || hunk.end < hunk.start - 1
                            || hunk.end > base.len()
                            || hunk.lines.len() > 2000
                            || hunk
                                .lines
                                .iter()
                                .any(|line| line.contains(['\n', '\r', '\0']))
                        {
                            return Err(WorkspaceError("overlapping or invalid hunk".into()));
                        }
                        output.extend(
                            base[cursor..hunk.start - 1]
                                .iter()
                                .map(|line| (*line).to_owned()),
                        );
                        output.extend(hunk.lines);
                        cursor = hunk.end;
                    }
                    output.extend(base[cursor..].iter().map(|line| (*line).to_owned()));
                    ("update", Some(snapshot.content), Some(output.join("\n")))
                }
            };
            if new_text
                .as_ref()
                .is_some_and(|text| text.len() > 512 * 1024 || text.contains('\0'))
            {
                return Err(WorkspaceError("binary or oversized edit content".into()));
            }
            let file = PlannedFile {
                path,
                op: op.into(),
                before_hash: original.as_ref().map(|text| hash(text)).unwrap_or_default(),
                result_hash: new_text.as_ref().map(|text| hash(text)).unwrap_or_default(),
                original,
                new_text,
            };
            reviewed_files.push(diff_review(&file, operation));
            files.push(file);
        }
        let warnings = if reviewed_files.iter().any(|file| !file.warnings.is_empty()) {
            vec!["some files reported warnings; review carefully".into()]
        } else {
            Vec::new()
        };
        Ok(EditReview {
            proposal_id: uuid::Uuid::new_v4().to_string(),
            workspace_id: proposal.workspace_id.clone(),
            files,
            reviewed_files,
            warnings,
        })
    }
    fn edit_lock(
        &self,
        id: &str,
        records: &Path,
    ) -> Result<crate::state::LockGuard, WorkspaceError> {
        let root = self.root(id)?;
        crate::memory::owned_directory(records)?;
        let mut config = crate::state::Config::from_home(records)
            .map_err(|error| WorkspaceError(error.to_string()))?;
        config.lock = records.join(format!(
            "workspace-{}.lock",
            hash(&root.path.to_string_lossy())
        ));
        crate::state::StateStore::new(config)
            .lock(Duration::from_secs(10))
            .map_err(|error| WorkspaceError(error.to_string()))
    }
    pub fn apply(
        &self,
        review: EditReview,
        records: &Path,
        timestamp: &str,
    ) -> Result<ApplyResult, WorkspaceError> {
        self.apply_with(review, records, timestamp, |_| Ok(()))
    }
    pub fn apply_with(
        &self,
        review: EditReview,
        records: &Path,
        timestamp: &str,
        before: impl Fn(usize) -> Result<(), WorkspaceError>,
    ) -> Result<ApplyResult, WorkspaceError> {
        if review.files.iter().any(|file| file.op == "delete") {
            return Err(WorkspaceError("delete apply is not enabled".into()));
        }
        let guard = self.edit_lock(&review.workspace_id, records)?;
        for file in &review.files {
            let path = self.resolve(&review.workspace_id, &file.path, file.op == "create")?;
            if file.op == "create" {
                if fs::symlink_metadata(&path).is_ok() {
                    return Err(WorkspaceError("create target appeared".into()));
                }
            } else if hash(&self.text(&review.workspace_id, &file.path, 512 * 1024)?)
                != file.before_hash
            {
                return Err(WorkspaceError("stale edit approval".into()));
            }
        }
        let id = uuid::Uuid::new_v4().to_string();
        let record = ApplyRecord {
            schema_version: 1,
            application_id: id.clone(),
            workspace_id: review.workspace_id.clone(),
            created_at: timestamp.into(),
            root_identity: Some(self.root_identity(&review.workspace_id)?),
            files: review
                .files
                .iter()
                .map(|file| RecordFile {
                    path: file.path.clone(),
                    op: file.op.clone(),
                    before_hash: file.before_hash.clone(),
                    after_hash: file.result_hash.clone(),
                    original: file.original.clone(),
                })
                .collect(),
        };
        let pending = records.join(format!("{id}.pending.json"));
        crate::memory::atomic_file(&pending, serde_json::to_vec_pretty(&record)?.as_slice())?;
        let mut replaced = 0;
        let result = (|| {
            for (index, file) in review.files.iter().enumerate() {
                before(index)?;
                guard
                    .assert_owned()
                    .map_err(|error| WorkspaceError(error.to_string()))?;
                let path = self.resolve(&review.workspace_id, &file.path, file.op == "create")?;
                if file.op == "create" {
                    if fs::symlink_metadata(&path).is_ok() {
                        return Err(WorkspaceError("create target appeared".into()));
                    }
                } else {
                    if hash(&self.text(&review.workspace_id, &file.path, 512 * 1024)?)
                        != file.before_hash
                    {
                        return Err(WorkspaceError(
                            "edit target changed before publication".into(),
                        ));
                    }
                }
                self.replace_text(
                    &review.workspace_id,
                    &file.path,
                    file.new_text
                        .as_deref()
                        .ok_or_else(|| WorkspaceError("missing edit bytes".into()))?,
                    file.op == "create",
                )?;
                replaced += 1;
            }
            fs::rename(&pending, records.join(format!("{id}.json")))?;
            Ok::<_, WorkspaceError>(())
        })();
        if let Err(error) = result {
            for file in review.files.iter().take(replaced).rev() {
                self.resolve(&review.workspace_id, &file.path, false)?;
                if hash(&self.text(&review.workspace_id, &file.path, 512 * 1024)?)
                    != file.result_hash
                {
                    return Err(WorkspaceError(
                        "rollback refused changed file; recovery record retained".into(),
                    ));
                }
                if let Some(original) = &file.original {
                    self.replace_text(&review.workspace_id, &file.path, original, false)?;
                } else {
                    self.remove_text(&review.workspace_id, &file.path)?;
                }
            }
            fs::remove_file(pending)?;
            return Err(error);
        }
        guard
            .release()
            .map_err(|error| WorkspaceError(error.to_string()))?;
        Ok(ApplyResult {
            application_id: id,
            files: review.files.into_iter().map(|file| file.path).collect(),
        })
    }
    pub fn revert(&self, id: &str, records: &Path) -> Result<RevertResult, WorkspaceError> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(WorkspaceError("invalid application id".into()));
        }
        let raw =
            crate::state::secure_read(&records.join(format!("{id}.json")), 16 * 1024 * 1024, true)
                .map_err(|error| WorkspaceError(error.to_string()))?;
        let record: ApplyRecord = serde_json::from_str(&raw)?;
        if record.schema_version != 1 || record.application_id != id || record.files.len() > 20 {
            return Err(WorkspaceError("invalid apply record".into()));
        }
        let guard = self.edit_lock(&record.workspace_id, records)?;
        let mut reverted = Vec::new();
        let mut skipped = Vec::new();
        for file in record.files.iter().rev() {
            let current = self.text(&record.workspace_id, &file.path, 512 * 1024);
            if !current.is_ok_and(|text| hash(&text) == file.after_hash) {
                skipped.push(file.path.clone());
                continue;
            }
            self.resolve(&record.workspace_id, &file.path, false)?;
            if file.op == "create" {
                self.remove_text(&record.workspace_id, &file.path)?;
            } else if file.op == "update" {
                let original = file
                    .original
                    .as_ref()
                    .ok_or_else(|| WorkspaceError("missing rollback content".into()))?;
                if hash(original) != file.before_hash {
                    return Err(WorkspaceError("rollback content mismatch".into()));
                }
                self.replace_text(&record.workspace_id, &file.path, original, false)?;
            } else {
                return Err(WorkspaceError("invalid rollback operation".into()));
            }
            reverted.push(file.path.clone());
        }
        guard
            .release()
            .map_err(|error| WorkspaceError(error.to_string()))?;
        Ok(RevertResult {
            application_id: id.into(),
            reverted,
            skipped,
        })
    }
}
