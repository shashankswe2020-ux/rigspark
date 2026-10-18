use rigspark_runtime::{
    identity::{Listener, ProcessIdentity, ProcessProbe},
    process_control::{ChildProcess, ProcessControl, SpawnSpec, stop_owned, wait_owned},
    state::StateError,
};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
fn identity() -> ProcessIdentity {
    ProcessIdentity {
        pid: 123,
        process: "runtime".into(),
        executable: "/opt/runtime".into(),
        started: "instance".into(),
    }
}
struct Probe;

#[test]
fn bitnet_server_requires_an_explicit_absolute_executable_file() {
    use rigspark_runtime::process_control::bitnet_server_path;
    assert!(
        bitnet_server_path(None)
            .unwrap_err()
            .contains("RIGSPARK_BITNET_SERVER")
    );
    assert!(bitnet_server_path(Some(std::ffi::OsStr::new("llama-server"))).is_err());
    let root = tempfile::tempdir().unwrap();
    assert!(bitnet_server_path(Some(root.path().as_os_str())).is_err());
    let binary = root.path().join("llama-server");
    assert!(bitnet_server_path(Some(binary.as_os_str())).is_err());
    std::fs::write(&binary, b"not executed in tests").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(bitnet_server_path(Some(binary.as_os_str())).is_err());
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    assert_eq!(
        bitnet_server_path(Some(binary.as_os_str())).unwrap(),
        binary.canonicalize().unwrap()
    );
}
#[async_trait::async_trait]
impl ProcessProbe for Probe {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        Ok(Listener {
            identity: identity(),
            port,
            address: host.into(),
        })
    }
    async fn process(&self, _pid: u32) -> Result<ProcessIdentity, StateError> {
        Ok(identity())
    }
}
struct Child {
    terminated: Arc<Mutex<bool>>,
}
#[async_trait::async_trait]
impl ChildProcess for Child {
    fn pid(&self) -> u32 {
        123
    }
    fn exited(&mut self) -> Result<bool, String> {
        Ok(false)
    }
    async fn terminate(&mut self) -> Result<(), String> {
        *self.terminated.lock().unwrap() = true;
        Ok(())
    }
    fn detach(&mut self) {}
}
struct Control {
    signals: Mutex<Vec<u32>>,
}
#[async_trait::async_trait]
impl ProcessControl for Control {
    async fn occupied(&self, _endpoint: &str) -> Result<bool, String> {
        Ok(false)
    }
    async fn spawn(&self, _spec: &SpawnSpec) -> Result<Box<dyn ChildProcess>, String> {
        unreachable!()
    }
    async fn signal(&self, process: &ProcessIdentity, _force: bool) -> Result<(), String> {
        self.signals.lock().unwrap().push(process.pid);
        Ok(())
    }
    async fn alive(&self, _pid: u32) -> Result<bool, String> {
        Ok(false)
    }
}
#[tokio::test]
async fn failed_readiness_terminates_child_and_success_preserves_it() {
    for ready in [true, false] {
        let terminated = Arc::new(Mutex::new(false));
        let child: Box<dyn ChildProcess> = Box::new(Child {
            terminated: terminated.clone(),
        });
        let result = wait_owned(
            child,
            "http://127.0.0.1:11435",
            "/opt/runtime",
            &Probe,
            &CancellationToken::new(),
            || async move {
                if ready {
                    Ok(())
                } else {
                    Err("failed readiness".into())
                }
            },
        )
        .await;
        assert_eq!(result.is_ok(), ready);
        assert_eq!(*terminated.lock().unwrap(), !ready);
    }
}
#[tokio::test]
async fn stop_refuses_reused_pid_before_signalling() {
    let control = Control {
        signals: Mutex::new(Vec::new()),
    };
    let mut changed = identity();
    changed.started = "old instance".into();
    assert!(
        stop_owned(
            "http://127.0.0.1:11435",
            &changed,
            &Probe,
            &control,
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert!(control.signals.lock().unwrap().is_empty());
    stop_owned(
        "http://127.0.0.1:11435",
        &identity(),
        &Probe,
        &control,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(*control.signals.lock().unwrap(), vec![123]);
}

#[tokio::test]
async fn startup_uses_backend_deadline_and_cleans_up_on_timeout() {
    let terminated = Arc::new(Mutex::new(false));
    let child: Box<dyn ChildProcess> = Box::new(Child {
        terminated: terminated.clone(),
    });
    let result = rigspark_runtime::process_control::wait_owned_with_timeout(
        child,
        "http://127.0.0.1:11435",
        "/opt/runtime",
        &Probe,
        &CancellationToken::new(),
        std::time::Duration::from_millis(5),
        std::future::pending::<Result<(), String>>,
    )
    .await;
    assert!(result.unwrap_err().contains("timed out"));
    assert!(*terminated.lock().unwrap());
}

struct ExitingProbe(std::sync::atomic::AtomicUsize);
#[async_trait::async_trait]
impl ProcessProbe for ExitingProbe {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        Probe.listener(port, host).await
    }
    async fn process(&self, _: u32) -> Result<ProcessIdentity, StateError> {
        if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            return Ok(identity());
        }
        Err(StateError {
            kind: "identity",
            message: "process disappeared".into(),
        })
    }
}
struct ExitRaceControl {
    probes: std::sync::atomic::AtomicUsize,
    remains_alive: bool,
    signals: Mutex<Vec<bool>>,
}
#[async_trait::async_trait]
impl ProcessControl for ExitRaceControl {
    async fn occupied(&self, _: &str) -> Result<bool, String> {
        Ok(false)
    }
    async fn spawn(&self, _: &SpawnSpec) -> Result<Box<dyn ChildProcess>, String> {
        unreachable!()
    }
    async fn signal(&self, _: &ProcessIdentity, force: bool) -> Result<(), String> {
        self.signals.lock().unwrap().push(force);
        Ok(())
    }
    async fn alive(&self, _: u32) -> Result<bool, String> {
        Ok(self
            .probes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 0
            || self.remains_alive)
    }
}
#[tokio::test]
async fn exit_between_liveness_and_identity_probe_is_not_a_shutdown_failure() {
    for remains_alive in [false, true] {
        let control = ExitRaceControl {
            probes: Default::default(),
            remains_alive,
            signals: Mutex::new(Vec::new()),
        };
        let result = stop_owned(
            "http://127.0.0.1:11435",
            &identity(),
            &ExitingProbe(Default::default()),
            &control,
            &CancellationToken::new(),
        )
        .await;
        assert_eq!(result.is_ok(), !remains_alive);
        assert_eq!(*control.signals.lock().unwrap(), vec![false]);
    }
}
