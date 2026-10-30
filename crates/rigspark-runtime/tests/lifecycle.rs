use rigspark_runtime::{
    adapters::{BackendAdapter, BackendError, ServeRequest},
    identity::{Listener, ProcessIdentity, ProcessProbe},
    lifecycle::{Lifecycle, Registry},
    state::{Config, RuntimeState, ServerState, StateError, StateStore},
};
use std::{sync::Mutex, time::Duration};
use tokio_util::sync::CancellationToken;
fn state() -> RuntimeState {
    RuntimeState::parse(r#"{"schemaVersion":2,"active":{"backend":"ollama","modelId":"prior:latest","endpoint":"http://127.0.0.1:11435","port":11435,"ownedByUs":false,"pid":123,"processExecutable":"/trusted/ollama","processStartedAt":"instance"}}"#).unwrap()
}
struct Probe;
#[async_trait::async_trait]
impl ProcessProbe for Probe {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        Ok(Listener {
            identity: ProcessIdentity {
                pid: 123,
                process: "ollama".into(),
                executable: "/trusted/ollama".into(),
                started: "instance".into(),
            },
            port,
            address: host.into(),
        })
    }
    async fn process(&self, _pid: u32) -> Result<ProcessIdentity, StateError> {
        Ok(self.listener(11435, "127.0.0.1").await?.identity)
    }
}
struct Adapter {
    fail: bool,
    stopped: Mutex<bool>,
}

struct SwitchAdapter<'store> {
    store: &'store StateStore,
    fault: &'static str,
    backend: &'static str,
    calls: Mutex<usize>,
}

#[async_trait::async_trait]
impl BackendAdapter for SwitchAdapter<'_> {
    fn name(&self) -> &'static str {
        self.backend
    }
    fn trusts(&self, identity: &ProcessIdentity) -> bool {
        identity.executable == "/trusted/ollama"
    }
    async fn serve(
        &self,
        _: &ServeRequest,
        _: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        panic!("pointer switch must not start a daemon")
    }
    async fn stop(&self, _: &ServerState, _: &CancellationToken) -> Result<(), BackendError> {
        panic!("pointer switch must not stop the daemon")
    }
    async fn ready(
        &self,
        request: &ServeRequest,
        _: &CancellationToken,
    ) -> Result<(), BackendError> {
        *self.calls.lock().unwrap() += 1;
        assert_eq!(request.model_id, "next:latest");
        assert_eq!(request.endpoint, "http://127.0.0.1:11435");
        assert!(self.store.config.lock.exists());
        if self.fault == "readiness" {
            return Err(BackendError("not ready".into()));
        }
        if ["changed", "disappeared"].contains(&self.fault) {
            let mut changed = state();
            if self.fault == "changed" {
                changed.active.as_mut().unwrap().model_id = "concurrent:latest".into();
            } else {
                changed.active = None;
            }
            std::fs::write(
                &self.store.config.state,
                serde_json::to_vec(&changed).unwrap(),
            )
            .unwrap();
        }
        Ok(())
    }
}

#[tokio::test]
async fn pointer_switch_preserves_daemon_clears_context_and_rejects_readiness_drift() {
    for fault in ["none", "readiness", "changed", "disappeared"] {
        let home = tempfile::tempdir().unwrap();
        let store = StateStore::new(Config::from_home(home.path()).unwrap());
        let mut prior = state();
        let active = prior.active.as_mut().unwrap();
        active.runtime_model_id = Some("llmup-context-old:8192".into());
        active.context = Some(8192);
        active.integrity = Some("local-manifest".into());
        active.local_manifest_digest = Some("a".repeat(64));
        let guard = store.lock(Duration::from_secs(1)).unwrap();
        store.write(&guard, &prior).unwrap();
        guard.release().unwrap();
        let before = std::fs::read(&store.config.state).unwrap();
        let adapter = SwitchAdapter {
            store: &store,
            fault,
            backend: "ollama",
            calls: Mutex::new(0),
        };
        let registry = Registry::new(vec![&adapter]);
        let lifecycle = Lifecycle {
            store: &store,
            registry: &registry,
            probe: &Probe,
        };
        let cancel = CancellationToken::new();
        let reviewed = lifecycle.review("next:latest", &cancel).await.unwrap();
        let result = lifecycle
            .switch_pointer("next:latest", &reviewed, &cancel)
            .await;
        assert_eq!(*adapter.calls.lock().unwrap(), 1);
        assert!(!store.config.lock.exists());
        match fault {
            "none" => {
                let switched = result.unwrap();
                let mut expected = prior.active.unwrap();
                expected.model_id = "next:latest".into();
                expected.runtime_model_id = None;
                expected.context = None;
                expected.integrity = None;
                expected.local_manifest_digest = None;
                assert_eq!(switched, expected);
                assert_eq!(store.read().unwrap().active, Some(expected));
            }
            "readiness" => {
                assert!(result.unwrap_err().to_string().contains("not ready"));
                assert_eq!(std::fs::read(&store.config.state).unwrap(), before);
            }
            "changed" => {
                assert!(result.is_err());
                assert_eq!(
                    store.read().unwrap().active.unwrap().model_id,
                    "concurrent:latest"
                );
            }
            _ => {
                assert!(result.is_err());
                assert!(store.read().unwrap().active.is_none());
            }
        }
    }
}

