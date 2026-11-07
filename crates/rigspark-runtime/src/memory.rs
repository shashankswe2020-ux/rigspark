#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn interrupted_move_restores_source_and_target_from_either_entry_point() {
        for open_source in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let source = MemoryStore::open(root.path(), "source", "original-source").unwrap();
            let target = MemoryStore::open(root.path(), "target", "original-target").unwrap();
            let expected = target.files().unwrap();
            let mut next = expected.clone();
            next.insert("system.md".into(), "migrated persona".into());
            let guard = source.lock().unwrap();
            target.publish_pending(&guard, &expected, &next).unwrap();
            target.prepare_move(&source, &guard).unwrap();
            guard.release().unwrap();
            if open_source {
                MemoryStore::existing(root.path(), "source").unwrap();
            } else {
                MemoryStore::existing(root.path(), "target").unwrap();
            }
            assert_eq!(
                MemoryStore::existing(root.path(), "source")
                    .unwrap()
                    .meta
                    .created_at,
                "original-source"
            );
            assert_eq!(
                MemoryStore::existing(root.path(), "target")
                    .unwrap()
                    .files()
                    .unwrap(),
                expected
            );
        }
    }
}
use crate::state::{Config, LockGuard, StateStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, thiserror::Error)]
#[error("memory: {0}")]
pub struct MemoryError(pub String);
impl From<std::io::Error> for MemoryError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}
impl From<serde_json::Error> for MemoryError {
    fn from(error: serde_json::Error) -> Self {
        Self(error.to_string())
    }
}
impl From<crate::state::StateError> for MemoryError {
    fn from(error: crate::state::StateError) -> Self {
        Self(error.to_string())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EmbeddingMeta {
    pub model: String,
    pub dimension: usize,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemoryMeta {
    pub schema_version: u8,
    pub model_id: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<EmbeddingMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_unsupported: Option<bool>,
}
impl MemoryMeta {
    pub fn validate(&self) -> Result<(), MemoryError> {
        if self.schema_version != 1
            || self.model_id.is_empty()
            || self.model_id.len() > 8192
            || self.created_at.is_empty()
            || self.created_at.len() > 1024
            || self.embedding_unsupported == Some(false)
            || self.embedding.as_ref().is_some_and(|embedding| {
                embedding.model.is_empty()
                    || embedding.model.len() > 8192
                    || embedding.dimension == 0
                    || embedding.dimension > 100000
            })
        {
            return Err(MemoryError("invalid metadata".into()));
        }
        Ok(())
    }
}
pub fn memory_slug(id: &str, windows: bool) -> Result<String, MemoryError> {
    if id.len() > 1024 * 1024 {
        return Err(MemoryError("model id exceeds limit".into()));
    }
    let mut slug = String::new();
    for character in id.trim().to_lowercase().chars() {
        let character = if character.is_ascii_alphanumeric() || "._-".contains(character) {
            character
        } else {
            '-'
        };
        if character != '-' || !slug.ends_with('-') {
            slug.push(character);
        }
    }
    slug = slug.trim_matches(['-', '.']).into();
    let stem = slug.split('.').next().unwrap_or("");
    if windows
        && (["con", "prn", "aux", "nul"].contains(&stem)
            || (stem.starts_with("com") || stem.starts_with("lpt"))
                && stem.len() == 4
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        slug = format!("x-{slug}");
    }
    if slug.is_empty() {
        return Err(MemoryError("model id has no filesystem-safe slug".into()));
    }
    if slug.len() > 128 {
        let digest = format!("{:x}", Sha256::digest(slug.as_bytes()));
        slug = format!(
            "{}-{}",
            slug[..111].trim_end_matches(['-', '.']),
            &digest[..16]
        );
    }
    Ok(slug)
}
pub(crate) fn private_directory(path: &Path) -> Result<(), MemoryError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
        Ok(_) => {
            return Err(MemoryError(
                "directory is symlinked or not a directory".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                private_directory(parent)?;
            }
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let mut builder = builder;
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    return private_directory(path);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(error) => return Err(error.into()),
    }
    Ok(())
}
pub(crate) fn owned_directory(path: &Path) -> Result<(), MemoryError> {
    private_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        crate::secure_fs::Directory::open(path)?
            .dir()?
            .open(".")?
            .into_std()
            .set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub(crate) fn atomic_file(path: &Path, bytes: &[u8]) -> Result<(), MemoryError> {
    let parent = path
        .parent()
        .ok_or_else(|| MemoryError("missing file parent".into()))?;
    private_directory(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;
    staged
        .persist(path)
        .map_err(|error| MemoryError(error.to_string()))?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
#[derive(Debug, Clone)]
pub struct MemoryStore {
    pub dir: PathBuf,
    pub meta: MemoryMeta,
    home: PathBuf,
    home_directory: std::sync::Arc<crate::secure_fs::Directory>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveJournal {
    source: String,
    recovery: String,
}
impl MemoryStore {
    pub fn open(home: &Path, id: &str, created_at: &str) -> Result<Self, MemoryError> {
        let home = std::path::absolute(home)?;
        let root = home.join("memory");
        owned_directory(&home)?;
        let home_directory = std::sync::Arc::new(crate::secure_fs::Directory::open(&home)?);
        home_directory.ensure_dir(Path::new("memory"))?;
        let dir = root.join(memory_slug(id, cfg!(windows))?);
        let store = Self {
            dir,
            home,
            home_directory,
            meta: MemoryMeta {
                schema_version: 1,
                model_id: id.into(),
                created_at: created_at.into(),
                embedding: None,
                embedding_unsupported: None,
            },
        };
        store.initialize()
    }
    fn initialize(self) -> Result<Self, MemoryError> {
        let mut store = self;
        store.recover_if_needed()?;
        store.mkdir(&store.dir)?;
        let path = store.dir.join("meta.json");
        if !path.try_exists()? {
            store.meta.validate()?;
            let mut bytes = serde_json::to_vec_pretty(&store.meta)?;
            bytes.push(b'\n');
            match store
                .home_directory
                .write(store.relative(&path)?, &bytes, true, false)
            {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error.into()),
            }
        }
        store.meta = store.read_meta()?;
        Ok(store)
    }
    pub fn existing(home: &Path, id: &str) -> Result<Self, MemoryError> {
        Self::existing_inner(home, id, true)
    }
    pub fn snapshot(home: &Path, id: &str) -> Result<SourceMemory, MemoryError> {
        let store = Self::existing_inner(home, id, false)?;
        load_files(&store.files()?, &store.read_meta()?)
    }
    fn existing_inner(home: &Path, id: &str, recover: bool) -> Result<Self, MemoryError> {
        let home = std::path::absolute(home)?;
        let dir = home.join("memory").join(memory_slug(id, cfg!(windows))?);
        let mut store = Self {
            dir,
            home_directory: std::sync::Arc::new(crate::secure_fs::Directory::open(&home)?),
            home,
            meta: MemoryMeta {
                schema_version: 1,
                model_id: id.into(),
                created_at: String::new(),
                embedding: None,
                embedding_unsupported: None,
            },
        };
        if recover {
            store.recover_if_needed()?;
        } else if store.transaction_dir()?.try_exists()? {
            return Err(MemoryError(
                "memory recovery required before preview".into(),
            ));
        }
        store.meta = store.read_meta()?;
        Ok(store)
    }
    fn check_path(&self) -> Result<(), MemoryError> {
        self.home_directory.check()?;
        for path in [&self.home, &self.home.join("memory"), &self.dir] {
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(MemoryError("unsafe memory directory".into()));
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if fs::metadata(&self.dir)?.permissions().mode() & 0o077 != 0 {
                return Err(MemoryError("memory store permissions are too broad".into()));
            }
        }
        if !self
            .dir
            .canonicalize()?
            .starts_with(self.home.join("memory").canonicalize()?)
        {
            return Err(MemoryError("memory path escaped root".into()));
        }
        Ok(())
    }
    pub fn read_meta(&self) -> Result<MemoryMeta, MemoryError> {
        self.check_path()?;
        let raw = self.read_at(&self.dir.join("meta.json"), 16384, true)?;
        let value: serde_json::Value = serde_json::from_str(&raw)?;
        if value
            .as_object()
            .is_some_and(|object| object.values().any(serde_json::Value::is_null))
        {
            return Err(MemoryError("null metadata fields".into()));
        }
        let meta: MemoryMeta = serde_json::from_value(value)?;
        meta.validate()?;
        if meta.model_id != self.meta.model_id {
            return Err(MemoryError("memory slug collision".into()));
        }
        Ok(meta)
    }
    pub fn lock(&self) -> Result<LockGuard, MemoryError> {
        Ok(StateStore::new(Config::from_home(&self.home)?).lock(Duration::from_secs(10))?)
    }
    pub fn write_meta(&self, guard: &LockGuard, meta: &MemoryMeta) -> Result<(), MemoryError> {
        guard.assert_for(&Config::from_home(&self.home)?.lock)?;
        self.read_meta()?;
        meta.validate()?;
        if meta.model_id != self.meta.model_id {
            return Err(MemoryError("metadata owner changed".into()));
        }
        let mut bytes = serde_json::to_vec_pretty(meta)?;
        bytes.push(b'\n');
        self.write_at(&self.dir.join("meta.json"), &bytes)
    }
    fn relative<'path>(&self, path: &'path Path) -> Result<&'path Path, MemoryError> {
        path.strip_prefix(&self.home)
            .map_err(|_| MemoryError("path escaped memory home".into()))
    }
    fn mkdir(&self, path: &Path) -> Result<(), MemoryError> {
        Ok(self.home_directory.ensure_dir(self.relative(path)?)?)
    }
    fn write_at(&self, path: &Path, bytes: &[u8]) -> Result<(), MemoryError> {
        Ok(self
            .home_directory
            .write(self.relative(path)?, bytes, false, false)?)
    }
    fn read_at(
        &self,
        path: &Path,
        maximum: usize,
        secret: bool,
    ) -> Result<String, crate::state::StateError> {
        let relative = path
            .strip_prefix(&self.home)
            .map_err(|_| crate::state::StateError {
                kind: "invalid",
                message: "path escaped memory home".into(),
            })?;
        let bytes = self
            .home_directory
            .read(relative, maximum as u64, secret)
            .map_err(|error| crate::state::StateError {
                kind: if error.kind() == std::io::ErrorKind::NotFound {
                    "absent"
                } else {
                    "io"
                },
                message: error.to_string(),
            })?;
        String::from_utf8(bytes).map_err(|_| crate::state::StateError {
            kind: "invalid",
            message: "memory file is not UTF-8".into(),
        })
    }
    fn rename_at(&self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<(), MemoryError> {
        let directory = self.home_directory.dir()?;
        directory.rename(
            self.relative(from.as_ref())?,
            directory,
            self.relative(to.as_ref())?,
        )?;
        #[cfg(unix)]
        directory.open(".")?.sync_all()?;
        Ok(())
    }
    fn remove_dir_at(&self, path: impl AsRef<Path>) -> Result<(), MemoryError> {
        Ok(self
            .home_directory
            .dir()?
            .remove_dir_all(self.relative(path.as_ref())?)?)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub role: String,
    pub content: String,
    pub ts: String,
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Chunk {
    pub id: String,
    pub text: String,
    pub ts: String,
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Vector {
    pub id: String,
    pub vector: Vec<f64>,
}
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fact {
    pub text: String,
    pub ts: String,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Facts {
    schema_version: u8,
    facts: Vec<Fact>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceMemory {
    pub turns: Vec<Turn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    pub facts_text: String,
    pub facts_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding: Option<SourceEmbedding>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceEmbedding {
    pub meta: EmbeddingMeta,
    pub chunks: Vec<Chunk>,
    pub vectors: Vec<Vector>,
}
pub struct EmbeddingOutput {
    pub vectors: Vec<Vec<f64>>,
    pub dimension: usize,
}
#[async_trait::async_trait]
pub trait Embedder: Send + Sync {
    fn model(&self) -> &str;
    async fn embed(
        &self,
        texts: &[String],
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<EmbeddingOutput, MemoryError>;
}
pub struct CaptureOptions<'capture> {
    pub timestamp: &'capture str,
    pub embedder: Option<&'capture dyn Embedder>,
    pub embedding_unsupported: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureResult {
    pub turns_appended: usize,
    pub facts_extracted: usize,
    pub vectors_embedded: usize,
    /// Why recall vectors were not written; the turns and facts were still saved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_skipped: Option<String>,
}
type Files = std::collections::BTreeMap<String, String>;
fn jsonl<T: serde::de::DeserializeOwned>(raw: &str) -> Result<Vec<T>, MemoryError> {
    let mut records = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        records.push(serde_json::from_str(line)?);
        if records.len() > 100000 {
            return Err(MemoryError("record limit exceeded".into()));
        }
    }
    Ok(records)
}
fn encode_lines<T: Serialize>(records: &[T]) -> Result<String, MemoryError> {
    let mut result = String::new();
    for record in records {
        result.push_str(&serde_json::to_string(record)?);
        result.push('\n');
    }
    Ok(result)
}
fn pretty<T: Serialize>(value: &T) -> Result<String, MemoryError> {
    Ok(format!("{}\n", serde_json::to_string_pretty(value)?))
}
pub fn extract_facts(text: &str) -> Result<Vec<String>, MemoryError> {
    let rules = [
        (
            r"(?i)(?-u:\b)(?:my name is|call me)\s+([^.,;!?\n]+)",
            "name = ",
        ),
        (
            r"(?i)(?-u:\b)i (?:live|reside) in\s+([^.,;!?\n]+)",
            "location = ",
        ),
        (
            r"(?i)(?-u:\b)i work (?:as|at)\s+([^.,;!?\n]+)",
            "occupation = ",
        ),
        (
            r"(?i)(?-u:\b)i (?:like|love|prefer|enjoy)\s+([^.,;!?\n]+)",
            "preference = ",
        ),
        (r"(?i)(?-u:\b)remember(?: that)?[:\s]+([^.\n]+)", ""),
    ];
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (pattern, prefix) in rules {
        let regex = regex::Regex::new(pattern).map_err(|error| MemoryError(error.to_string()))?;
        for captures in regex.captures_iter(text) {
            let Some(value) = captures
                .get(1)
                .map(|value| value.as_str().trim())
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let fact = format!("{prefix}{value}");
            if seen.insert(fact.to_lowercase()) {
                result.push(fact);
            }
        }
    }
    Ok(result)
}
fn validate_embeddings(output: &EmbeddingOutput, count: usize) -> Result<(), MemoryError> {
    if output.dimension == 0
        || output.dimension > 100000
        || output.vectors.len() != count
        || count
            .checked_mul(output.dimension)
            .is_none_or(|size| size > 2000000)
        || output.vectors.iter().any(|vector| {
            vector.len() != output.dimension || vector.iter().any(|value| !value.is_finite())
        })
    {
        return Err(MemoryError("invalid embedding vectors or dimension".into()));
    }
    Ok(())
}
impl MemoryStore {
    fn files(&self) -> Result<Files, MemoryError> {
        self.read_meta()?;
        let mut files = Files::new();
        fn scan(
            store: &MemoryStore,
            directory: &cap_std::fs::Dir,
            relative: &Path,
            files: &mut Files,
            total: &mut usize,
            entries: &mut usize,
            depth: usize,
        ) -> Result<(), MemoryError> {
            if depth > 40 {
                return Err(MemoryError("memory directory depth exceeds limit".into()));
            }
            for entry in directory.entries()? {
                let entry = entry?;
                *entries += 1;
                if *entries > 1024 {
                    return Err(MemoryError("memory file count exceeds limit".into()));
                }
                let kind = entry.file_type()?;
                let child = relative.join(entry.file_name());
                if kind.is_dir() {
                    scan(
                        store,
                        &directory.open_dir(entry.file_name())?,
                        &child,
                        files,
                        total,
                        entries,
                        depth + 1,
                    )?;
                } else if kind.is_file() {
                    let relative = child
                        .to_str()
                        .ok_or_else(|| MemoryError("invalid memory filename encoding".into()))?
                        .replace('\\', "/");
                    let maximum = match relative.as_str() {
                        "meta.json" => 16384,
                        "facts.json" => 1048576,
                        "system.md" => 65536,
                        "embeddings/vectors.jsonl" => 16 * 1024 * 1024,
                        _ => 8 * 1024 * 1024,
                    };
                    let raw = store.read_at(&store.dir.join(&child), maximum, true)?;
                    *total += raw.len();
                    if *total > 64 * 1024 * 1024 {
                        return Err(MemoryError("memory store exceeds limit".into()));
                    }
                    files.insert(relative, raw);
                } else {
                    return Err(MemoryError("symlink or special memory file".into()));
                }
            }
            Ok(())
        }
        let directory = self.home_directory.subdir(self.relative(&self.dir)?)?;
        scan(
            self,
            &directory,
            Path::new(""),
            &mut files,
            &mut 0,
            &mut 0,
            0,
        )?;
        Ok(files)
    }
    pub fn load(&self) -> Result<SourceMemory, MemoryError> {
        self.recover_if_needed()?;
        let files = self.files()?;
        load_files(&files, &self.read_meta()?)
    }
    fn transaction_dir(&self) -> Result<PathBuf, MemoryError> {
        Ok(self.home.join(".staging").join(format!(
            "memory-{}",
            memory_slug(&self.meta.model_id, cfg!(windows))?
        )))
    }
    pub fn recover(&self, guard: &LockGuard) -> Result<(), MemoryError> {
        guard.assert_for(&Config::from_home(&self.home)?.lock)?;
        let transaction = self.transaction_dir()?;
        if !transaction.try_exists()? {
            return Ok(());
        }
        self.home_directory.subdir(self.relative(&transaction)?)?;
        let marker = match self.read_at(&transaction.join("phase"), 32, true) {
            Ok(marker) => marker,
            Err(error) if error.kind == "absent" => "staging".into(),
            Err(error) => return Err(error.into()),
        };
        let previous = transaction.join("previous");
        let next = transaction.join("next");
        if marker.trim() == "moving" {
            let target_id = self.read_at(&transaction.join("target"), 8192, true)?;
            if memory_slug(&target_id, cfg!(windows))?
                == memory_slug(&self.meta.model_id, cfg!(windows))?
            {
                return Err(MemoryError("recursive memory recovery".into()));
            }
            let target = Self {
                home_directory: self.home_directory.clone(),
                dir: self
                    .home
                    .join("memory")
                    .join(memory_slug(&target_id, cfg!(windows))?),
                home: self.home.clone(),
                meta: MemoryMeta {
                    model_id: target_id,
                    ..self.meta.clone()
                },
            };
            if target.transaction_dir()?.try_exists()? {
                let phase = self.read_at(&target.transaction_dir()?.join("phase"), 32, true)?;
                if !["prepared", "committed"].contains(&phase.trim()) {
                    return Err(MemoryError("invalid move target journal".into()));
                }
                target.recover(guard)?;
            }
            if transaction.try_exists()? {
                self.remove_dir_at(transaction)?;
            }
            return Ok(());
        }
        if marker.trim() == "staging" {
            if previous.try_exists()? {
                return Err(MemoryError("ambiguous staging journal".into()));
            }
            self.read_meta()?;
            let quarantine = self
                .home
                .join(".staging")
                .join(format!("memory-recovered-{}", uuid::Uuid::new_v4()));
            self.rename_at(transaction, quarantine)?;
            return Ok(());
        }
        if marker.trim() == "committed" {
            self.read_meta()?;
            self.recover_move(guard, false)?;
            self.remove_dir_at(transaction)?;
            return Ok(());
        }
        if marker.trim() != "prepared" {
            return Err(MemoryError("invalid memory recovery journal".into()));
        }
        if previous.try_exists()? {
            self.home_directory.subdir(self.relative(&previous)?)?;
            if self.dir.try_exists()? {
                self.check_path()?;
                if next.try_exists()? {
                    return Err(MemoryError(
                        "ambiguous memory recovery; preserve all generations".into(),
                    ));
                }
                self.rename_at(&self.dir, &next)?;
            }
            self.rename_at(&previous, &self.dir)?;
        }
        self.read_meta()?;
        self.recover_move(guard, true)?;
        if next.try_exists()? {
            let quarantine = self
                .home
                .join(".staging")
                .join(format!("memory-recovered-{}", uuid::Uuid::new_v4()));
            self.rename_at(&next, quarantine)?;
        }
        self.remove_dir_at(transaction)?;
        Ok(())
    }
    fn recover_if_needed(&self) -> Result<(), MemoryError> {
        if self.transaction_dir()?.try_exists()? {
            let guard = self.lock()?;
            self.recover(&guard)?;
            guard.release()?;
        }
        Ok(())
    }
    fn publish(
        &self,
        guard: &LockGuard,
        expected: &Files,
        next: &Files,
    ) -> Result<(), MemoryError> {
        self.publish_pending(guard, expected, next)?;
        self.commit_pending(guard)
    }
    fn publish_pending(
        &self,
        guard: &LockGuard,
        expected: &Files,
        next: &Files,
    ) -> Result<(), MemoryError> {
        guard.assert_for(&Config::from_home(&self.home)?.lock)?;
        self.recover(guard)?;
        if &self.files()? != expected {
            return Err(MemoryError("memory changed during preparation".into()));
        }
        let transaction = self.transaction_dir()?;
        self.mkdir(&self.home.join(".staging"))?;
        if transaction.try_exists()? {
            return Err(MemoryError("memory transaction already exists".into()));
        }
        self.mkdir(&transaction)?;
        self.write_at(&transaction.join("phase"), b"staging\n")?;
        let staged = transaction.join("next");
        self.mkdir(&staged)?;
        let staged_result = (|| {
            for (name, raw) in next {
                if !crate::acquire::safe_file(name) {
                    return Err(MemoryError("unsafe memory artifact name".into()));
                }
                let path = staged.join(name);
                self.mkdir(
                    path.parent()
                        .ok_or_else(|| MemoryError("missing memory parent".into()))?,
                )?;
                self.write_at(&path, raw.as_bytes())?;
            }
            self.write_at(&transaction.join("phase"), b"prepared\n")?;
            Ok::<_, MemoryError>(())
        })();
        if let Err(error) = staged_result {
            self.remove_dir_at(&transaction)?;
            return Err(error);
        }
        let commit = (|| {
            guard.assert_owned()?;
            if &self.files()? != expected {
                return Err(MemoryError("memory changed before publication".into()));
            }
            self.rename_at(&self.dir, transaction.join("previous"))?;
            self.rename_at(&staged, &self.dir)?;
            if self.files()? != *next {
                return Err(MemoryError("published memory verification failed".into()));
            }
            Ok::<_, MemoryError>(())
        })();
        if let Err(error) = commit {
            self.recover(guard)?;
            return Err(error);
        }
        Ok(())
    }
    fn commit_pending(&self, guard: &LockGuard) -> Result<(), MemoryError> {
        guard.assert_for(&Config::from_home(&self.home)?.lock)?;
        self.write_at(&self.transaction_dir()?.join("phase"), b"committed\n")?;
        self.recover(guard)
    }
    fn prepare_move(&self, source: &MemoryStore, guard: &LockGuard) -> Result<(), MemoryError> {
        guard.assert_for(&Config::from_home(&self.home)?.lock)?;
        if source.home != self.home {
            return Err(MemoryError("move crosses memory homes".into()));
        }
        source.check_path()?;
        let recovery = format!("memory-moved-{}", uuid::Uuid::new_v4());
        self.write_at(
            &self.transaction_dir()?.join("move.json"),
            &serde_json::to_vec(&MoveJournal {
                source: source.meta.model_id.clone(),
                recovery: recovery.clone(),
            })?,
        )?;
        let source_transaction = source.transaction_dir()?;
        if source_transaction.try_exists()? {
            return Err(MemoryError("source transaction exists".into()));
        }
        self.mkdir(&source_transaction)?;
        self.write_at(
            &source_transaction.join("target"),
            self.meta.model_id.as_bytes(),
        )?;
        self.write_at(&source_transaction.join("phase"), b"moving\n")?;
        self.rename_at(&source.dir, self.home.join(".staging").join(recovery))?;
        Ok(())
    }
    fn recover_move(&self, guard: &LockGuard, rollback: bool) -> Result<(), MemoryError> {
        guard.assert_for(&Config::from_home(&self.home)?.lock)?;
        let raw = match self.read_at(&self.transaction_dir()?.join("move.json"), 16384, true) {
            Ok(raw) => raw,
            Err(error) if error.kind == "absent" => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let journal: MoveJournal = serde_json::from_str(&raw)?;
        let suffix = journal
            .recovery
            .strip_prefix("memory-moved-")
            .ok_or_else(|| MemoryError("invalid move recovery name".into()))?;
        if uuid::Uuid::parse_str(suffix).is_err() {
            return Err(MemoryError("invalid move recovery id".into()));
        }
        let slug = memory_slug(&journal.source, cfg!(windows))?;
        if slug == memory_slug(&self.meta.model_id, cfg!(windows))? {
            return Err(MemoryError("invalid move source".into()));
        }
        let source = self.home.join("memory").join(&slug);
        let recovery = self.home.join(".staging").join(&journal.recovery);
        if rollback && recovery.try_exists()? {
            self.home_directory.subdir(self.relative(&recovery)?)?;
            let meta: MemoryMeta =
                serde_json::from_str(&self.read_at(&recovery.join("meta.json"), 16384, true)?)?;
            meta.validate()?;
            if meta.model_id != journal.source || source.try_exists()? {
                return Err(MemoryError(
                    "ambiguous source recovery; bytes retained".into(),
                ));
            }
            self.rename_at(recovery, source)?;
        }
        let source_transaction = self.home.join(".staging").join(format!("memory-{slug}"));
        if source_transaction.try_exists()? {
            self.home_directory
                .subdir(self.relative(&source_transaction)?)?;
            let target = self.read_at(&source_transaction.join("target"), 8192, true)?;
            if target != self.meta.model_id {
                return Err(MemoryError("move recovery target changed".into()));
            }
            self.remove_dir_at(source_transaction)?;
        }
        Ok(())
    }
    pub async fn capture(
        &self,
        user: &str,
        assistant: &str,
        options: CaptureOptions<'_>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<CaptureResult, MemoryError> {
        if user.len() + assistant.len() > 8 * 1024 * 1024
            || options.timestamp.is_empty()
            || options.timestamp.len() > 1024
        {
            return Err(MemoryError("invalid capture input".into()));
        }
        if cancel.is_cancelled() {
            return Err(MemoryError("cancelled".into()));
        }
        self.recover_if_needed()?;
        let expected = self.files()?;
        let mut source = load_files(&expected, &self.read_meta()?)?;
        let mut meta = self.read_meta()?;
        let texts = [
            rigspark_core::reports::strip_control(user),
            rigspark_core::reports::strip_control(assistant),
        ];
        let chunks: Vec<_> = texts
            .iter()
            .filter(|text| !text.is_empty())
            .cloned()
            .collect();
        let mut embedding_skipped = None;
        let embedding = if !options.embedding_unsupported && !chunks.is_empty() {
            if let Some(embedder) = options.embedder {
                let attempt = tokio::select! {biased;_=cancel.cancelled()=>return Err(MemoryError("cancelled".into())),result=tokio::time::timeout(Duration::from_secs(120),embedder.embed(&chunks,cancel))=>result};
                // Embeddings only add recall: an embedder that cannot be reached skips the
                // vectors but never loses the conversation. Malformed output or a changed
                // embedding space still refuses the capture, and no vector is fabricated.
                match attempt
                    .map_err(|_| MemoryError("embedding timed out".into()))
                    .and_then(|result| result)
                {
                    Err(error) => {
                        embedding_skipped = Some(format!(
                            "{} ({})",
                            error.0,
                            rigspark_core::reports::strip_control(embedder.model())
                        ));
                        None
                    }
                    Ok(result) => {
                        validate_embeddings(&result, chunks.len())?;
                        let space = EmbeddingMeta {
                            model: embedder.model().into(),
                            dimension: result.dimension,
                        };
                        if space.model.is_empty()
                            || meta
                                .embedding
                                .as_ref()
                                .is_some_and(|existing| existing != &space)
                        {
                            return Err(MemoryError("embedding space mismatch".into()));
                        }
                        Some((space, result))
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        let before = source.turns.len();
        for (role, content) in ["user", "assistant"].into_iter().zip(&texts) {
            if !content.is_empty() {
                source.turns.push(Turn {
                    role: role.into(),
                    content: content.clone(),
                    ts: options.timestamp.into(),
                });
            }
        }
        let mut facts = if source.facts_present {
            serde_json::from_str::<Facts>(&source.facts_text)?
        } else {
            Facts {
                schema_version: 1,
                facts: Vec::new(),
            }
        };
        validate_facts(&facts)?;
        let mut known: std::collections::HashSet<_> = facts
            .facts
            .iter()
            .map(|fact| fact.text.to_lowercase())
            .collect();
        let mut added = 0;
        for text in extract_facts(&texts[0])? {
            if known.insert(text.to_lowercase()) {
                facts.facts.push(Fact {
                    text,
                    ts: options.timestamp.into(),
                });
                added += 1;
            }
        }
        let mut next = expected.clone();
        next.insert("conversation.jsonl".into(), encode_lines(&source.turns)?);
        if added > 0 || !source.facts_present {
            next.insert("facts.json".into(), pretty(&facts)?);
        }
        let mut embedded = 0;
        if options.embedding_unsupported {
            meta.embedding = None;
            meta.embedding_unsupported = Some(true);
        } else if let Some((space, output)) = embedding {
            let mut index = source.embedding.unwrap_or(SourceEmbedding {
                meta: space.clone(),
                chunks: Vec::new(),
                vectors: Vec::new(),
            });
            for (text, vector) in chunks.into_iter().zip(output.vectors) {
                let id = uuid::Uuid::new_v4().to_string();
                index.chunks.push(Chunk {
                    id: id.clone(),
                    text,
                    ts: options.timestamp.into(),
                });
                index.vectors.push(Vector { id, vector });
                embedded += 1;
            }
            meta.embedding = Some(space);
            meta.embedding_unsupported = None;
            next.insert(
                "embeddings/chunks.jsonl".into(),
                encode_lines(&index.chunks)?,
            );
            next.insert(
                "embeddings/vectors.jsonl".into(),
                encode_lines(&index.vectors)?,
            );
        }
        meta.validate()?;
        next.insert("meta.json".into(), pretty(&meta)?);
        load_files(&next, &meta)?;
        let guard = self.lock()?;
        if cancel.is_cancelled() {
            return Err(MemoryError("cancelled".into()));
        }
        self.publish(&guard, &expected, &next)?;
        guard.release()?;
        Ok(CaptureResult {
            turns_appended: source.turns.len() - before,
            facts_extracted: added,
            vectors_embedded: embedded,
            embedding_skipped,
        })
    }
}
fn validate_facts(facts: &Facts) -> Result<(), MemoryError> {
    if facts.schema_version != 1
        || facts.facts.len() > 100000
        || facts
            .facts
            .iter()
            .any(|fact| fact.text.is_empty() || fact.ts.is_empty())
    {
        return Err(MemoryError("invalid facts".into()));
    }
    Ok(())
}
fn load_files(files: &Files, meta: &MemoryMeta) -> Result<SourceMemory, MemoryError> {
    let turns: Vec<Turn> = jsonl(
        files
            .get("conversation.jsonl")
            .map(String::as_str)
            .unwrap_or(""),
    )?;
    if turns
        .iter()
        .any(|turn| !["user", "assistant", "system"].contains(&turn.role.as_str()))
    {
        return Err(MemoryError("invalid stored role".into()));
    }
    let embedding = if let Some(space) = &meta.embedding {
        let chunks: Vec<Chunk> = jsonl(
            files
                .get("embeddings/chunks.jsonl")
                .map(String::as_str)
                .unwrap_or(""),
        )?;
        let vectors: Vec<Vector> = jsonl(
            files
                .get("embeddings/vectors.jsonl")
                .map(String::as_str)
                .unwrap_or(""),
        )?;
        let ids: std::collections::HashSet<_> = chunks.iter().map(|chunk| &chunk.id).collect();
        let vector_ids: std::collections::HashSet<_> =
            vectors.iter().map(|vector| &vector.id).collect();
        if ids.len() != chunks.len()
            || vector_ids.len() != vectors.len()
            || ids != vector_ids
            || vectors.iter().any(|vector| {
                vector.vector.len() != space.dimension
                    || vector.vector.iter().any(|value| !value.is_finite())
            })
        {
            return Err(MemoryError("inconsistent embedding index".into()));
        }
        Some(SourceEmbedding {
            meta: space.clone(),
            chunks,
            vectors,
        })
    } else {
        None
    };
    let facts_text = files.get("facts.json").cloned().unwrap_or_default();
    let facts_present = files.contains_key("facts.json");
    Ok(SourceMemory {
        turns,
        embedding,
        system_prompt: files.get("system.md").cloned(),
        facts_text,
        facts_present,
    })
}

#[async_trait::async_trait]
pub trait Summarizer: Send + Sync {
    async fn summarize(
        &self,
        turns: &[Turn],
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<String, MemoryError>;
}
pub struct MigrationOptions<'migration> {
    pub context: u32,
    pub embedder: Option<&'migration dyn Embedder>,
    pub target_dimension: Option<usize>,
    pub summarizer: Option<&'migration dyn Summarizer>,
    pub embedding_unsupported: bool,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationSummary {
    pub turns_carried: usize,
    pub turns_summarized: usize,
    pub vectors_reembedded: usize,
    pub strategy: String,
    pub embedding_strategy: String,
}
pub struct MigrationPlan {
    pub source: SourceMemory,
    pub embedding_unsupported: bool,
    pub summary: MigrationSummary,
}
fn token_count(text: &str) -> usize {
    text.encode_utf16().count().div_ceil(4)
}
fn truncate_utf16(text: &str, maximum: usize) -> String {
    let mut length = 0;
    text.chars()
        .take_while(|character| {
            length += character.len_utf16();
            length <= maximum
        })
        .collect()
}
pub async fn plan_migration(
    source: &SourceMemory,
    options: MigrationOptions<'_>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<MigrationPlan, MemoryError> {
    if options.context == 0 || options.context > 10000000 || source.turns.len() > 100000 {
        return Err(MemoryError("invalid migration budget or history".into()));
    }
    if cancel.is_cancelled() {
        return Err(MemoryError("cancelled".into()));
    }
    let mut output = source.clone();
    let mut summary = MigrationSummary {
        turns_carried: source.turns.len(),
        turns_summarized: 0,
        vectors_reembedded: 0,
        strategy: "none".into(),
        embedding_strategy: "none".into(),
    };
    let reserved = token_count(source.system_prompt.as_deref().unwrap_or(""))
        + token_count(&source.facts_text);
    let turn_cost = source
        .turns
        .iter()
        .map(|turn| token_count(&turn.content))
        .sum::<usize>();
    if reserved + turn_cost > options.context as usize {
        let available = (options.context as usize).saturating_sub(reserved + 256);
        let mut running = 0;
        let mut kept = 0;
        for turn in source.turns.iter().rev() {
            let cost = token_count(&turn.content);
            if running + cost > available {
                break;
            }
            running += cost;
            kept += 1;
        }
        let overflow = source.turns.len() - kept;
        if overflow > 0 {
            let content = if let Some(summarizer) = options.summarizer {
                let text = tokio::select! {biased;_=cancel.cancelled()=>return Err(MemoryError("cancelled".into())),result=tokio::time::timeout(Duration::from_secs(120),summarizer.summarize(&source.turns[..overflow],cancel))=>result.map_err(|_|MemoryError("summarization timed out".into()))??};
                summary.strategy = "summarize".into();
                truncate_utf16(
                    &format!(
                        "Summary of prior conversation: {}",
                        rigspark_core::reports::strip_control(&text)
                    ),
                    1024,
                )
            } else {
                summary.strategy = "truncate".into();
                format!("[{overflow} earlier turns omitted during migration]")
            };
            output.turns = vec![Turn {
                role: "system".into(),
                content,
                ts: source.turns[0].ts.clone(),
            }];
            output.turns.extend_from_slice(&source.turns[overflow..]);
            summary.turns_carried = kept;
            summary.turns_summarized = overflow;
        }
    }
    if options.embedding_unsupported {
        output.embedding = None;
    } else if let Some(index) = &source.embedding {
        if !index.chunks.is_empty() {
            if let Some(embedder) = options.embedder.filter(|embedder| {
                embedder.model() != index.meta.model
                    || options.target_dimension != Some(index.meta.dimension)
            }) {
                let dimension = options
                    .target_dimension
                    .filter(|dimension| *dimension > 0 && *dimension <= 100000)
                    .ok_or_else(|| MemoryError("target embedding dimension required".into()))?;
                let texts: Vec<_> = index
                    .chunks
                    .iter()
                    .map(|chunk| chunk.text.clone())
                    .collect();
                let output_vectors = tokio::select! {biased;_=cancel.cancelled()=>return Err(MemoryError("cancelled".into())),result=tokio::time::timeout(Duration::from_secs(120),embedder.embed(&texts,cancel))=>result.map_err(|_|MemoryError("reembedding timed out".into()))??};
                validate_embeddings(&output_vectors, texts.len())?;
                if output_vectors.dimension != dimension {
                    return Err(MemoryError("reembedding dimension mismatch".into()));
                }
                let vectors = index
                    .chunks
                    .iter()
                    .zip(output_vectors.vectors)
                    .map(|(chunk, vector)| Vector {
                        id: chunk.id.clone(),
                        vector,
                    })
                    .collect();
                output.embedding = Some(SourceEmbedding {
                    meta: EmbeddingMeta {
                        model: embedder.model().into(),
                        dimension,
                    },
                    chunks: index.chunks.clone(),
                    vectors,
                });
                summary.embedding_strategy = "reembed".into();
                summary.vectors_reembedded = texts.len();
            } else {
                summary.embedding_strategy = "reuse".into();
            }
        } else {
            output.embedding = None;
        }
    }
    Ok(MigrationPlan {
        source: output,
        embedding_unsupported: options.embedding_unsupported,
        summary,
    })
}
pub struct PreparedMigration {
    source_dir: PathBuf,
    source_id: String,
    source_files: Files,
    target_id: String,
    prior_target: Option<Files>,
    plan: MigrationPlan,
}
impl PreparedMigration {
    pub fn summary(&self) -> &MigrationSummary {
        &self.plan.summary
    }
}
impl MemoryStore {
    pub async fn migrate(
        &self,
        target_id: &str,
        created_at: &str,
        options: MigrationOptions<'_>,
        move_source: bool,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<MemoryStore, MemoryError> {
        let prepared = self.prepare_migration(target_id, options, cancel).await?;
        self.commit_migration(prepared, created_at, move_source, cancel)
    }
    pub async fn prepare_migration(
        &self,
        target_id: &str,
        options: MigrationOptions<'_>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<PreparedMigration, MemoryError> {
        if memory_slug(target_id, cfg!(windows))?
            == memory_slug(&self.meta.model_id, cfg!(windows))?
        {
            return Err(MemoryError("source and target stores must differ".into()));
        }
        self.recover_if_needed()?;
        let source_files = self.files()?;
        let source = load_files(&source_files, &self.read_meta()?)?;
        let target_path = self
            .home
            .join("memory")
            .join(memory_slug(target_id, cfg!(windows))?);
        let target_probe = Self {
            home_directory: self.home_directory.clone(),
            dir: target_path.clone(),
            home: self.home.clone(),
            meta: MemoryMeta {
                model_id: target_id.into(),
                ..self.meta.clone()
            },
        };
        target_probe.recover_if_needed()?;
        let prior_target = if target_path.try_exists()? {
            Some(Self::existing(&self.home, target_id)?.files()?)
        } else {
            None
        };
        let plan = plan_migration(&source, options, cancel).await?;
        Ok(PreparedMigration {
            source_dir: self.dir.clone(),
            source_id: self.meta.model_id.clone(),
            source_files,
            target_id: target_id.into(),
            prior_target,
            plan,
        })
    }
    pub fn commit_migration(
        &self,
        prepared: PreparedMigration,
        created_at: &str,
        move_source: bool,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<MemoryStore, MemoryError> {
        self.commit_migration_with(prepared, created_at, move_source, cancel, || Ok(()))
    }
    pub fn commit_migration_with(
        &self,
        prepared: PreparedMigration,
        created_at: &str,
        move_source: bool,
        cancel: &tokio_util::sync::CancellationToken,
        check: impl FnOnce() -> Result<(), MemoryError>,
    ) -> Result<MemoryStore, MemoryError> {
        if prepared.source_dir != self.dir || prepared.source_id != self.meta.model_id {
            return Err(MemoryError(
                "prepared migration belongs to another source".into(),
            ));
        }
        let PreparedMigration {
            source_files,
            target_id,
            prior_target,
            plan,
            ..
        } = prepared;
        let target_id = target_id.as_str();
        let target_path = self
            .home
            .join("memory")
            .join(memory_slug(target_id, cfg!(windows))?);
        let target_probe = Self {
            dir: target_path.clone(),
            home: self.home.clone(),
            home_directory: self.home_directory.clone(),
            meta: MemoryMeta {
                model_id: target_id.into(),
                ..self.meta.clone()
            },
        };
        let guard = self.lock()?;
        check()?;
        if cancel.is_cancelled() {
            return Err(MemoryError("cancelled".into()));
        }
        if self.files()? != source_files {
            return Err(MemoryError(
                "source memory changed during preparation".into(),
            ));
        }
        if target_probe.transaction_dir()?.try_exists()? {
            return Err(MemoryError(
                "target transaction changed during preparation".into(),
            ));
        }
        let current_target = if target_path.try_exists()? {
            Some(Self::existing(&self.home, target_id)?.files()?)
        } else {
            None
        };
        if current_target != prior_target {
            return Err(MemoryError(
                "target memory changed during preparation".into(),
            ));
        }
        let mut target = target_probe;
        target.meta.created_at = created_at.into();
        target.meta.embedding = None;
        target.meta.embedding_unsupported = None;
        let target = target.initialize()?;
        let expected = target.files()?;
        let mut meta = target.read_meta()?;
        meta.embedding = plan
            .source
            .embedding
            .as_ref()
            .map(|index| index.meta.clone());
        meta.embedding_unsupported = plan.embedding_unsupported.then_some(true);
        let mut next = source_files.clone();
        next.insert("meta.json".into(), pretty(&meta)?);
        next.insert(
            "conversation.jsonl".into(),
            encode_lines(&plan.source.turns)?,
        );
        if let Some(persona) = &plan.source.system_prompt {
            next.insert("system.md".into(), persona.clone());
        } else {
            next.remove("system.md");
        }
        if plan.source.facts_present {
            next.insert("facts.json".into(), plan.source.facts_text.clone());
        } else {
            next.remove("facts.json");
        }
        if let Some(index) = &plan.source.embedding {
            next.insert(
                "embeddings/chunks.jsonl".into(),
                encode_lines(&index.chunks)?,
            );
            next.insert(
                "embeddings/vectors.jsonl".into(),
                encode_lines(&index.vectors)?,
            );
        } else {
            next.remove("embeddings/chunks.jsonl");
            next.remove("embeddings/vectors.jsonl");
        }
        load_files(&next, &meta)?;
        if let Err(error) = target.publish_pending(&guard, &expected, &next) {
            if prior_target.is_none() && target.dir.try_exists()? && target.files()? == expected {
                self.remove_dir_at(&target.dir)?;
            }
            return Err(error);
        }
        let finish = (|| {
            if move_source {
                if self.files()? != source_files {
                    return Err(MemoryError("source changed before retirement".into()));
                }
                target.prepare_move(self, &guard)?;
            }
            target.commit_pending(&guard)
        })();
        if let Err(error) = finish {
            target.recover(&guard)?;
            if prior_target.is_none() && target.files()? == expected {
                self.remove_dir_at(&target.dir)?;
            }
            return Err(error);
        }
        guard.release()?;
        Self::existing(&self.home, target_id)
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    fn meta() -> MemoryMeta {
        MemoryMeta {
            schema_version: 1,
            model_id: "qwen3:8b".into(),
            created_at: "2026-11-06T00:00:00Z".into(),
            embedding: Some(EmbeddingMeta {
                model: "nomic-embed-text".into(),
                dimension: 4,
            }),
            embedding_unsupported: None,
        }
    }
    fn turn(content: &str) -> Turn {
        Turn {
            role: "user".into(),
            content: content.into(),
            ts: "t".into(),
        }
    }
    fn source(turns: Vec<Turn>, system: Option<&str>) -> SourceMemory {
        SourceMemory {
            turns,
            system_prompt: system.map(Into::into),
            facts_text: String::new(),
            facts_present: false,
            embedding: None,
        }
    }
    fn budget(context: u32) -> MigrationOptions<'static> {
        MigrationOptions {
            context,
            embedder: None,
            target_dimension: None,
            summarizer: None,
            embedding_unsupported: false,
        }
    }
    struct Fixed(&'static str, usize);
    #[async_trait::async_trait]
    impl Embedder for Fixed {
        fn model(&self) -> &str {
            self.0
        }
        async fn embed(
            &self,
            texts: &[String],
            _: &CancellationToken,
        ) -> Result<EmbeddingOutput, MemoryError> {
            Ok(EmbeddingOutput {
                vectors: texts.iter().map(|_| vec![0.5; self.1]).collect(),
                dimension: self.1,
            })
        }
    }

    #[test]
    fn metadata_limits_are_inclusive() {
        assert!(meta().validate().is_ok());
        let accepted: [fn(&mut MemoryMeta); 4] = [
            |meta| meta.model_id = "m".repeat(8192),
            |meta| meta.created_at = "c".repeat(1024),
            |meta| meta.embedding.as_mut().unwrap().model = "e".repeat(8192),
            |meta| meta.embedding.as_mut().unwrap().dimension = 100_000,
        ];
        for change in accepted {
            let mut edited = meta();
            change(&mut edited);
            assert!(edited.validate().is_ok(), "{edited:?}");
        }
        let mut unsupported = meta();
        unsupported.embedding = None;
        unsupported.embedding_unsupported = Some(true);
        assert!(unsupported.validate().is_ok());
        let rejected: [fn(&mut MemoryMeta); 10] = [
            |meta| meta.schema_version = 2,
            |meta| meta.model_id.clear(),
            |meta| meta.model_id = "m".repeat(8193),
            |meta| meta.created_at.clear(),
            |meta| meta.created_at = "c".repeat(1025),
            |meta| meta.embedding_unsupported = Some(false),
            |meta| meta.embedding.as_mut().unwrap().model.clear(),
            |meta| meta.embedding.as_mut().unwrap().model = "e".repeat(8193),
            |meta| meta.embedding.as_mut().unwrap().dimension = 0,
            |meta| meta.embedding.as_mut().unwrap().dimension = 100_001,
        ];
        for change in rejected {
            let mut edited = meta();
            change(&mut edited);
            assert!(edited.validate().is_err(), "{edited:?}");
        }
    }

    #[test]
    fn slugs_reserve_windows_device_names_and_hash_only_past_128_bytes() {
        assert_eq!(memory_slug("con", false).unwrap(), "con");
        for (id, slug) in [
            ("con", "x-con"),
            ("COM1", "x-com1"),
            ("lpt9.gguf", "x-lpt9.gguf"),
            ("com0", "com0"),
            ("compute", "compute"),
            ("lpt10", "lpt10"),
        ] {
            assert_eq!(memory_slug(id, true).unwrap(), slug, "{id}");
        }
        let exact = "a".repeat(128);
        assert_eq!(memory_slug(&exact, false).unwrap(), exact);
        let long = memory_slug(&"a".repeat(129), false).unwrap();
        assert_eq!(long.len(), 128);
        assert!(long.starts_with(&"a".repeat(111)));
        assert!(memory_slug(&"a".repeat(1024 * 1024), false).is_ok());
        assert!(memory_slug(&"a".repeat(1024 * 1024 + 1), false).is_err());
    }

    #[test]
    fn jsonl_allows_exactly_one_hundred_thousand_records() {
        let lines = "1\n".repeat(100_000);
        assert_eq!(jsonl::<u8>(&lines).unwrap().len(), 100_000);
        assert!(jsonl::<u8>(&format!("{lines}1\n")).is_err());
    }

    #[test]
    fn embedding_output_must_match_count_dimension_and_size_caps() {
        let output = |count: usize, dimension: usize| EmbeddingOutput {
            vectors: vec![vec![0.25; dimension]; count],
            dimension,
        };
        assert!(validate_embeddings(&output(2, 4), 2).is_ok());
        assert!(validate_embeddings(&output(1, 100_000), 1).is_ok());
        assert!(validate_embeddings(&output(1000, 2000), 1000).is_ok());
        assert!(validate_embeddings(&output(1, 0), 1).is_err());
        assert!(validate_embeddings(&output(1, 100_001), 1).is_err());
        assert!(validate_embeddings(&output(2, 4), 3).is_err());
        assert!(validate_embeddings(&output(1001, 2000), 1001).is_err());
        let mut short = output(2, 4);
        short.vectors[1].pop();
        assert!(validate_embeddings(&short, 2).is_err());
        let mut infinite = output(2, 4);
        infinite.vectors[0][0] = f64::NAN;
        assert!(validate_embeddings(&infinite, 2).is_err());
    }

    #[test]
    fn facts_need_schema_one_text_and_timestamp_within_count() {
        let fact = || Fact {
            text: "name = Ada".into(),
            ts: "t".into(),
        };
        let facts = |schema_version, facts| Facts {
            schema_version,
            facts,
        };
        assert!(validate_facts(&facts(1, vec![fact(); 100_000])).is_ok());
        assert!(validate_facts(&facts(1, vec![fact(); 100_001])).is_err());
        assert!(validate_facts(&facts(2, vec![fact()])).is_err());
        let mut blank = fact();
        blank.text.clear();
        assert!(validate_facts(&facts(1, vec![blank])).is_err());
        let mut undated = fact();
        undated.ts.clear();
        assert!(validate_facts(&facts(1, vec![undated])).is_err());
    }

    #[test]
    fn embedding_index_must_pair_unique_ids_with_finite_vectors() {
        let chunk = |id: &str| format!(r#"{{"id":"{id}","text":"x","ts":"t"}}"#);
        let vector = |id: &str, values: &str| format!(r#"{{"id":"{id}","vector":[{values}]}}"#);
        let index = |chunks: &[String], vectors: &[String]| {
            let mut files = Files::new();
            files.insert("embeddings/chunks.jsonl".into(), chunks.join("\n"));
            files.insert("embeddings/vectors.jsonl".into(), vectors.join("\n"));
            load_files(&files, &meta())
        };
        let four = "1,2,3,4";
        assert!(
            index(
                &[chunk("a"), chunk("b")],
                &[vector("a", four), vector("b", four)]
            )
            .is_ok()
        );
        for (chunks, vectors) in [
            (vec![chunk("a"), chunk("a")], vec![vector("a", four)]),
            (vec![chunk("a")], vec![vector("a", four), vector("a", four)]),
            (vec![chunk("a")], vec![vector("b", four)]),
            (vec![chunk("a")], vec![vector("a", "1,2,3")]),
            (vec![chunk("a")], vec![vector("a", "1,2,3,1e999")]),
        ] {
            assert!(index(&chunks, &vectors).is_err(), "{chunks:?} {vectors:?}");
        }
    }

    #[tokio::test]
    async fn migration_budget_bounds_and_exact_truncation() {
        let cancel = CancellationToken::new();
        let empty = source(Vec::new(), None);
        assert!(plan_migration(&empty, budget(0), &cancel).await.is_err());
        assert!(
            plan_migration(&empty, budget(10_000_000), &cancel)
                .await
                .is_ok()
        );
        assert!(
            plan_migration(&empty, budget(10_000_001), &cancel)
                .await
                .is_err()
        );
        let crowded = source(vec![turn("x"); 100_001], None);
        assert!(
            plan_migration(&crowded, budget(10_000_000), &cancel)
                .await
                .is_err()
        );
        let full = source(vec![turn("x"); 100_000], None);
        assert!(
            plan_migration(&full, budget(10_000_000), &cancel)
                .await
                .is_ok()
        );
        // Facts count against the budget alongside the persona.
        let mut factual = source(vec![turn(&"x".repeat(40)); 5], None);
        factual.facts_text = "f".repeat(400);
        factual.facts_present = true;
        let plan = plan_migration(&factual, budget(120), &cancel)
            .await
            .unwrap();
        assert_eq!(plan.summary.strategy, "truncate");

        // 30 turns of 10 tokens against 286 tokens: 256 are reserved for the reply,
        // leaving exactly 30, so the newest 3 turns fit and 27 are folded away.
        let ten_tokens = "x".repeat(40);
        let long = source(vec![turn(&ten_tokens); 30], None);
        let plan = plan_migration(&long, budget(286), &cancel).await.unwrap();
        assert_eq!(plan.summary.strategy, "truncate");
        assert_eq!(plan.summary.turns_carried, 3);
        assert_eq!(plan.summary.turns_summarized, 27);
        assert_eq!(plan.source.turns.len(), 4);
        assert_eq!(
            plan.source.turns[0].content,
            "[27 earlier turns omitted during migration]"
        );
        // Exactly at the budget nothing is dropped.
        let fits = source(vec![turn(&ten_tokens); 30], None);
        let plan = plan_migration(&fits, budget(300), &cancel).await.unwrap();
        assert_eq!(plan.summary.strategy, "none");
        // The persona counts against the budget even with no facts.
        let persona = "p".repeat(400);
        let reserved = source(vec![turn(&ten_tokens); 5], Some(&persona));
        let plan = plan_migration(&reserved, budget(120), &cancel)
            .await
            .unwrap();
        assert_eq!(plan.summary.strategy, "truncate");
        // A persona alone over budget leaves no turns to fold.
        let only_persona = source(Vec::new(), Some(&persona));
        let plan = plan_migration(&only_persona, budget(10), &cancel)
            .await
            .unwrap();
        assert!(plan.source.turns.is_empty());
        assert_eq!(plan.summary.strategy, "none");
    }

    #[tokio::test]
    async fn reembedding_needs_a_plausible_target_dimension() {
        let cancel = CancellationToken::new();
        let mut indexed = source(Vec::new(), None);
        indexed.embedding = Some(SourceEmbedding {
            meta: EmbeddingMeta {
                model: "old".into(),
                dimension: 2,
            },
            chunks: vec![Chunk {
                id: "a".into(),
                text: "x".into(),
                ts: "t".into(),
            }],
            vectors: vec![Vector {
                id: "a".into(),
                vector: vec![1.0, 2.0],
            }],
        });
        let embedder = Fixed("new", 3);
        for (dimension, ok) in [
            (Some(3), true),
            (Some(0), false),
            (Some(100_001), false),
            (None, false),
        ] {
            let options = MigrationOptions {
                embedder: Some(&embedder),
                target_dimension: dimension,
                ..budget(1000)
            };
            let result = plan_migration(&indexed, options, &cancel).await;
            match result {
                Ok(plan) => {
                    assert!(ok, "{dimension:?}");
                    assert_eq!(plan.summary.embedding_strategy, "reembed");
                }
                Err(error) => {
                    assert!(!ok, "{dimension:?}");
                    assert_eq!(error.0, "target embedding dimension required");
                }
            }
        }
    }

    #[cfg(unix)]
    mod store {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        fn private_file(path: &Path, bytes: &[u8]) {
            fs::write(path, bytes).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        fn private_dir(path: &Path) {
            fs::create_dir_all(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fn capture_options(timestamp: &str) -> CaptureOptions<'_> {
            CaptureOptions {
                timestamp,
                embedder: None,
                embedding_unsupported: false,
            }
        }

        #[test]
        fn private_directory_refuses_files() {
            let root = tempfile::tempdir().unwrap();
            let file = root.path().join("file");
            private_file(&file, b"x");
            assert!(private_directory(&file).is_err());
            let created = root.path().join("a/b");
            private_directory(&created).unwrap();
            assert!(created.is_dir());
        }

        #[test]
        fn store_directory_replaced_by_a_file_is_unsafe() {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(root.path(), "model", "now").unwrap();
            fs::remove_dir_all(&store.dir).unwrap();
            private_file(&store.dir, b"{}");
            let error = MemoryStore::existing(root.path(), "model").unwrap_err();
            assert_eq!(error.0, "unsafe memory directory");
        }

        #[test]
        fn metadata_rewrites_keep_their_owner() {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(root.path(), "model", "now").unwrap();
            let guard = store.lock().unwrap();
            let mut edited = store.meta.clone();
            edited.embedding_unsupported = Some(true);
            store.write_meta(&guard, &edited).unwrap();
            assert_eq!(store.read_meta().unwrap(), edited);
            edited.model_id = "other".into();
            assert!(store.write_meta(&guard, &edited).is_err());
            guard.release().unwrap();
        }

        #[test]
        fn scan_allows_forty_levels_and_1024_entries() {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(root.path(), "model", "now").unwrap();
            let mut nested = store.dir.clone();
            for level in 0..40 {
                nested = nested.join(format!("d{level}"));
            }
            private_dir(&nested);
            assert!(store.files().is_ok());
            private_dir(&nested.join("deeper"));
            assert!(store.files().is_err());
            fs::remove_dir_all(store.dir.join("d0")).unwrap();
            // meta.json plus 1023 files is exactly the entry limit.
            for index in 0..1023 {
                private_file(&store.dir.join(format!("f{index}")), b"");
            }
            assert_eq!(store.files().unwrap().len(), 1024);
            private_file(&store.dir.join("f1023"), b"");
            assert!(store.files().is_err());
        }

        #[test]
        fn scan_applies_per_file_and_total_size_caps() {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(root.path(), "model", "now").unwrap();
            let mib = 1024 * 1024;
            private_file(&store.dir.join("facts.json"), &vec![b' '; mib + 1]);
            assert!(store.files().is_err());
            fs::remove_file(store.dir.join("facts.json")).unwrap();
            private_dir(&store.dir.join("embeddings"));
            private_file(
                &store.dir.join("embeddings/vectors.jsonl"),
                &vec![b' '; 10 * mib],
            );
            private_file(&store.dir.join("notes.txt"), &vec![b' '; 8 * mib]);
            assert!(store.files().is_ok());
            private_file(&store.dir.join("notes.txt"), &vec![b' '; 8 * mib + 1]);
            assert!(store.files().is_err());
            fs::remove_dir_all(store.dir.join("embeddings")).unwrap();
            fs::remove_file(store.dir.join("notes.txt")).unwrap();
            // Fill the store to exactly 64 MiB, then one byte past it.
            let meta = fs::metadata(store.dir.join("meta.json")).unwrap().len() as usize;
            for index in 0..7 {
                private_file(
                    &store.dir.join(format!("part{index}")),
                    &vec![b' '; 8 * mib],
                );
            }
            private_file(&store.dir.join("part7"), &vec![b' '; 8 * mib - meta]);
            assert!(store.files().is_ok());
            private_file(&store.dir.join("extra"), b" ");
            assert_eq!(store.files().unwrap_err().0, "memory store exceeds limit");
        }

        #[test]
        fn scan_caps_metadata_at_16_kib() {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(root.path(), "model", "now").unwrap();
            let path = store.dir.join("meta.json");
            let mut raw = fs::read(&path).unwrap();
            raw.resize(16384, b' ');
            private_file(&path, &raw);
            assert!(store.files().is_ok());
            raw.push(b' ');
            private_file(&path, &raw);
            assert!(store.files().is_err());
        }

        #[test]
        fn move_recovery_refuses_a_parked_store_for_another_source() {
            let root = tempfile::tempdir().unwrap();
            MemoryStore::open(root.path(), "model", "now").unwrap();
            let parked = MemoryStore::open(root.path(), "src", "now").unwrap();
            let recovery = format!("memory-moved-{}", uuid::Uuid::new_v4());
            let journal = transaction(root.path());
            fs::rename(&parked.dir, root.path().join(".staging").join(&recovery)).unwrap();
            private_file(&journal.join("phase"), b"prepared\n");
            private_file(
                &journal.join("move.json"),
                serde_json::json!({"source": "other", "recovery": recovery})
                    .to_string()
                    .as_bytes(),
            );
            let error = MemoryStore::existing(root.path(), "model").unwrap_err();
            assert_eq!(error.0, "ambiguous source recovery; bytes retained");
            assert!(!root.path().join("memory/other").exists());
        }

        fn transaction(root: &Path) -> PathBuf {
            let path = root.join(".staging").join("memory-model");
            private_dir(&path);
            path
        }

        #[test]
        fn a_journal_without_a_phase_is_an_abandoned_staging_area() {
            let root = tempfile::tempdir().unwrap();
            MemoryStore::open(root.path(), "model", "now").unwrap();
            let journal = transaction(root.path());
            MemoryStore::existing(root.path(), "model").unwrap();
            assert!(!journal.exists());
        }

        #[test]
        fn unreadable_journal_files_stop_recovery() {
            let root = tempfile::tempdir().unwrap();
            MemoryStore::open(root.path(), "model", "now").unwrap();
            let journal = transaction(root.path());
            private_file(&journal.join("phase"), &[b'x'; 100]);
            assert!(MemoryStore::existing(root.path(), "model").is_err());
            private_file(&journal.join("phase"), b"committed\n");
            private_file(&journal.join("move.json"), &vec![b' '; 20_000]);
            assert!(MemoryStore::existing(root.path(), "model").is_err());
        }

        #[tokio::test]
        async fn capture_input_caps_are_inclusive() {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(root.path(), "model", "now").unwrap();
            let cancel = CancellationToken::new();
            let invalid = "memory: invalid capture input";
            let error = |result: Result<CaptureResult, MemoryError>| {
                result.err().map(|error| error.to_string())
            };
            let limit = 8 * 1024 * 1024;
            let over = "x".repeat(limit + 1);
            assert_eq!(
                error(
                    store
                        .capture(&over, "", capture_options("t"), &cancel)
                        .await
                )
                .as_deref(),
                Some(invalid)
            );
            let half = "x".repeat(limit / 2 + 1);
            assert_eq!(
                error(
                    store
                        .capture(&half, &half, capture_options("t"), &cancel)
                        .await
                )
                .as_deref(),
                Some(invalid)
            );
            let exact = "x".repeat(limit);
            assert_ne!(
                error(
                    store
                        .capture(&exact, "", capture_options("t"), &cancel)
                        .await
                )
                .as_deref(),
                Some(invalid)
            );
            for timestamp in [String::new(), "t".repeat(1025)] {
                assert_eq!(
                    error(
                        store
                            .capture("hi", "", capture_options(&timestamp), &cancel)
                            .await
                    )
                    .as_deref(),
                    Some(invalid)
                );
            }
            let timestamp = "t".repeat(1024);
            store
                .capture("hi", "", capture_options(&timestamp), &cancel)
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn capture_keeps_one_embedding_space_and_leaves_facts_alone() {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(root.path(), "model", "now").unwrap();
            let cancel = CancellationToken::new();
            let embedder = Fixed("embed", 2);
            let with = |embedder: &'static Fixed| CaptureOptions {
                timestamp: "t",
                embedder: Some(embedder),
                embedding_unsupported: false,
            };
            let embedder: &'static Fixed = Box::leak(Box::new(embedder));
            store
                .capture("hi", "hello", with(embedder), &cancel)
                .await
                .unwrap();
            let again = store
                .capture("hi", "hello", with(embedder), &cancel)
                .await
                .unwrap();
            assert_eq!(again.vectors_embedded, 2);
            let other: &'static Fixed = Box::leak(Box::new(Fixed("embed", 3)));
            assert!(store.capture("hi", "", with(other), &cancel).await.is_err());
            let unnamed: &'static Fixed = Box::leak(Box::new(Fixed("", 2)));
            let fresh = MemoryStore::open(root.path(), "fresh", "now").unwrap();
            assert_eq!(
                fresh
                    .capture("hi", "", with(unnamed), &cancel)
                    .await
                    .unwrap_err()
                    .0,
                "embedding space mismatch"
            );
            // A capture that finds no new facts leaves a hand-written facts file byte for byte.
            let compact = r#"{"schemaVersion":1,"facts":[{"text":"name = Ada","ts":"t"}]}"#;
            private_file(&store.dir.join("facts.json"), compact.as_bytes());
            store
                .capture("nothing to remember", "", capture_options("t"), &cancel)
                .await
                .unwrap();
            assert_eq!(
                fs::read_to_string(store.dir.join("facts.json")).unwrap(),
                compact
            );
        }

        #[tokio::test]
        async fn a_prepared_migration_only_commits_on_its_own_source() {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(root.path(), "model", "now").unwrap();
            let cancel = CancellationToken::new();
            let prepared = store
                .prepare_migration("target", budget(1000), &cancel)
                .await
                .unwrap();
            let mut renamed = store.clone();
            renamed.meta.model_id = "imposter".into();
            let error = renamed
                .commit_migration(prepared, "now", false, &cancel)
                .err()
                .unwrap();
            assert_eq!(error.0, "prepared migration belongs to another source");
        }

        #[tokio::test]
        async fn capture_writes_facts_when_missing_or_when_one_is_learned() {
            let root = tempfile::tempdir().unwrap();
            let store = MemoryStore::open(root.path(), "model", "now").unwrap();
            let cancel = CancellationToken::new();
            let facts = store.dir.join("facts.json");
            assert!(!facts.exists());
            store
                .capture("nothing to remember", "", capture_options("t"), &cancel)
                .await
                .unwrap();
            assert!(facts.exists());
            let compact = r#"{"schemaVersion":1,"facts":[{"text":"name = Ada","ts":"t"}]}"#;
            private_file(&facts, compact.as_bytes());
            store
                .capture("I live in Paris", "", capture_options("t"), &cancel)
                .await
                .unwrap();
            let written = fs::read_to_string(&facts).unwrap();
            assert!(written.contains("name = Ada"), "{written}");
            assert!(written.contains("location = Paris"), "{written}");
        }

        #[tokio::test]
        async fn a_migration_commits_into_the_target_and_can_retire_the_source() {
            let root = tempfile::tempdir().unwrap();
            let cancel = CancellationToken::new();
            let store = MemoryStore::open(root.path(), "model", "now").unwrap();
            store
                .capture("hi", "hello", capture_options("t"), &cancel)
                .await
                .unwrap();
            // An abandoned staging area for an existing target is cleared while preparing.
            MemoryStore::open(root.path(), "target", "now").unwrap();
            let stray = root.path().join(".staging").join("memory-target");
            private_dir(&stray);
            let prepared = store
                .prepare_migration("target", budget(1000), &cancel)
                .await
                .unwrap();
            assert!(!stray.exists());
            let target = store
                .commit_migration(prepared, "later", false, &cancel)
                .unwrap();
            assert_eq!(target.meta.model_id, "target");
            assert_eq!(target.read_meta().unwrap().model_id, "target");
            let copied = MemoryStore::snapshot(root.path(), "target").unwrap();
            assert_eq!(copied.turns.len(), 2);
            assert!(store.dir.exists());
            // Retiring the source moves it out of the memory directory.
            let other = MemoryStore::open(root.path(), "other", "now").unwrap();
            let prepared = other
                .prepare_migration("fresh", budget(1000), &cancel)
                .await
                .unwrap();
            let fresh = other
                .commit_migration(prepared, "later", true, &cancel)
                .unwrap();
            assert_eq!(fresh.read_meta().unwrap().model_id, "fresh");
            assert!(!other.dir.exists());
        }
    }
}
