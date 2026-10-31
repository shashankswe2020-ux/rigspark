use crate::identity::{Listener, NativeProcessProbe, ProcessIdentity, ProcessProbe};
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub struct SpawnSpec {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}
#[async_trait::async_trait]
pub trait ChildProcess: Send {
    fn pid(&self) -> u32;
    fn exited(&mut self) -> Result<bool, String>;
    async fn terminate(&mut self) -> Result<(), String>;
    fn detach(&mut self);
}
#[async_trait::async_trait]
pub trait ProcessControl: Send + Sync {
    async fn occupied(&self, endpoint: &str) -> Result<bool, String>;
    async fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn ChildProcess>, String>;
    async fn signal(&self, process: &ProcessIdentity, force: bool) -> Result<(), String>;
    async fn alive(&self, pid: u32) -> Result<bool, String>;
}
struct NativeChild {
    child: tokio::process::Child,
    detached: bool,
    pid: u32,
}
#[async_trait::async_trait]
impl ChildProcess for NativeChild {
    fn pid(&self) -> u32 {
        self.pid
    }
    fn exited(&mut self) -> Result<bool, String> {
        self.child
            .try_wait()
            .map(|value| value.is_some())
            .map_err(|_| "cannot observe child exit".into())
    }
    async fn terminate(&mut self) -> Result<(), String> {
        if self.exited()? {
            return Ok(());
        }
        self.child
            .start_kill()
            .map_err(|_| "child termination failed")?;
        tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .map_err(|_| "child termination timed out")?
            .map_err(|_| "child wait failed")?;
        Ok(())
    }
    fn detach(&mut self) {
        self.detached = true;
    }
}
impl Drop for NativeChild {
    fn drop(&mut self) {
        if !self.detached {
            let _ = self.child.start_kill();
        }
    }
}
pub struct NativeProcessControl;
#[async_trait::async_trait]
impl ProcessControl for NativeProcessControl {
    async fn occupied(&self, endpoint: &str) -> Result<bool, String> {
        let url = crate::state::loopback(endpoint).map_err(|error| error.to_string())?;
        let host = url
            .host_str()
            .ok_or("missing host")?
            .trim_matches(['[', ']']);
        let port = url.port_or_known_default().ok_or("missing port")?;
        let addresses = if host == "localhost" {
            vec![format!("127.0.0.1:{port}"), format!("[::1]:{port}")]
        } else {
            vec![format!(
                "{}:{port}",
                if host.contains(':') {
                    format!("[{host}]")
                } else {
                    host.into()
                }
            )]
        };
        for address in addresses {
            match tokio::time::timeout(
                Duration::from_secs(2),
                tokio::net::TcpStream::connect(address),
            )
            .await
            {
                Ok(Ok(_)) => return Ok(true),
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => (),
                _ => return Err("cannot establish port availability".into()),
            }
        }
        Ok(false)
    }
    async fn spawn(&self, spec: &SpawnSpec) -> Result<Box<dyn ChildProcess>, String> {
        if !spec.binary.is_absolute() || spec.args.iter().any(|arg| arg.contains('\0')) {
            return Err("invalid spawn specification".into());
        }
        let mut command = tokio::process::Command::new(&spec.binary);
        command
            .args(&spec.args)
            .env_clear()
            .envs(&spec.env)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| "runtime spawn failed")?;
        let Some(pid) = child.id().filter(|pid| *pid > 0) else {
            let _ = child.kill().await;
            return Err("runtime reported invalid PID".into());
        };
        Ok(Box::new(NativeChild {
            child,
            detached: false,
            pid,
        }))
    }
    async fn signal(&self, expected: &ProcessIdentity, force: bool) -> Result<(), String> {
        let observed = NativeProcessProbe
            .process(expected.pid)
            .await
            .map_err(|_| "cannot verify process before signal")?;
        if !same_process(&observed, expected) {
            return Err("process changed before signal".into());
        }
        #[cfg(unix)]
        {
            let pid = i32::try_from(expected.pid)
                .ok()
                .and_then(rustix::process::Pid::from_raw)
                .ok_or("invalid process PID")?;
            rustix::process::kill_process(
                pid,
                if force {
                    rustix::process::Signal::KILL
                } else {
                    rustix::process::Signal::TERM
                },
            )
            .map_err(|_| "signal failed".to_owned())
        }
        #[cfg(windows)]
        {
            let _ = force;
            let system = sysinfo::System::new_all();
            let process = system
                .process(sysinfo::Pid::from_u32(expected.pid))
                .ok_or("process unavailable")?;
            if process.kill() {
                Ok(())
            } else {
                Err("process termination failed".into())
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = force;
            Err("unsupported platform".into())
        }
    }
    async fn alive(&self, pid: u32) -> Result<bool, String> {
        #[cfg(unix)]
        {
            let pid = i32::try_from(pid)
                .ok()
                .and_then(rustix::process::Pid::from_raw)
                .ok_or("invalid process PID")?;
            match rustix::process::test_kill_process(pid) {
                Ok(()) => Ok(true),
                Err(rustix::io::Errno::SRCH) => Ok(false),
                Err(rustix::io::Errno::PERM) => Ok(true),
                Err(_) => Err("process existence unavailable".into()),
            }
        }
        #[cfg(not(unix))]
        {
            let system = sysinfo::System::new_all();
            Ok(system.process(sysinfo::Pid::from_u32(pid)).is_some())
        }
    }
}
pub fn same_process(actual: &ProcessIdentity, expected: &ProcessIdentity) -> bool {
    actual.pid == expected.pid
        && actual.executable == expected.executable
        && actual.started == expected.started
}
/// Package-manager directories searched after `PATH` once
/// [`enable_package_manager_fallback`] is called: apps opened from Finder or a launcher
/// inherit only the system `PATH` and would otherwise miss Homebrew runtimes.
#[cfg(target_os = "macos")]
pub const FALLBACK_BINARY_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];
#[cfg(target_os = "linux")]
pub const FALLBACK_BINARY_DIRS: &[&str] = &["/usr/local/bin", "/home/linuxbrew/.linuxbrew/bin"];
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub const FALLBACK_BINARY_DIRS: &[&str] = &[];