#[tokio::test]
async fn single_model_switch_rejects_without_readiness_or_state_change() {
    for backend in ["llamacpp", "mlx", "lmstudio"] {
        let home = tempfile::tempdir().unwrap();
        let store = StateStore::new(Config::from_home(home.path()).unwrap());
        let mut prior = state();
        let active = prior.active.as_mut().unwrap();
        active.backend = backend.into();
        active.owned_by_us = backend == "mlx";
        active.auth_token = (backend == "mlx").then(|| "a".repeat(64));
        active.model_path = (backend == "lmstudio").then(|| "fixture/model.gguf".into());
        let guard = store.lock(Duration::from_secs(1)).unwrap();
        store.write(&guard, &prior).unwrap();
        guard.release().unwrap();
        let bytes = std::fs::read(&store.config.state).unwrap();
        let adapter = SwitchAdapter {
            store: &store,
            backend,
            fault: "none",
            calls: Mutex::new(0),
        };
        let registry = Registry::new(vec![&adapter]);
        let lifecycle = Lifecycle {
            store: &store,
            registry: &registry,
            probe: &Probe,
        };
        let cancel = CancellationToken::new();
        let reviewed = lifecycle.review("next:latest", &cancel).await.unwrap();
        assert_eq!(
            lifecycle
                .switch_pointer("next:latest", &reviewed, &cancel)
                .await
                .unwrap_err()
                .to_string(),
            "single-model or delegated runtime requires up"
        );
        assert_eq!(*adapter.calls.lock().unwrap(), 0);
        assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
        assert!(!store.config.lock.exists());
    }
}

#[tokio::test]
async fn health_checks_identity_and_readiness_without_mutating_state() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &state()).unwrap();
    guard.release().unwrap();
    let adapter = Adapter {
        fail: false,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    let active = state().active.unwrap();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: active.model_id.clone(),
        endpoint: active.endpoint.clone(),
        model_path: None,
        context: None,
        cache: None,
    };
    lifecycle
        .health(&active, &request, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(store.read().unwrap(), state());
    assert!(!store.config.lock.exists());
    let mut changed = active;
    changed.pid = Some(456);
    assert!(
        lifecycle
            .health(&changed, &request, &CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(store.read().unwrap(), state());
    assert!(!store.config.lock.exists());
    assert!(!*adapter.stopped.lock().unwrap());
    let failing = Adapter {
        fail: true,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&failing]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    assert_eq!(
        lifecycle
            .health(
                &state().active.unwrap(),
                &request,
                &CancellationToken::new()
            )
            .await
            .unwrap_err()
            .0,
        "readiness failed"
    );
    assert_eq!(store.read().unwrap(), state());
    assert!(!store.config.lock.exists());
    assert!(!*failing.stopped.lock().unwrap());
}
#[async_trait::async_trait]
impl BackendAdapter for Adapter {
    async fn attach_only(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        self.serve(request, cancel).await
    }
    fn name(&self) -> &'static str {
        "ollama"
    }
    fn trusts(&self, identity: &ProcessIdentity) -> bool {
        identity.executable == "/trusted/ollama"
    }
    async fn serve(
        &self,
        request: &ServeRequest,
        _cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        if self.fail {
            return Err(BackendError("startup failed".into()));
        }
        let mut active = state().active.unwrap();
        active.model_id = request.model_id.clone();
        Ok(active)
    }
    async fn ready(
        &self,
        _request: &ServeRequest,
        _cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        if self.fail {
            return Err(BackendError("readiness failed".into()));
        }
        Ok(())
    }
    async fn stop(
        &self,
        handle: &ServerState,
        _cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        if handle.process_executable.is_none() || handle.process_started_at.is_none() {
            return Err(BackendError("missing observed identity".into()));
        }
        *self.stopped.lock().unwrap() = true;
        Ok(())
    }
}

#[tokio::test]
async fn legacy_owned_state_is_enriched_from_verified_live_identity_before_stop() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let mut prior = state();
    let active = prior.active.as_mut().unwrap();
    active.owned_by_us = true;
    active.process_executable = None;
    active.process_started_at = None;
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &prior).unwrap();
    guard.release().unwrap();
    let adapter = Adapter {
        fail: false,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    lifecycle.down(&CancellationToken::new()).await.unwrap();
    assert!(*adapter.stopped.lock().unwrap());
    assert!(store.read().unwrap().active.is_none());
}

