use rigspark_core::catalog::BACKENDS;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use thiserror::Error;

#[derive(Debug, Error)]
#[error("{kind}: {message}")]
pub struct StateError {
    pub kind: &'static str,
    pub message: String,
}
fn error(kind: &'static str, message: impl ToString) -> StateError {
    StateError {
        kind,
        message: message.to_string(),
    }
}
fn io(cause: std::io::Error) -> StateError {
    error("io", cause)
}
fn check(condition: bool, message: &str) -> Result<(), StateError> {
    if condition {
        Ok(())
    } else {
        Err(error("invalid", message))
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub home: PathBuf,
    pub state: PathBuf,
    pub lock: PathBuf,
    pub staging: PathBuf,
}
/// A non-blank `RIGSPARK_HOME` wins; otherwise `~/.rigspark` under the user's home.
pub fn resolve_home(
    custom: Option<&str>,
    user_home: Option<std::ffi::OsString>,
) -> Result<PathBuf, StateError> {
    match custom.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) => Ok(PathBuf::from(value)),
        None => user_home
            .map(|home| PathBuf::from(home).join(".rigspark"))
            .ok_or_else(|| error("io", "cannot determine home directory")),
    }
}
impl Config {
    pub fn from_home(home: impl AsRef<Path>) -> Result<Self, StateError> {
        let home = std::path::absolute(home).map_err(io)?;
        Ok(Self {
            state: home.join("state.json"),
            lock: home.join("lock"),
            staging: home.join(".staging"),
            home,
        })
    }
    pub fn load() -> Result<Self, StateError> {
        Self::from_home(resolve_home(
            std::env::var("RIGSPARK_HOME").ok().as_deref(),
            std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")),
        )?)
    }
    pub fn user_backend(&self) -> Result<Option<String>, StateError> {
        Ok(self.user_config()?.default_backend)
    }
    /// `config.json`: v1 names a default backend; v2 may add a default cache profile.
    pub fn user_config(&self) -> Result<UserConfig, StateError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            schema_version: u8,
            default_backend: Option<String>,
            cache: Option<crate::cache::CacheProfile>,
        }
        let raw = match secure_read(&self.home.join("config.json"), 4096, false) {
            Ok(raw) => raw,
            Err(cause) if cause.kind == "absent" => return Ok(UserConfig::default()),
            Err(cause) => return Err(cause),
        };
        if raw.trim().is_empty() {
            return Ok(UserConfig::default());
        }
        let parsed: Raw = serde_json::from_str(&raw).map_err(|cause| error("invalid", cause))?;
        check(
            match parsed.schema_version {
                1 => parsed.default_backend.is_some() && parsed.cache.is_none(),
                2 => true,
                _ => false,
            } && parsed
                .default_backend
                .as_deref()
                .is_none_or(|backend| BACKENDS.contains(&backend)),
            "invalid user config",
        )?;
        if let Some(cache) = &parsed.cache {
            cache
                .validate()
                .map_err(|cause| error("invalid", format!("invalid user config: {cause}")))?;
        }
        Ok(UserConfig {
            default_backend: parsed.default_backend,
            cache: parsed.cache,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UserConfig {
    pub default_backend: Option<String>,
    pub cache: Option<crate::cache::CacheProfile>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerState {
    pub backend: String,
    pub model_id: String,
    pub endpoint: String,
    pub port: u16,
    pub owned_by_us: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_manifest_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_started_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<crate::cache::CacheProfile>,
}
pub fn loopback(raw: &str) -> Result<url::Url, StateError> {
    let url = url::Url::parse(raw).map_err(|cause| error("invalid", cause))?;
    let local = match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    check(
        url.scheme() == "http" && local && url.username().is_empty() && url.password().is_none(),
        "endpoint must be unauthenticated loopback HTTP",
    )?;
    Ok(url)
}
fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
impl ServerState {
    pub fn validate(&self) -> Result<(), StateError> {
        check(
            BACKENDS.contains(&self.backend.as_str()) && !self.model_id.is_empty(),
            "invalid backend or model id",
        )?;
        check(
            self.port != 0 && loopback(&self.endpoint)?.port_or_known_default() == Some(self.port),
            "endpoint port mismatch",
        )?;
        check(
            self.pid != Some(0) && (!self.owned_by_us || self.pid.is_some()),
            "owned process requires positive pid",
        )?;
        check(
            self.context
                .is_none_or(|context| (1..=10000000).contains(&context)),
            "invalid context",
        )?;
        check(
            self.integrity
                .as_deref()
                .is_none_or(|value| value == "local-manifest"),
            "invalid integrity kind",
        )?;
        for sha in [&self.auth_token, &self.local_manifest_digest]
            .into_iter()
            .flatten()
        {
            check(hex(sha), "invalid digest or token")?;
        }
        for value in [
            &self.model_path,
            &self.process_executable,
            &self.process_started_at,
        ]
        .into_iter()
        .flatten()
        {
            check(!value.is_empty(), "empty process metadata")?;
        }
        if let Some(id) = &self.runtime_model_id {
            check(
                id.starts_with(|ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit())
                    && id.bytes().all(|ch| {
                        ch.is_ascii_lowercase() || ch.is_ascii_digit() || b"._:/-".contains(&ch)
                    }),
                "invalid runtime model id",
            )?;
        }
        let complete = self.pid.is_some()
            && self.process_executable.is_some()
            && self.process_started_at.is_some();
        let absent = self.pid.is_none()
            && self.process_executable.is_none()
            && self.process_started_at.is_none();
        match self.backend.as_str() {
            "mlx" => check(
                self.owned_by_us && complete && self.auth_token.is_some(),
                "MLX requires owned process identity and token",
            )?,
            "lmstudio" => check(
                !self.owned_by_us
                    && complete
                    && self.model_path.is_some()
                    && self.auth_token.is_none(),
                "LM Studio requires attached identity and model path",
            )?,
            "bitnet" => check(
                self.owned_by_us
                    && complete
                    && self.model_path.is_some()
                    && self.auth_token.is_none()
                    && self
                        .context
                        .is_some_and(|context| (1..=4096).contains(&context)),
                "BitNet requires owned identity, model path and bounded context",
            )?,
            _ => {
                check(self.auth_token.is_none(), "token only allowed for MLX")?;
                check(
                    self.owned_by_us || complete || absent,
                    "partial attached identity",
                )?;
            }
        }
        check(
            self.cache.is_none_or(|cache| {
                cache != crate::cache::CacheProfile::default()
                    && self.owned_by_us
                    && ["ollama", "llamacpp"].contains(&self.backend.as_str())
            }),
            "a cache profile is recorded only for owned Ollama or llama.cpp runtimes",
        )?;
        check(
            ["lmstudio", "bitnet"].contains(&self.backend.as_str()) || self.model_path.is_none(),
            "model path only allowed for LM Studio or BitNet",
        )
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeState {
    pub schema_version: u8,
    pub active: Option<ServerState>,
}
impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            schema_version: 2,
            active: None,
        }
    }
}
impl RuntimeState {
    /// Schema 3 is written only when a cache profile is recorded, so 1.0.x can still read other state.
    pub fn for_active(active: ServerState) -> Self {
        Self {
            schema_version: if active.cache.is_some() { 3 } else { 2 },
            active: Some(active),
        }
    }
    pub fn parse(raw: &str) -> Result<Self, StateError> {
        if raw.is_empty() {
            return Err(error("empty", "state file is empty"));
        }
        check(raw.len() <= 1048576, "state exceeds 1 MiB")?;
        let mut value: Value =
            serde_json::from_str(raw).map_err(|cause| error("unparseable", cause))?;
        check(
            value.is_object() && value.get("active").is_some(),
            "missing state fields",
        )?;
        if value["schemaVersion"] == 1 {
            value["schemaVersion"] = Value::from(2);
            if let Some(active) = value["active"].as_object_mut() {
                active.entry("backend").or_insert(Value::from("ollama"));
            }
        }
        if let Some(active) = value["active"].as_object_mut() {
            if active.get("ownedByUs") == Some(&Value::Bool(false))
                && active.get("pid") == Some(&Value::from(0))
            {
                active.remove("pid");
            }
            check(
                !active.values().any(Value::is_null),
                "null active metadata forbidden",
            )?;
        }
        let state: Self = serde_json::from_value(value).map_err(|cause| error("invalid", cause))?;
        state.validate()?;
        Ok(state)
    }
    pub fn validate(&self) -> Result<(), StateError> {
        check(
            matches!(self.schema_version, 2 | 3),
            "unsupported state schema",
        )?;
        let cached = self
            .active
            .as_ref()
            .is_some_and(|active| active.cache.is_some());
        check(
            (self.schema_version == 3) == cached,
            "state schema 3 is used exactly when a cache profile is applied",
        )?;
        if let Some(active) = &self.active {
            active.validate()?;
        }
        Ok(())
    }
}

fn nofollow_open(path: &Path) -> Result<File, StateError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            rustix::fs::OFlags::NOFOLLOW.bits() as i32 | rustix::fs::OFlags::NONBLOCK.bits() as i32,
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000);
    }
    options.open(path).map_err(|cause| {
        if cause.kind() == std::io::ErrorKind::NotFound {
            error("absent", cause)
        } else {
            io(cause)
        }
    })
}
pub fn secure_read(path: &Path, maximum: u64, secret: bool) -> Result<String, StateError> {
    let file = nofollow_open(path)?;
    let metadata = file.metadata().map_err(io)?;
    check(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "not a regular file",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        check(
            metadata.mode() & if secret { 0o077 } else { 0o022 } == 0,
            "unsafe file permissions",
        )?;
    }
    #[cfg(not(unix))]
    let _ = secret;
    check(metadata.len() <= maximum, "file exceeds byte limit")?;
    let mut bytes = Vec::new();
    file.take(maximum + 1).read_to_end(&mut bytes).map_err(io)?;
    check(bytes.len() as u64 <= maximum, "file exceeds byte limit")?;
    String::from_utf8(bytes).map_err(|cause| error("invalid", cause))
}
fn ensure_dir(path: &Path) -> Result<(), StateError> {
    match fs::symlink_metadata(path) {
        Ok(meta) => check(
            meta.is_dir() && !meta.file_type().is_symlink(),
            "unsafe state directory",
        )?,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                ensure_dir(parent)?;
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
                Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {
                    return ensure_dir(path);
                }
                Err(cause) => return Err(io(cause)),
            }
        }
        Err(cause) => return Err(io(cause)),
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<(), StateError> {
    ensure_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io)?;
    }
    Ok(())
}