static PACKAGE_MANAGER_FALLBACK: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Lets a GUI-launched app, whose `PATH` is the system default, also find runtimes in
/// [`FALLBACK_BINARY_DIRS`]. Command-line use keeps the caller's `PATH` exactly.
pub fn enable_package_manager_fallback() {
    PACKAGE_MANAGER_FALLBACK.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Runtimes an app may ship inside its own bundle; nothing else resolves from there.
pub const BUNDLED_RUNTIMES: &[&str] = &["llama-server"];

static BUNDLED_RUNTIME_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Registers the directory holding an app's bundled [`BUNDLED_RUNTIMES`]. They are
/// searched last, so a runtime the user installed still wins.
pub fn register_bundled_runtime_dir(dir: PathBuf) -> Result<(), String> {
    if !dir.is_absolute() {
        return Err("bundled runtime directory must be absolute".into());
    }
    BUNDLED_RUNTIME_DIR
        .set(dir)
        .map_err(|_| "bundled runtime directory is already registered".into())
}

/// Whether `binary` resolves to the copy shipped in the registered app bundle.
pub fn resolves_to_bundled(binary: &str) -> bool {
    let (Some(dir), Ok(path)) = (BUNDLED_RUNTIME_DIR.get(), resolve_binary(binary)) else {
        return false;
    };
    dir.canonicalize().is_ok_and(|dir| path.starts_with(dir))
}

/// Directories searched after `PATH` for `binary`, in order.
pub fn fallback_directories<'a>(
    binary: &str,
    package_manager: &[&'a Path],
    bundled: Option<&'a Path>,
) -> Vec<&'a Path> {
    let mut dirs = package_manager.to_vec();
    if BUNDLED_RUNTIMES.contains(&binary)
        && let Some(dir) = bundled.filter(|dir| dir.is_absolute())
    {
        dirs.push(dir);
    }
    dirs
}

pub fn resolve_binary(binary: &str) -> Result<PathBuf, String> {
    let package_manager: Vec<&Path> =
        if PACKAGE_MANAGER_FALLBACK.load(std::sync::atomic::Ordering::Relaxed) {
            FALLBACK_BINARY_DIRS.iter().map(Path::new).collect()
        } else {
            Vec::new()
        };
    let fallbacks = fallback_directories(
        binary,
        &package_manager,
        BUNDLED_RUNTIME_DIR.get().map(PathBuf::as_path),
    );
    resolve_binary_in(
        binary,
        &std::env::var_os("PATH").unwrap_or_default(),
        &fallbacks,
    )
}

pub fn resolve_binary_in(
    binary: &str,
    path_var: &std::ffi::OsStr,
    fallbacks: &[&Path],
) -> Result<PathBuf, String> {
    let path = Path::new(binary);
    if path.is_absolute() {
        return path
            .canonicalize()
            .map_err(|_| "runtime executable unavailable".into());
    }

    let fixed = fallbacks.iter().map(|dir| dir.to_path_buf());
    for directory in std::env::split_paths(path_var).chain(fixed) {
        if directory.as_os_str().is_empty() || !directory.is_absolute() {
            continue;
        }
        for name in [binary.to_owned(), format!("{binary}.exe")] {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return candidate
                    .canonicalize()
                    .map_err(|_| "runtime executable cannot be resolved".into());
            }
        }
    }
    Err("runtime executable not found on PATH".into())
}
pub fn bitnet_server_path(value: Option<&std::ffi::OsStr>) -> Result<PathBuf, String> {
    let invalid = || {
        "Set RIGSPARK_BITNET_SERVER to the absolute path of a compiled BitNet llama-server executable (not stock llama.cpp)".to_owned()
    };
    let path = Path::new(value.ok_or_else(invalid)?);
    if !path.is_absolute()
        || path
            .to_str()
            .is_none_or(|text| text.chars().any(char::is_control))
        || !path.is_file()
    {
        return Err(invalid());
    }
    let path = path.canonicalize().map_err(|_| invalid())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.metadata().map_err(|_| invalid())?.permissions().mode() & 0o111 == 0 {
            return Err(invalid());
        }
    }
    Ok(path)
}