struct RestartedProbe;
#[async_trait::async_trait]
impl ProcessProbe for RestartedProbe {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        Ok(Listener {
            identity: ProcessIdentity {
                pid: 456,
                process: "ollama".into(),
                executable: "/trusted/ollama".into(),
                started: "restarted".into(),
            },
            port,
            address: host.into(),
        })
    }
    async fn process(&self, _pid: u32) -> Result<ProcessIdentity, StateError> {
        Ok(self.listener(11435, "127.0.0.1").await?.identity)
    }
}

#[tokio::test]
async fn restarted_daemon_detaches_stale_attached_pointer_but_never_stops_owned_process() {
    for owned in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let store = StateStore::new(Config::from_home(home.path()).unwrap());
        let mut prior = state();
        prior.active.as_mut().unwrap().owned_by_us = owned;
        let guard = store.lock(Duration::from_millis(10)).unwrap();
        store.write(&guard, &prior).unwrap();
        guard.release().unwrap();
        let bytes = std::fs::read(&store.config.state).unwrap();
        let adapter = Adapter {
            fail: false,
            stopped: Mutex::new(false),
        };
        let registry = Registry::new(vec![&adapter]);
        let lifecycle = Lifecycle {
            store: &store,
            registry: &registry,
            probe: &RestartedProbe,
        };
        let refused = lifecycle
            .down(&CancellationToken::new())
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("does not match"), "{refused}");
        assert_eq!(refused.contains("llmup down --forget"), !owned, "{refused}");
        assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
        let result = lifecycle.forget_attached(&CancellationToken::new()).await;
        assert!(!*adapter.stopped.lock().unwrap(), "owned={owned}");
        assert!(!store.config.lock.exists());
        if owned {
            assert!(result.unwrap_err().to_string().contains("owned"));
            assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
        } else {
            assert_eq!(result.unwrap(), prior.active);
            assert!(store.read().unwrap().active.is_none());
            assert_eq!(
                lifecycle
                    .forget_attached(&CancellationToken::new())
                    .await
                    .unwrap(),
                None
            );
        }
    }
}

#[tokio::test]
async fn forget_attached_never_probes_and_respects_cancellation() {
    struct Unprobed;
    #[async_trait::async_trait]
    impl ProcessProbe for Unprobed {
        async fn listener(&self, _: u16, _: &str) -> Result<Listener, StateError> {
            panic!("forget must not probe listeners")
        }
        async fn process(&self, _: u32) -> Result<ProcessIdentity, StateError> {
            panic!("forget must not probe processes")
        }
    }
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &state()).unwrap();
    guard.release().unwrap();
    let adapter = Adapter {
        fail: false,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Unprobed,
    };
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(lifecycle.forget_attached(&cancel).await.is_err());
    assert_eq!(store.read().unwrap(), state());
    assert_eq!(
        lifecycle
            .forget_attached(&CancellationToken::new())
            .await
            .unwrap(),
        state().active
    );
}
#[tokio::test]
async fn replacement_failure_preserves_foreign_state_and_drift_prevents_startup() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &state()).unwrap();
    guard.release().unwrap();
    let adapter = Adapter {
        fail: true,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    let cancel = CancellationToken::new();
    let reviewed = lifecycle.review("next:latest", &cancel).await.unwrap();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "next:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: None,
        cache: None,
    };
    assert!(
        lifecycle
            .replace("ollama", &request, &reviewed, &cancel)
            .await
            .is_err()
    );
    assert_eq!(store.read().unwrap(), state());
    assert!(!*adapter.stopped.lock().unwrap());
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &RuntimeState::default()).unwrap();
    guard.release().unwrap();
    assert!(
        lifecycle
            .replace("ollama", &request, &reviewed, &cancel)
            .await
            .unwrap_err()
            .to_string()
            .contains("changed")
    );
}
#[tokio::test]
async fn owned_replacement_failure_clears_stale_pid_and_detach_does_not_stop_foreign() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let mut owned = state();
    owned.active.as_mut().unwrap().owned_by_us = true;
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &owned).unwrap();
    guard.release().unwrap();
    let adapter = Adapter {
        fail: true,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    let cancel = CancellationToken::new();
    let reviewed = lifecycle.review("next:latest", &cancel).await.unwrap();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "next:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: None,
        cache: None,
    };
    assert!(
        lifecycle
            .replace("ollama", &request, &reviewed, &cancel)
            .await
            .is_err()
    );
    assert!(store.read().unwrap().active.is_none());
    assert!(*adapter.stopped.lock().unwrap());
}