struct MutationGuard {
    path: PathBuf,
}
impl MutationGuard {
    fn acquire(lock: &Path, timeout: Duration) -> Result<Self, StateError> {
        let mut guard_path = lock.as_os_str().to_owned();
        guard_path.push(".guard");
        let path = PathBuf::from(guard_path);
        let deadline = Instant::now() + timeout;
        loop {
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let mut builder = builder;
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(cause) => return Err(io(cause)),
            }
            if Instant::now() >= deadline {
                return Err(error(
                    "locked",
                    "lock guard busy or abandoned; explicit recovery required",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for MutationGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}
pub struct LockGuard {
    path: PathBuf,
    identity: same_file::Handle,
    released: bool,
}
impl LockGuard {
    pub(crate) fn assert_for(&self, path: &Path) -> Result<(), StateError> {
        self.assert_owned()?;
        let expected = same_file::Handle::from_file(nofollow_open(path)?).map_err(io)?;
        if expected != self.identity {
            return Err(error("locked", "lock belongs to another store"));
        }
        Ok(())
    }
    pub fn assert_owned(&self) -> Result<(), StateError> {
        let file = nofollow_open(&self.path)?;
        check(file.metadata().map_err(io)?.is_file(), "invalid lock type")?;
        let current = same_file::Handle::from_file(file).map_err(io)?;
        if current != self.identity {
            return Err(error("locked", "lock ownership changed"));
        }
        Ok(())
    }
    pub fn release(mut self) -> Result<(), StateError> {
        let result = self.release_inner();
        self.released = true;
        result
    }
    fn release_inner(&self) -> Result<(), StateError> {
        let _guard = MutationGuard::acquire(&self.path, Duration::from_secs(10))?;
        self.assert_owned()?;
        fs::remove_file(&self.path).map_err(io)
    }
}
impl Drop for LockGuard {
    fn drop(&mut self) {
        if !self.released {
            let _ = self.release_inner();
        }
    }
}

pub struct StateStore {
    pub config: Config,
}
impl StateStore {
    pub fn new(config: Config) -> Self {
        Self { config }
    }
    pub fn read(&self) -> Result<RuntimeState, StateError> {
        match secure_read(&self.config.state, 1048576, true) {
            Ok(raw) => RuntimeState::parse(&raw),
            // Windows reports "path not found" when home is a file, so absence alone is not proof of empty state.
            Err(cause)
                if cause.kind == "absent"
                    && std::fs::symlink_metadata(&self.config.home)
                        .map_or(true, |meta| meta.is_dir()) =>
            {
                Ok(RuntimeState::default())
            }
            Err(cause) if cause.kind == "absent" => {
                Err(error("invalid", "state home is not a directory"))
            }
            Err(cause) => Err(cause),
        }
    }
    pub fn lock(&self, timeout: Duration) -> Result<LockGuard, StateError> {
        self.lock_with(timeout, process_alive)
    }
    pub fn lock_with(
        &self,
        timeout: Duration,
        alive: impl Fn(u32) -> bool,
    ) -> Result<LockGuard, StateError> {
        private_dir(&self.config.home)?;
        let deadline = Instant::now() + timeout;
        loop {
            {
                let _guard = MutationGuard::acquire(
                    &self.config.lock,
                    deadline.saturating_duration_since(Instant::now()),
                )?;
                let mut options = OpenOptions::new();
                options.write(true).read(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&self.config.lock) {
                    Ok(mut file) => {
                        if let Err(cause) =
                            writeln!(file, "{}", std::process::id()).and_then(|()| file.sync_all())
                        {
                            let _ = fs::remove_file(&self.config.lock);
                            return Err(io(cause));
                        }
                        return Ok(LockGuard {
                            path: self.config.lock.clone(),
                            identity: same_file::Handle::from_file(file).map_err(io)?,
                            released: false,
                        });
                    }
                    Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {
                        if holder_pid(&self.config.lock).is_some_and(|pid| !alive(pid)) {
                            fs::remove_file(&self.config.lock).map_err(io)?;
                            continue;
                        }
                    }
                    Err(cause) => return Err(io(cause)),
                }
            }
            if Instant::now() >= deadline {
                return Err(error(
                    "locked",
                    "state lock busy or stale; explicit recovery required",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    pub fn write(&self, guard: &LockGuard, state: &RuntimeState) -> Result<(), StateError> {
        state.validate()?;
        check(
            guard.path == self.config.lock,
            "guard belongs to another store",
        )?;
        let _mutation = MutationGuard::acquire(&self.config.lock, Duration::from_secs(10))?;
        guard.assert_owned()?;
        private_dir(&self.config.staging)?;
        let mut temp = tempfile::NamedTempFile::new_in(&self.config.staging).map_err(io)?;
        serde_json::to_writer_pretty(&mut temp, state).map_err(|cause| error("io", cause))?;
        temp.write_all(b"\n").map_err(io)?;
        temp.as_file().sync_all().map_err(io)?;
        temp.persist(&self.config.state)
            .map_err(|cause| io(cause.error))?;
        #[cfg(unix)]
        File::open(&self.config.home)
            .and_then(|file| file.sync_all())
            .map_err(io)?;
        Ok(())
    }
    pub fn compare_and_write(
        &self,
        guard: &LockGuard,
        expected: &RuntimeState,
        next: &RuntimeState,
    ) -> Result<(), StateError> {
        check(self.read()? == *expected, "state changed since preparation")?;
        self.write(guard, next)
    }
}

fn holder_pid(path: &Path) -> Option<u32> {
    let raw = secure_read(path, 64, false).ok()?;
    let raw = raw.trim();
    if raw.starts_with('0') || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    raw.parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0 && *pid <= i32::MAX as u32)
}

pub(crate) fn process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let Some(pid) = i32::try_from(pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        else {
            return true;
        };
        !matches!(
            rustix::process::test_kill_process(pid),
            Err(rustix::io::Errno::SRCH)
        )
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}
