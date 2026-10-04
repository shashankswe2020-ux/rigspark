use async_trait::async_trait;
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_util::{io::StreamReader, sync::CancellationToken};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub backend: String,
    pub repo: String,
    pub revision: String,
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
}
pub struct DownloadResponse {
    pub status: u16,
    pub commit: Option<String>,
    pub length: Option<u64>,
    pub body: Pin<Box<dyn AsyncRead + Send>>,
}
#[async_trait]
pub trait DownloadTransport: Send + Sync {
    async fn get(&self, url: &str) -> Result<DownloadResponse, String>;
    /// Continues a download at `offset`; transports without range support refuse.
    async fn get_from(&self, url: &str, offset: u64) -> Result<DownloadResponse, String> {
        if offset == 0 {
            self.get(url).await
        } else {
            Err("download resume unsupported".into())
        }
    }
}
const MAX_RESUMES: u32 = 8;
pub struct HfTransport {
    client: reqwest::Client,
}
fn retain_commit(evidence: &mut Option<String>, observed: Option<&str>) -> Result<(), String> {
    if let Some(observed) = observed {
        if observed.len() != 40 || !observed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("invalid repository commit evidence".into());
        }
        if evidence
            .as_ref()
            .is_some_and(|previous| !previous.eq_ignore_ascii_case(observed))
        {
            return Err("conflicting repository commit evidence".into());
        }
        *evidence = Some(observed.to_owned());
    }
    Ok(())
}
#[test]
fn commit_evidence_survives_cdn_redirects_and_rejects_conflicts() {
    let revision = "a".repeat(40);
    let mut evidence = None;
    retain_commit(&mut evidence, Some(&revision)).unwrap();
    retain_commit(&mut evidence, None).unwrap();
    assert_eq!(evidence.as_deref(), Some(revision.as_str()));
    assert!(retain_commit(&mut evidence, Some(&"b".repeat(40))).is_err());
    assert!(retain_commit(&mut None, Some("not-a-commit")).is_err());
    let mut absent = None;
    retain_commit(&mut absent, None).unwrap();
    assert!(absent.is_none());
}
impl HfTransport {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .dns_resolver(std::sync::Arc::new(PublicResolver))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .map_err(|error| error.to_string())?,
        })
    }
}
fn safe_url(raw: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(raw).map_err(|error| error.to_string())?;
    let host = url.host_str().ok_or("missing download host")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !["huggingface.co", "hf.co"]
            .iter()
            .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
    {
        return Err("download URL is not approved HTTPS source".into());
    }
    Ok(url)
}
/// Accepts only `bytes <offset>-<end>/<total>` so a resume can never splice the wrong range.
fn content_range_starts_at(value: Option<&str>, offset: u64) -> bool {
    value
        .and_then(|value| value.strip_prefix("bytes "))
        .and_then(|range| range.split_once('-'))
        .and_then(|(start, _)| start.parse::<u64>().ok())
        == Some(offset)
}
#[test]
fn resume_requires_matching_content_range() {
    assert!(content_range_starts_at(Some("bytes 40-99/100"), 40));
    assert!(!content_range_starts_at(Some("bytes 0-99/100"), 40));
    assert!(!content_range_starts_at(Some("items 40-99/100"), 40));
    assert!(!content_range_starts_at(None, 40));
}
#[async_trait]
impl DownloadTransport for HfTransport {
    async fn get(&self, raw: &str) -> Result<DownloadResponse, String> {
        self.get_from(raw, 0).await
    }
    async fn get_from(&self, raw: &str, offset: u64) -> Result<DownloadResponse, String> {
        let mut url = safe_url(raw)?;
        let mut commit = None;
        for redirect in 0..=5 {
            let mut request = self.client.get(url.clone());
            if offset > 0 {
                request = request.header("range", format!("bytes={offset}-"));
            }
            let response = request
                .send()
                .await
                .map_err(|_| "download transport failed".to_owned())?;
            retain_commit(
                &mut commit,
                response
                    .headers()
                    .get("x-repo-commit")
                    .map(|value| {
                        value
                            .to_str()
                            .map_err(|_| "invalid repository commit header".to_owned())
                    })
                    .transpose()?,
            )?;
            if response.status().is_redirection() {
                if redirect == 5 {
                    return Err("too many download redirects".into());
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or("redirect has no location")?;
                url = safe_url(
                    url.join(location)
                        .map_err(|error| error.to_string())?
                        .as_str(),
                )?;
                continue;
            }
            let length = response.content_length();
            let status = response.status().as_u16();
            if offset > 0
                && (status != 206
                    || !content_range_starts_at(
                        response
                            .headers()
                            .get("content-range")
                            .and_then(|value| value.to_str().ok()),
                        offset,
                    ))
            {
                return Err("download resume was not honored".into());
            }
            let stream = response.bytes_stream().map_err(std::io::Error::other);
            return Ok(DownloadResponse {
                status,
                commit,
                length,
                body: Box::pin(StreamReader::new(stream)),
            });
        }
        Err("download redirect limit".into())
    }
}

pub fn public_addresses(
    addresses: Vec<std::net::SocketAddr>,
) -> Result<Vec<std::net::SocketAddr>, String> {
    fn public(address: std::net::IpAddr) -> bool {
        match address {
            std::net::IpAddr::V4(address) => {
                let octets = address.octets();
                !address.is_private()
                    && !address.is_loopback()
                    && !address.is_link_local()
                    && !address.is_multicast()
                    && !address.is_unspecified()
                    && !address.is_broadcast()
                    && !address.is_documentation()
                    && octets[0] != 0
                    && octets[0] < 224
                    && !(octets[0] == 100 && (64..=127).contains(&octets[1]))
                    && !(octets[0] == 198 && (18..=19).contains(&octets[1]))
                    && !(octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
            }
            std::net::IpAddr::V6(address) => {
                if let Some(mapped) = address.to_ipv4_mapped() {
                    return public(mapped.into());
                }
                let segments = address.segments();
                segments[0] & 0xe000 == 0x2000
                    && segments[0] != 0x2002
                    && !(segments[0] == 0x2001 && (segments[1] < 0x200 || segments[1] == 0xdb8))
            }
        }
    }
    if addresses.is_empty()
        || addresses.len() > 64
        || addresses.iter().any(|address| !public(address.ip()))
    {
        return Err("download DNS contains private or unsupported addresses".into());
    }
    Ok(addresses)
}
struct PublicResolver;
impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addresses = tokio::time::timeout(
                Duration::from_secs(5),
                tokio::net::lookup_host((name.as_str(), 0)),
            )
            .await
            .map_err(|_| std::io::Error::other("download DNS timed out"))??;
            let addresses =
                public_addresses(addresses.take(65).collect()).map_err(std::io::Error::other)?;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

#[derive(Debug)]
pub struct Acquired {
    pub path: PathBuf,
    pub bytes: u64,
    pub cached: bool,
}
#[derive(Debug, Clone, Copy)]
pub enum SizePolicy {
    Exact,
    Ceiling,
}
impl SizePolicy {
    fn accepts(self, actual: u64, expected: u64) -> bool {
        actual > 0
            && match self {
                Self::Exact => actual == expected,
                Self::Ceiling => actual <= expected,
            }
    }
}
pub struct Acquisition {
    root: PathBuf,
    progress: Option<std::sync::Arc<Progress>>,
    timeout: Duration,
}
type Progress = dyn Fn(u64, u64, &str) + Send + Sync;

struct ArtifactLock {
    guard: crate::state::LockGuard,
}
impl ArtifactLock {
    fn acquire(path: PathBuf) -> Result<Self, String> {
        let mut config =
            crate::state::Config::from_home(path.parent().ok_or("missing lock parent")?)
                .map_err(|error| error.to_string())?;
        config.lock = path;
        let guard = crate::state::StateStore::new(config)
            .lock(Duration::ZERO)
            .map_err(|error| error.to_string())?;
        Ok(Self { guard })
    }
    fn check(&self) -> Result<(), String> {
        self.guard.assert_owned().map_err(|error| error.to_string())
    }
}
fn suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_owned();
    value.push(suffix);
    PathBuf::from(value)
}
pub fn cleanup_partials(parent: &Path, alive: impl Fn(u32) -> bool) -> Result<(), String> {
    for (index, entry) in std::fs::read_dir(parent)
        .map_err(|error| error.to_string())?
        .enumerate()
    {
        if index >= 10000 {
            return Err("cache directory exceeds cleanup entry limit".into());
        }
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(rest) = name
            .strip_prefix(".llmup-download.")
            .and_then(|value| value.strip_suffix(".part"))
        else {
            continue;
        };
        let Some((pid, unique)) = rest.split_once('.') else {
            continue;
        };
        if unique.is_empty()
            || unique.contains('.')
            || !pid.bytes().all(|byte| byte.is_ascii_digit())
            || pid.starts_with('0')
        {
            continue;
        }
        let Some(pid) = pid
            .parse::<u32>()
            .ok()
            .filter(|pid| *pid > 0 && *pid <= i32::MAX as u32)
        else {
            continue;
        };
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_file()
            && !alive(pid)
        {
            std::fs::remove_file(entry.path()).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}
fn segment(value: &str) -> bool {
    !value.is_empty()
        && portable_component(value)
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}
fn portable_component(value: &str) -> bool {
    let base = value.split('.').next().unwrap_or("").to_ascii_uppercase();
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.ends_with('.')
        && value.trim() == value
        && !["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&base.as_str())
        && !(base.starts_with("COM") || base.starts_with("LPT"))
            .then(|| &base[3..])
            .is_some_and(|suffix| {
                [
                    "1", "2", "3", "4", "5", "6", "7", "8", "9", "\u{b9}", "\u{b2}", "\u{b3}",
                ]
                .contains(&suffix)
            })
}
pub fn safe_file(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value
            .chars()
            .any(|ch| ch.is_control() || "\\%*?[]{}:".contains(ch))
        && value.split('/').all(portable_component)
}
fn digest(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
impl Artifact {
    pub fn validate(&self) -> Result<(), String> {
        let coordinates: Vec<_> = self.repo.split('/').collect();
        if !["llamacpp", "mlx", "comfyui"].contains(&self.backend.as_str())
            || coordinates.len() != 2
            || !coordinates.iter().all(|part| segment(part))
            || !digest(&self.revision, 40)
            || !digest(&self.sha256, 64)
            || !safe_file(&self.file)
            || self.bytes == 0
            || self.bytes > 9007199254740991
        {
            return Err("invalid pinned artifact".into());
        }
        Ok(())
    }
}
fn private_directory(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("unsafe cache directory".into());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                private_directory(parent)?;
            }
            let builder = std::fs::DirBuilder::new();
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
                Err(error) => return Err(error.to_string()),
            }
        }
        Err(error) => return Err(error.to_string()),
    }
    Ok(())
}
pub async fn hash_file(path: &Path, cancel: &CancellationToken) -> Result<(String, u64), String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
        );
    }
    let file = options.open(path).map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
        || std::fs::symlink_metadata(path)
            .map_err(|error| error.to_string())?
            .file_type()
            .is_symlink()
    {
        return Err("artifact is not a regular file".into());
    }
    let mut file = tokio::fs::File::from_std(file);
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0u8; 65536];
    loop {
        let count = tokio::select! { _=cancel.cancelled()=>return Err("cancelled".into()), result=file.read(&mut buffer)=>result.map_err(|error|error.to_string())? };
        if count == 0 {
            break;
        }
        total = total
            .checked_add(count as u64)
            .ok_or("artifact size overflow")?;
        hash.update(&buffer[..count]);
    }
    Ok((format!("{:x}", hash.finalize()), total))
}
impl Acquisition {
    fn cache_directory(&self, path: &Path) -> Result<(), String> {
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|_| "cache path escaped root")?;
        let mut current = self.root.clone();
        private_directory(&current)?;
        for component in relative.components() {
            let std::path::Component::Normal(name) = component else {
                return Err("invalid cache path component".into());
            };
            current.push(name);
            private_directory(&current)?;
            if !current
                .canonicalize()
                .map_err(|error| error.to_string())?
                .starts_with(&self.root)
            {
                return Err("cache path escaped root".into());
            }
        }
        Ok(())
    }
    pub fn new(root: impl AsRef<Path>) -> Result<Self, String> {
        let root = std::path::absolute(root).map_err(|error| error.to_string())?;
        private_directory(&root)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
        }
        Ok(Self {
            root: root.canonicalize().map_err(|error| error.to_string())?,
            progress: None,
            timeout: Duration::from_secs(1800),
        })
    }
    pub fn with_progress(
        mut self,
        progress: impl Fn(u64, u64, &str) + Send + Sync + 'static,
    ) -> Self {
        self.progress = Some(std::sync::Arc::new(progress));
        self
    }
    /// Overrides the default 30 minute limit for multi-gigabyte artifacts.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
    fn emit(&self, completed: u64, artifact: &Artifact) {
        if let Some(progress) = &self.progress {
            progress(completed, artifact.bytes, &artifact.file);
        }
    }
    pub async fn acquire(
        &self,
        artifact: &Artifact,
        transport: &dyn DownloadTransport,
        cancel: &CancellationToken,
    ) -> Result<Acquired, String> {
        self.acquire_sized(artifact, SizePolicy::Exact, transport, cancel)
            .await
    }
    pub async fn acquire_sized(
        &self,
        artifact: &Artifact,
        size: SizePolicy,
        transport: &dyn DownloadTransport,
        cancel: &CancellationToken,
    ) -> Result<Acquired, String> {
        let operation = async {
            artifact.validate()?;
            let coordinates: Vec<_> = artifact.repo.split('/').collect();
            let owner = self.root.join(&artifact.backend).join(coordinates[0]);
            self.cache_directory(&owner)?;
            let repository = owner.join(format!("{}@{}", coordinates[1], artifact.revision));
            let lock = ArtifactLock::acquire(suffix(&repository, ".repository.lock"))?;
            let acquired = self
                .acquire_inner(artifact, size, transport, cancel)
                .await?;
            lock.check()?;
            Ok(acquired)
        };
        tokio::select! {biased;_=cancel.cancelled()=>Err("cancelled".into()),result=tokio::time::timeout(self.timeout,operation)=>result.map_err(|_|"acquisition timed out".to_owned())?}
    }
    async fn acquire_inner(
        &self,
        artifact: &Artifact,
        size: SizePolicy,
        transport: &dyn DownloadTransport,
        cancel: &CancellationToken,
    ) -> Result<Acquired, String> {
        artifact.validate()?;
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        let coordinates: Vec<_> = artifact.repo.split('/').collect();
        let directory = self
            .root
            .join(&artifact.backend)
            .join(coordinates[0])
            .join(format!("{}@{}", coordinates[1], artifact.revision));
        let target = directory.join(&artifact.file);
        let parent = target.parent().ok_or("missing artifact parent")?;
        self.cache_directory(parent)?;
        if !parent
            .canonicalize()
            .map_err(|error| error.to_string())?
            .starts_with(&self.root)
        {
            return Err("cache escaped root".into());
        }
        let lock = ArtifactLock::acquire(suffix(&target, ".lock"))?;
        cleanup_partials(parent, crate::state::process_alive)?;
        self.emit(0, artifact);
        if let Ok(metadata) = std::fs::symlink_metadata(&target) {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err("unsafe cached artifact".into());
            }
            let (sha, bytes) = hash_file(&target, cancel).await?;
            if sha.eq_ignore_ascii_case(&artifact.sha256) && size.accepts(bytes, artifact.bytes) {
                lock.check()?;
                self.emit(bytes, artifact);
                return Ok(Acquired {
                    path: target,
                    bytes,
                    cached: true,
                });
            }
        }
        let mut url =
            url::Url::parse("https://huggingface.co").map_err(|error| error.to_string())?;
        {
            let mut path = url.path_segments_mut().map_err(|_| "invalid URL base")?;
            path.extend(coordinates)
                .push("resolve")
                .push(&artifact.revision)
                .extend(artifact.file.split('/'));
        }
        url.set_query(Some("download=true"));
        let operation = async {
            let mut response = transport.get(url.as_str()).await?;
            if !(200..300).contains(&response.status) {
                return Err(format!("download HTTP {}", response.status));
            }
            if response
                .commit
                .as_ref()
                .is_none_or(|commit| !commit.eq_ignore_ascii_case(&artifact.revision))
            {
                return Err("resolved commit mismatch".into());
            }
            if response
                .length
                .is_some_and(|length| length > artifact.bytes)
            {
                return Err("download exceeds byte limit".into());
            }
            let temporary = tempfile::Builder::new()
                .prefix(&format!(".llmup-download.{}.", std::process::id()))
                .suffix(".part")
                .tempfile_in(parent)
                .map_err(|error| error.to_string())?;
            let mut output =
                tokio::fs::File::from_std(temporary.reopen().map_err(|error| error.to_string())?);
            let mut hash = Sha256::new();
            let mut total = 0u64;
            let mut reported = 0u64;
            let mut buffer = vec![0u8; 65536];
            let mut resumes = 0u32;
            loop {
                let count = match response.body.read(&mut buffer).await {
                    Ok(count) => count,
                    Err(error) => {
                        resumes += 1;
                        if resumes > MAX_RESUMES {
                            return Err(error.to_string());
                        }
                        tokio::time::sleep(Duration::from_secs(u64::from(resumes.min(5)))).await;
                        response = transport
                            .get_from(url.as_str(), total)
                            .await
                            .map_err(|resume| format!("{error}; {resume}"))?;
                        if response.status != 206
                            || response.commit.as_ref().is_none_or(|commit| {
                                !commit.eq_ignore_ascii_case(&artifact.revision)
                            })
                            || response
                                .length
                                .is_some_and(|length| length > artifact.bytes - total)
                        {
                            return Err("download resume response mismatch".into());
                        }
                        continue;
                    }
                };
                if count == 0 {
                    break;
                }
                total = total
                    .checked_add(count as u64)
                    .ok_or("download size overflow")?;
                if total > artifact.bytes {
                    return Err("download exceeds byte limit".into());
                }
                hash.update(&buffer[..count]);
                output
                    .write_all(&buffer[..count])
                    .await
                    .map_err(|error| error.to_string())?;
                if total - reported >= 8 * 1024 * 1024 {
                    self.emit(total, artifact);
                    reported = total;
                }
            }
            if !size.accepts(total, artifact.bytes)
                || !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(&artifact.sha256)
            {
                return Err("artifact integrity mismatch".into());
            }
            output.sync_all().await.map_err(|error| error.to_string())?;
            drop(output);
            lock.check()?;
            if cancel.is_cancelled() {
                return Err("cancelled".into());
            }
            temporary
                .persist(&target)
                .map_err(|error| error.to_string())?;
            self.emit(total, artifact);
            Ok(Acquired {
                path: target.clone(),
                bytes: total,
                cached: false,
            })
        };
        tokio::select! { _=cancel.cancelled()=>Err("cancelled".into()), result=tokio::time::timeout(self.timeout,operation)=>result.map_err(|_|"download timed out".to_owned())? }
    }
    pub async fn repository(
        &self,
        artifacts: &[Artifact],
        transport: &dyn DownloadTransport,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, String> {
        tokio::select! {biased;_=cancel.cancelled()=>Err("cancelled".into()),result=tokio::time::timeout(Duration::from_secs(1800),self.repository_inner(artifacts,transport,cancel))=>result.map_err(|_|"repository acquisition timed out".to_owned())?}
    }
    async fn repository_inner(
        &self,
        artifacts: &[Artifact],
        transport: &dyn DownloadTransport,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, String> {
        let first = artifacts.first().ok_or("empty repository manifest")?;
        if artifacts.len() > 256 {
            return Err("repository file count exceeds limit".into());
        }
        let mut names = HashSet::new();
        let mut root = None;
        for artifact in artifacts {
            artifact.validate()?;
            if ["py", "pyc", "pyo", "so", "dylib", "dll", "bundle"]
                .iter()
                .any(|extension| {
                    artifact
                        .file
                        .to_ascii_lowercase()
                        .ends_with(&format!(".{extension}"))
                })
            {
                return Err("executable repository artifact forbidden".into());
            }
            if artifact.repo != first.repo
                || artifact.revision != first.revision
                || artifact.backend != first.backend
                || !names.insert(artifact.file.clone())
            {
                return Err("inconsistent repository manifest".into());
            }
        }
        let folded: HashSet<_> = names.iter().map(|name| name.to_lowercase()).collect();
        if folded.len() != names.len()
            || folded.iter().any(|name| {
                name.match_indices('/')
                    .any(|(index, _)| folded.contains(&name[..index]))
            })
        {
            return Err("repository contains path aliases or file/directory collisions".into());
        }
        if first.backend == "mlx"
            && (artifacts.len() < 3
                || !names.contains("config.json")
                || !names.contains("tokenizer_config.json")
                || !names.iter().any(|name| name.ends_with(".safetensors")))
        {
            return Err("incomplete MLX repository manifest".into());
        }
        let total = artifacts
            .iter()
            .try_fold(0u64, |total, artifact| total.checked_add(artifact.bytes))
            .ok_or("repository byte total overflow")?;
        if total > 9007199254740991 {
            return Err("repository byte total exceeds safe integer".into());
        }
        let coordinates: Vec<_> = first.repo.split('/').collect();
        let owner = self.root.join(&first.backend).join(coordinates[0]);
        self.cache_directory(&owner)?;
        let repository = owner.join(format!("{}@{}", coordinates[1], first.revision));
        let lock = ArtifactLock::acquire(suffix(&repository, ".repository.lock"))?;
        for artifact in artifacts {
            let acquired = self
                .acquire_inner(artifact, SizePolicy::Exact, transport, cancel)
                .await?;
            let mut directory = acquired.path;
            for _ in artifact.file.split('/') {
                directory.pop();
            }
            if root.as_ref().is_some_and(|prior| prior != &directory) {
                return Err("repository root mismatch".into());
            }
            root = Some(directory);
        }
        let root = root.ok_or("missing repository root")?;
        fn list(root: &Path, path: &Path, files: &mut HashSet<String>) -> Result<(), String> {
            for entry in std::fs::read_dir(path).map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                let kind = entry.file_type().map_err(|error| error.to_string())?;
                if kind.is_symlink() {
                    return Err("symlink in repository".into());
                }
                if kind.is_dir() {
                    list(root, &entry.path(), files)?;
                } else if kind.is_file() {
                    files.insert(
                        entry
                            .path()
                            .strip_prefix(root)
                            .map_err(|error| error.to_string())?
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                } else {
                    return Err("special file in repository".into());
                }
            }
            Ok(())
        }
        let mut actual = HashSet::new();
        list(&root, &root, &mut actual)?;
        if actual != names {
            return Err("repository content differs from manifest".into());
        }
        lock.check()?;
        Ok(root)
    }
}