#[tokio::test]
async fn successful_replacement_and_detach_preserve_foreign_process() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let adapter = Adapter {
        fail: false,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    let cancel = CancellationToken::new();
    let review = lifecycle.review("next:latest", &cancel).await.unwrap();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "next:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: None,
        cache: None,
    };
    let active = lifecycle
        .replace("ollama", &request, &review, &cancel)
        .await
        .unwrap();
    assert_eq!(store.read().unwrap().active, Some(active.clone()));
    assert_eq!(lifecycle.down(&cancel).await.unwrap(), Some(active));
    assert!(store.read().unwrap().active.is_none());
    assert!(!*adapter.stopped.lock().unwrap());
}
struct Finalize;
struct DriftingActivation(std::path::PathBuf);
#[async_trait::async_trait]
impl rigspark_runtime::lifecycle::Activation for DriftingActivation {
    async fn finalize(
        &self,
        handle: &ServerState,
        _cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        std::fs::write(&self.0, serde_json::to_vec(&state()).unwrap()).unwrap();
        Ok(handle.clone())
    }
}
#[tokio::test]
async fn state_changed_during_activation_is_not_overwritten() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &RuntimeState::default()).unwrap();
    guard.release().unwrap();
    let adapter = Adapter {
        fail: false,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    let cancel = CancellationToken::new();
    let reviewed = lifecycle.review("next:latest", &cancel).await.unwrap();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "next:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: None,
        cache: None,
    };
    assert!(
        lifecycle
            .replace_with(
                "ollama",
                &request,
                &reviewed,
                &cancel,
                &DriftingActivation(store.config.state.clone())
            )
            .await
            .is_err()
    );
    assert_eq!(store.read().unwrap(), state());
}
struct FailingStop;
#[async_trait::async_trait]
impl BackendAdapter for FailingStop {
    fn name(&self) -> &'static str {
        "ollama"
    }
    fn trusts(&self, identity: &ProcessIdentity) -> bool {
        identity.executable == "/trusted/ollama"
    }
    async fn serve(
        &self,
        _request: &ServeRequest,
        _cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        panic!("unexpected spawn")
    }
    async fn ready(
        &self,
        _request: &ServeRequest,
        _cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        Ok(())
    }
    async fn stop(
        &self,
        _handle: &ServerState,
        _cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        Err(BackendError("stop failed".into()))
    }
}
#[tokio::test]
async fn failed_shutdown_restores_exact_owned_state_and_releases_lock() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let mut prior = state();
    prior.active.as_mut().unwrap().owned_by_us = true;
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &prior).unwrap();
    guard.release().unwrap();
    let bytes = std::fs::read(&store.config.state).unwrap();
    let registry = Registry::new(vec![&FailingStop]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    assert!(
        lifecycle
            .down_with_target(&CancellationToken::new(), || Ok(Some(
                "prior:latest".into()
            )))
            .await
            .unwrap_err()
            .to_string()
            .contains("stop failed")
    );
    assert_eq!(store.read().unwrap(), prior);
    assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
    assert!(!store.config.lock.exists());
}