pub fn resolve_bitnet_server() -> Result<PathBuf, String> {
    bitnet_server_path(std::env::var_os("RIGSPARK_BITNET_SERVER").as_deref())
}

pub fn minimal_env() -> BTreeMap<String, String> {
    [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TMPDIR",
        "TMP",
        "TEMP",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
    ]
    .into_iter()
    .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
    .collect()
}
pub async fn listener(
    probe: &dyn ProcessProbe,
    endpoint: &str,
    cancel: &CancellationToken,
) -> Result<Listener, String> {
    let url = crate::state::loopback(endpoint).map_err(|error| error.to_string())?;
    let host = url.host_str().ok_or("missing host")?;
    let port = url.port_or_known_default().ok_or("missing port")?;
    tokio::select! {biased;_=cancel.cancelled()=>Err("cancelled".into()),result=tokio::time::timeout(Duration::from_secs(5),probe.listener(port,host))=>{
        let observed=result.map_err(|_|"listener probe timed out")?.map_err(|_|"listener identity unavailable")?;
        crate::identity::choose_listener(std::slice::from_ref(&observed),port,host).map_err(|_|"listener address mismatch".into())
    }}
}
pub async fn wait_owned<F, Fut>(
    child: Box<dyn ChildProcess>,
    endpoint: &str,
    executable: &str,
    probe: &dyn ProcessProbe,
    cancel: &CancellationToken,
    ready: F,
) -> Result<Listener, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    wait_owned_with_timeout(
        child,
        endpoint,
        executable,
        probe,
        cancel,
        Duration::from_secs(30),
        ready,
    )
    .await
}
pub async fn wait_owned_with_timeout<F, Fut>(
    mut child: Box<dyn ChildProcess>,
    endpoint: &str,
    executable: &str,
    probe: &dyn ProcessProbe,
    cancel: &CancellationToken,
    timeout: Duration,
    ready: F,
) -> Result<Listener, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), String>>,
{
    if timeout.is_zero() || timeout > Duration::from_secs(300) {
        child.terminate().await?;
        return Err("invalid readiness deadline".into());
    }
    let operation = async {
        if child.pid() == 0 {
            return Err("invalid child PID".into());
        }
        let readiness = ready();
        tokio::pin!(readiness);
        loop {
            if child.exited()? {
                return Err("runtime exited before readiness".into());
            }
            tokio::select! {result=&mut readiness=>{result?;break;},_=tokio::time::sleep(Duration::from_millis(50))=>()}
        }
        let observed = listener(probe, endpoint, cancel).await?;
        if observed.identity.pid != child.pid() || observed.identity.executable != executable {
            return Err("readiness belongs to another process".into());
        }
        Ok(observed)
    };
    let result = tokio::select! {biased;_=cancel.cancelled()=>Err("cancelled".into()),result=tokio::time::timeout(timeout,operation)=>result.map_err(|_|"runtime readiness timed out".to_owned()).and_then(|result|result)};
    match result {
        Ok(observed) => {
            child.detach();
            Ok(observed)
        }
        Err(error) => {
            child
                .terminate()
                .await
                .map_err(|cleanup| format!("{error}; {cleanup}"))?;
            Err(error)
        }
    }
}
pub async fn stop_owned(
    endpoint: &str,
    expected: &ProcessIdentity,
    probe: &dyn ProcessProbe,
    control: &dyn ProcessControl,
    cancel: &CancellationToken,
) -> Result<(), String> {
    let observed = listener(probe, endpoint, cancel).await?;
    if !same_process(&observed.identity, expected) {
        return Err("listener changed; refusing to stop".into());
    }
    for force in [false, true] {
        let current = tokio::select! {biased;_=cancel.cancelled()=>return Err("cancelled".into()),result=tokio::time::timeout(Duration::from_secs(5),probe.process(expected.pid))=>result.map_err(|_|"process probe timed out")?.map_err(|_|"process identity unavailable")?};
        if !same_process(&current, expected) {
            if force {
                return Ok(());
            }
            return Err("process changed; refusing to stop".into());
        }
        control.signal(expected, force).await?;
        for _ in 0..10 {
            if !control.alive(expected.pid).await? {
                return Ok(());
            }
            let current = match probe.process(expected.pid).await {
                Ok(current) => current,
                Err(_) => {
                    if !control.alive(expected.pid).await? {
                        return Ok(());
                    }
                    return Err("process identity unavailable after signal".into());
                }
            };
            if !same_process(&current, expected) {
                return Ok(());
            }
            tokio::select! {biased;_=cancel.cancelled()=>return Err("cancelled".into()),_=tokio::time::sleep(Duration::from_millis(50))=>()}
        }
    }
    Err("runtime remains alive after termination".into())
}