#[tokio::test]
async fn shutdown_clear_failure_never_stops_and_successful_shutdown_is_idempotent() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let mut prior = state();
    prior.active.as_mut().unwrap().owned_by_us = true;
    let guard = store.lock(Duration::from_secs(1)).unwrap();
    store.write(&guard, &prior).unwrap();
    guard.release().unwrap();
    let bytes = std::fs::read(&store.config.state).unwrap();
    std::fs::remove_dir(&store.config.staging).unwrap();
    std::fs::write(&store.config.staging, b"blocked staging").unwrap();
    let adapter = Adapter {
        fail: false,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    let cancel = CancellationToken::new();
    assert!(lifecycle.down(&cancel).await.is_err());
    assert!(!*adapter.stopped.lock().unwrap());
    assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
    assert!(!store.config.lock.exists());
    std::fs::remove_file(&store.config.staging).unwrap();
    assert_eq!(lifecycle.down(&cancel).await.unwrap(), prior.active);
    assert!(*adapter.stopped.lock().unwrap());
    *adapter.stopped.lock().unwrap() = false;
    assert_eq!(lifecycle.down(&cancel).await.unwrap(), None);
    assert!(!*adapter.stopped.lock().unwrap());
    assert_eq!(store.read().unwrap(), RuntimeState::default());
}
#[async_trait::async_trait]
impl rigspark_runtime::lifecycle::Activation for Finalize {
    async fn finalize(
        &self,
        handle: &ServerState,
        _cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        let mut active = handle.clone();
        active.runtime_model_id = Some("llmup-context-fixture:8192".into());
        active.context = Some(8192);
        active.integrity = Some("local-manifest".into());
        active.local_manifest_digest = Some("a".repeat(64));
        Ok(active)
    }
}
#[tokio::test]
async fn installed_attachment_retains_ownership_of_same_recorded_daemon() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let mut prior = state();
    prior.active.as_mut().unwrap().owned_by_us = true;
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &prior).unwrap();
    guard.release().unwrap();
    let adapter = Adapter {
        fail: false,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    let cancel = CancellationToken::new();
    let review = lifecycle.review("next:latest", &cancel).await.unwrap();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "next:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: Some(8192),
        cache: None,
    };
    let active = lifecycle
        .attach_installed(&request, &review, &cancel, &Finalize)
        .await
        .unwrap();
    assert!(active.owned_by_us);
    assert_eq!(active.context, Some(8192));
    assert_eq!(store.read().unwrap().active, Some(active));
    assert!(!*adapter.stopped.lock().unwrap());
}
#[tokio::test]
async fn cancelled_replacement_never_rewrites_state() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let adapter = Adapter {
        fail: false,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    let cancel = CancellationToken::new();
    let review = lifecycle.review("next:latest", &cancel).await.unwrap();
    cancel.cancel();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "next:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: None,
        cache: None,
    };
    assert!(
        lifecycle
            .replace("ollama", &request, &review, &cancel)
            .await
            .is_err()
    );
    assert!(!store.config.state.exists());
    assert!(!*adapter.stopped.lock().unwrap());
}

struct NoProbe;
#[async_trait::async_trait]
impl ProcessProbe for NoProbe {
    async fn listener(&self, _port: u16, _host: &str) -> Result<Listener, StateError> {
        panic!("unexpected listener probe")
    }
    async fn process(&self, _pid: u32) -> Result<ProcessIdentity, StateError> {
        panic!("unexpected process probe")
    }
}

#[tokio::test]
async fn guarded_down_mismatch_has_no_effects_for_owned_or_foreign_daemons() {
    for owned in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let store = StateStore::new(Config::from_home(home.path()).unwrap());
        let mut prior = state();
        prior.active.as_mut().unwrap().owned_by_us = owned;
        let guard = store.lock(Duration::from_millis(10)).unwrap();
        store.write(&guard, &prior).unwrap();
        guard.release().unwrap();
        let bytes = std::fs::read(&store.config.state).unwrap();
        let registry = Registry::new(vec![]);
        let lifecycle = Lifecycle {
            store: &store,
            registry: &registry,
            probe: &NoProbe,
        };
        let error = lifecycle
            .down_with_target(&CancellationToken::new(), || {
                assert!(store.config.lock.exists());
                Ok(Some("prior:other".into()))
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "prior:other is not the active model (prior:latest)"
        );
        assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
        assert!(!store.config.lock.exists());
    }
}

#[tokio::test]
async fn guarded_down_match_stops_owned_and_only_detaches_foreign_daemons() {
    use rigspark_core::catalog::{Catalog, resolve};
    let mut catalog: Catalog =
        serde_json::from_str(include_str!("../../rigspark-core/data/models.json")).unwrap();
    catalog.models.truncate(1);
    catalog.models[0].id = "prior:latest".into();
    catalog.models[0].family = "prior".into();
    let quant_query = format!("prior:latest-{}", catalog.models[0].quantizations[0].name);
    for owned in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let store = StateStore::new(Config::from_home(home.path()).unwrap());
        let mut prior = state();
        let active = prior.active.as_mut().unwrap();
        active.owned_by_us = owned;
        active.runtime_model_id = Some("llmup-context-fixture:8192".into());
        active.context = Some(8192);
        active.integrity = Some("local-manifest".into());
        active.local_manifest_digest = Some("a".repeat(64));
        let adapter = Adapter {
            fail: false,
            stopped: Mutex::new(false),
        };
        let registry = Registry::new(vec![&adapter]);
        let lifecycle = Lifecycle {
            store: &store,
            registry: &registry,
            probe: &Probe,
        };
        for query in [
            None,
            Some("prior:latest"),
            Some("prior"),
            Some(" PRIOR:LATEST "),
            Some(quant_query.as_str()),
        ] {
            let guard = store.lock(Duration::from_millis(10)).unwrap();
            store.write(&guard, &prior).unwrap();
            guard.release().unwrap();
            *adapter.stopped.lock().unwrap() = false;
            let stopped = lifecycle
                .down_with_target(&CancellationToken::new(), || {
                    assert!(store.config.lock.exists());
                    query
                        .map(|query| {
                            resolve(&catalog, query)
                                .map(|resolved| resolved.model.id.clone())
                                .map_err(|error| BackendError(error.message))
                        })
                        .transpose()
                })
                .await
                .unwrap();
            assert_eq!(stopped, prior.active);
            assert_eq!(*adapter.stopped.lock().unwrap(), owned);
            assert_eq!(store.read().unwrap(), RuntimeState::default());
            assert!(!store.config.lock.exists());
        }
    }
}

#[tokio::test]
async fn guarded_down_without_active_skips_resolution_and_probing() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let registry = Registry::new(vec![]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &NoProbe,
    };
    assert_eq!(
        lifecycle
            .down_with_target(&CancellationToken::new(), || panic!(
                "unexpected resolution"
            ))
            .await
            .unwrap(),
        None
    );
    assert!(!store.config.state.exists());
    assert!(!store.config.lock.exists());
    assert!(home.path().read_dir().unwrap().next().is_none());
}

#[tokio::test]
async fn guarded_down_uses_state_after_waiting_for_the_shutdown_lock() {
    use std::{future::Future, task::Poll};
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &state()).unwrap();
    let registry = Registry::new(vec![]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &NoProbe,
    };
    let cancel = CancellationToken::new();
    let resolutions = std::sync::atomic::AtomicUsize::new(0);
    let pending = lifecycle.down_with_target(&cancel, || {
        resolutions.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert!(store.config.lock.exists());
        assert_eq!(
            store.read().unwrap().active.unwrap().model_id,
            "next:latest"
        );
        Ok(Some("prior:latest".into()))
    });
    tokio::pin!(pending);
    std::future::poll_fn(|context| {
        assert!(pending.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(resolutions.load(std::sync::atomic::Ordering::SeqCst), 0);
    let mut changed = state();
    changed.active.as_mut().unwrap().model_id = "next:latest".into();
    store.write(&guard, &changed).unwrap();
    guard.release().unwrap();
    assert_eq!(
        pending.await.unwrap_err().to_string(),
        "prior:latest is not the active model (next:latest)"
    );
    assert_eq!(store.read().unwrap(), changed);
    assert_eq!(resolutions.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(!store.config.lock.exists());
}

#[tokio::test]
async fn guarded_down_skips_resolution_when_active_disappears_while_waiting_for_lock() {
    use std::{future::Future, task::Poll};
    for remove_state in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let store = StateStore::new(Config::from_home(home.path()).unwrap());
        let guard = store.lock(Duration::from_millis(10)).unwrap();
        store.write(&guard, &state()).unwrap();
        let registry = Registry::new(vec![]);
        let lifecycle = Lifecycle {
            store: &store,
            registry: &registry,
            probe: &NoProbe,
        };
        let cancel = CancellationToken::new();
        let pending = lifecycle.down_with_target(&cancel, || panic!("unexpected resolution"));
        tokio::pin!(pending);
        std::future::poll_fn(|context| {
            assert!(pending.as_mut().poll(context).is_pending());
            Poll::Ready(())
        })
        .await;
        if remove_state {
            std::fs::remove_file(&store.config.state).unwrap();
        } else {
            store.write(&guard, &RuntimeState::default()).unwrap();
        }
        guard.release().unwrap();
        assert_eq!(pending.await.unwrap(), None);
        assert_eq!(store.read().unwrap(), RuntimeState::default());
        assert_eq!(store.config.state.exists(), !remove_state);
        assert!(!store.config.lock.exists());
    }
}

#[tokio::test]
async fn guarded_down_resolution_failure_and_cancellation_preserve_state() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &state()).unwrap();
    guard.release().unwrap();
    let registry = Registry::new(vec![]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &NoProbe,
    };
    assert_eq!(
        lifecycle
            .down_with_target(&CancellationToken::new(), || {
                assert!(store.config.lock.exists());
                Err(BackendError("unknown model".into()))
            })
            .await
            .unwrap_err()
            .to_string(),
        "unknown model"
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        lifecycle
            .down_with_target(&cancel, || panic!("unexpected resolution"))
            .await
            .unwrap_err()
            .to_string(),
        "cancelled"
    );
    assert_eq!(store.read().unwrap(), state());
    assert!(!store.config.lock.exists());
}

struct ChangedProbe<'runtime> {
    store: &'runtime StateStore,
    changed: Option<RuntimeState>,
    unknown: bool,
}
#[async_trait::async_trait]
impl ProcessProbe for ChangedProbe<'_> {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        assert!(self.store.config.lock.exists());
        if self.unknown {
            return Err(StateError {
                kind: "invalid",
                message: "unknown process identity".into(),
            });
        }
        let mut listener = Probe.listener(port, host).await?;
        if let Some(changed) = &self.changed {
            std::fs::write(
                &self.store.config.state,
                serde_json::to_vec(changed).unwrap(),
            )
            .unwrap();
        } else {
            listener.identity.started = "reused-pid".into();
        }
        Ok(listener)
    }
    async fn process(&self, _pid: u32) -> Result<ProcessIdentity, StateError> {
        panic!("unexpected process probe")
    }
}

#[tokio::test]
async fn guarded_down_rejects_state_drift_unknown_identity_and_reused_pid() {
    for owned in [true, false] {
        for scenario in ["state", "unknown", "pid"] {
            let home = tempfile::tempdir().unwrap();
            let store = StateStore::new(Config::from_home(home.path()).unwrap());
            let mut prior = state();
            prior.active.as_mut().unwrap().owned_by_us = owned;
            let guard = store.lock(Duration::from_millis(10)).unwrap();
            store.write(&guard, &prior).unwrap();
            guard.release().unwrap();
            let mut changed = prior.clone();
            changed.active.as_mut().unwrap().model_id = "next:latest".into();
            let probe = ChangedProbe {
                store: &store,
                changed: (scenario == "state").then_some(changed.clone()),
                unknown: scenario == "unknown",
            };
            let adapter = Adapter {
                fail: false,
                stopped: Mutex::new(false),
            };
            let registry = Registry::new(vec![&adapter]);
            let lifecycle = Lifecycle {
                store: &store,
                registry: &registry,
                probe: &probe,
            };
            assert!(
                lifecycle
                    .down_with_target(&CancellationToken::new(), || Ok(Some(
                        "prior:latest".into()
                    )))
                    .await
                    .is_err()
            );
            assert_eq!(
                store.read().unwrap(),
                if scenario == "state" { changed } else { prior }
            );
            assert!(!*adapter.stopped.lock().unwrap());
            assert!(!store.config.lock.exists());
        }
    }
}

struct CancellingProbe {
    cancel: CancellationToken,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl ProcessProbe for CancellingProbe {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        if self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            self.cancel.cancel();
        }
        Probe.listener(port, host).await
    }

    async fn process(&self, _pid: u32) -> Result<ProcessIdentity, StateError> {
        panic!("unexpected process probe")
    }
}

#[tokio::test]
async fn guarded_down_cancelled_during_final_identity_check_preserves_state() {
    for owned in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let store = StateStore::new(Config::from_home(home.path()).unwrap());
        let mut prior = state();
        prior.active.as_mut().unwrap().owned_by_us = owned;
        let guard = store.lock(Duration::from_millis(10)).unwrap();
        store.write(&guard, &prior).unwrap();
        guard.release().unwrap();
        let bytes = std::fs::read(&store.config.state).unwrap();
        let adapter = Adapter {
            fail: false,
            stopped: Mutex::new(false),
        };
        let registry = Registry::new(vec![&adapter]);
        let cancel = CancellationToken::new();
        let probe = CancellingProbe {
            cancel: cancel.clone(),
            calls: std::sync::atomic::AtomicUsize::new(0),
        };
        let lifecycle = Lifecycle {
            store: &store,
            registry: &registry,
            probe: &probe,
        };
        assert_eq!(
            lifecycle
                .down_with_target(&cancel, || Ok(Some("prior:latest".into())))
                .await
                .unwrap_err()
                .to_string(),
            "cancelled"
        );
        assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
        assert!(!*adapter.stopped.lock().unwrap());
        assert!(!store.config.lock.exists());
    }
}

struct Owning(Adapter);
#[async_trait::async_trait]
impl BackendAdapter for Owning {
    fn name(&self) -> &'static str {
        "ollama"
    }
    fn trusts(&self, identity: &ProcessIdentity) -> bool {
        self.0.trusts(identity)
    }
    async fn serve(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        let mut handle = self.0.serve(request, cancel).await?;
        handle.owned_by_us = true;
        Ok(handle)
    }
    async fn ready(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        self.0.ready(request, cancel).await
    }
    async fn stop(
        &self,
        handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        self.0.stop(handle, cancel).await
    }
}
#[tokio::test]
async fn owned_runtime_is_stopped_when_persisting_state_fails() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &RuntimeState::default()).unwrap();
    guard.release().unwrap();
    let adapter = Owning(Adapter {
        fail: false,
        stopped: Mutex::new(false),
    });
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    let cancel = CancellationToken::new();
    let reviewed = lifecycle.review("next:latest", &cancel).await.unwrap();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "next:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: None,
        cache: None,
    };
    let drift = DriftingActivation(store.config.state.clone());
    assert!(
        lifecycle
            .replace_with("ollama", &request, &reviewed, &cancel, &drift)
            .await
            .is_err()
    );
    assert!(*adapter.0.stopped.lock().unwrap());
    assert_eq!(store.read().unwrap(), state());
    assert!(!store.config.lock.exists());
}

#[tokio::test]
async fn an_unappliable_cache_profile_is_refused_before_the_running_server_is_touched() {
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let mut owned = state();
    owned.active.as_mut().unwrap().owned_by_us = true;
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &owned).unwrap();
    guard.release().unwrap();
    let before = std::fs::read(&store.config.state).unwrap();
    let adapter = Adapter {
        fail: false,
        stopped: Mutex::new(false),
    };
    let registry = Registry::new(vec![&adapter]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &Probe,
    };
    let cancel = CancellationToken::new();
    let reviewed = lifecycle.review("next:latest", &cancel).await.unwrap();
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "next:latest".into(),
        endpoint: "http://127.0.0.1:11435".into(),
        model_path: None,
        context: None,
        cache: Some(rigspark_runtime::cache::CacheProfile {
            kv_k: rigspark_core::sizing::KvCacheType::Q8_0,
            kv_v: rigspark_core::sizing::KvCacheType::Q4_0,
            ..Default::default()
        }),
    };
    let error = lifecycle
        .replace("ollama", &request, &reviewed, &cancel)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ollama"), "{error}");
    assert!(!*adapter.stopped.lock().unwrap());
    assert_eq!(std::fs::read(&store.config.state).unwrap(), before);
    assert!(!store.config.lock.exists());
}

struct GoneProbe {
    alive: Result<bool, ()>,
    asked: Mutex<Vec<u32>>,
}
#[async_trait::async_trait]
impl ProcessProbe for GoneProbe {
    async fn listener(&self, port: u16, host: &str) -> Result<Listener, StateError> {
        RestartedProbe.listener(port, host).await
    }
    async fn process(&self, pid: u32) -> Result<ProcessIdentity, StateError> {
        RestartedProbe.process(pid).await
    }
    async fn alive(&self, pid: u32) -> Result<bool, StateError> {
        self.asked.lock().unwrap().push(pid);
        self.alive.map_err(|()| StateError {
            kind: "invalid",
            message: "probe failed".into(),
        })
    }
}

#[tokio::test]
async fn a_record_whose_process_is_gone_is_cleared_before_replacement() {
    // (owned, liveness answer, cleared)
    for (owned, alive, cleared) in [
        (true, Ok(false), true),
        (true, Ok(true), false),
        (true, Err(()), false),
        (false, Ok(false), true),
        (false, Ok(true), false),
    ] {
        let home = tempfile::tempdir().unwrap();
        let store = StateStore::new(Config::from_home(home.path()).unwrap());
        let mut prior = state();
        prior.active.as_mut().unwrap().owned_by_us = owned;
        let guard = store.lock(Duration::from_millis(10)).unwrap();
        store.write(&guard, &prior).unwrap();
        guard.release().unwrap();
        let adapter = Adapter {
            fail: false,
            stopped: Mutex::new(false),
        };
        let registry = Registry::new(vec![&adapter]);
        let probe = GoneProbe {
            alive,
            asked: Mutex::new(Vec::new()),
        };
        let lifecycle = Lifecycle {
            store: &store,
            registry: &registry,
            probe: &probe,
        };
        let reviewed = lifecycle
            .review("next:latest", &CancellationToken::new())
            .await;
        let case = format!("owned={owned} alive={alive:?}");
        assert!(!*adapter.stopped.lock().unwrap(), "{case}: never signals");
        assert!(!store.config.lock.exists(), "{case}");
        if cleared {
            assert!(reviewed.is_ok(), "{case}: {:?}", reviewed.err());
            assert!(store.read().unwrap().active.is_none(), "{case}");
            assert_eq!(*probe.asked.lock().unwrap(), [123], "{case}");
        } else {
            let error = reviewed.err().unwrap().to_string();
            assert!(error.contains("does not match"), "{case}: {error}");
            assert_eq!(store.read().unwrap(), prior, "{case}: record kept");
        }
        let guard = store.lock(Duration::from_millis(10)).unwrap();
        store.write(&guard, &prior).unwrap();
        guard.release().unwrap();
        let down = lifecycle.down(&CancellationToken::new()).await;
        assert!(
            !*adapter.stopped.lock().unwrap(),
            "{case}: down never signals a dead or foreign process"
        );
        assert_eq!(down.is_ok(), cleared, "{case}: {down:?}");
        assert_eq!(store.read().unwrap().active.is_none(), cleared, "{case}");
    }
}
