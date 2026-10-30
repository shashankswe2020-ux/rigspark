use crate::{
    adapters::{BackendAdapter, BackendError, ServeRequest},
    identity::{Confirmation, LiveIdentity, Operation, ProcessProbe, capture},
    process_control::listener,
    state::{LockGuard, RuntimeState, ServerState, StateStore},
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
pub struct Registry<'runtime> {
    adapters: Vec<&'runtime dyn BackendAdapter>,
}
impl<'runtime> Registry<'runtime> {
    pub fn new(adapters: Vec<&'runtime dyn BackendAdapter>) -> Self {
        Self { adapters }
    }
    pub fn get(&self, name: &str) -> Result<&'runtime dyn BackendAdapter, BackendError> {
        let mut matching = self
            .adapters
            .iter()
            .filter(|adapter| adapter.name() == name);
        let adapter = matching
            .next()
            .ok_or_else(|| BackendError("backend not registered".into()))?;
        if matching.next().is_some() {
            return Err(BackendError("duplicate backend registration".into()));
        }
        Ok(*adapter)
    }
}
pub struct Reviewed {
    state: RuntimeState,
    snapshot: Confirmation,
}
pub struct Lifecycle<'runtime> {
    pub store: &'runtime StateStore,
    pub registry: &'runtime Registry<'runtime>,
    pub probe: &'runtime dyn ProcessProbe,
}
#[async_trait::async_trait]
pub trait Activation: Send + Sync {
    async fn finalize(
        &self,
        handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError>;
}
struct Unchanged;
#[async_trait::async_trait]
impl Activation for Unchanged {
    async fn finalize(
        &self,
        handle: &ServerState,
        _cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        Ok(handle.clone())
    }
}
impl Lifecycle<'_> {
    async fn stop_verified(
        &self,
        active: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        let state = RuntimeState::for_active(active.clone());
        let live = self
            .live(&state, cancel)
            .await?
            .ok_or_else(|| BackendError("owned process identity unavailable".into()))?;
        let mut handle = active.clone();
        handle.pid = Some(live.expected.pid);
        handle.process_executable = Some(live.expected.executable);
        handle.process_started_at = Some(live.expected.started);
        self.registry
            .get(&active.backend)?
            .stop(&handle, cancel)
            .await
    }
    pub async fn health(
        &self,
        active: &ServerState,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        let state = RuntimeState::for_active(active.clone());
        let operation = async {
            if self
                .store
                .read()
                .map_err(|error| BackendError(error.to_string()))?
                != state
            {
                return Err(BackendError("active state changed".into()));
            }
            let before = self.live(&state, cancel).await?;
            self.registry
                .get(&active.backend)?
                .ready_handle(request, active, cancel)
                .await?;
            let after = self.live(&state, cancel).await?;
            if before != after
                || self
                    .store
                    .read()
                    .map_err(|error| BackendError(error.to_string()))?
                    != state
            {
                return Err(BackendError(
                    "active runtime changed during health check".into(),
                ));
            }
            Ok(())
        };
        tokio::select! {biased;_=cancel.cancelled()=>Err(BackendError("cancelled".into())),result=tokio::time::timeout(Duration::from_secs(10),operation)=>result.map_err(|_|BackendError("active runtime health timed out".into()))?}
    }
    async fn live(
        &self,
        state: &RuntimeState,
        cancel: &CancellationToken,
    ) -> Result<Option<LiveIdentity>, BackendError> {
        let Some(active) = &state.active else {
            return Ok(None);
        };
        let adapter = self.registry.get(&active.backend)?;
        let observed = listener(self.probe, &active.endpoint, cancel).await?;
        capture(active, &observed, adapter.trusts(&observed.identity))
            .map(Some)
            .map_err(|error| {
                BackendError(if active.owned_by_us {
                    error.to_string()
                } else {
                    format!(
                        "{error}; if the attached {} daemon restarted, run `llmup down --forget` to clear the stale pointer",
                        active.backend
                    )
                })
            })
    }
    /// Clears an attached (not owned) pointer under the state lock without probing or signalling any process.
    pub async fn forget_attached(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<ServerState>, BackendError> {
        let guard = self.lock(cancel).await?;
        let prior = self
            .store
            .read()
            .map_err(|error| BackendError(error.to_string()))?;
        let outcome = match &prior.active {
            None => Ok(None),
            Some(active) if active.owned_by_us => Err(BackendError(
                "--forget only clears attached servers; this server is owned by rigspark, so use `llmup down`"
                    .into(),
            )),
            Some(_) if cancel.is_cancelled() => Err(BackendError("cancelled".into())),
            Some(_) => self
                .store
                .compare_and_write(&guard, &prior, &RuntimeState::default())
                .map(|()| prior.active.clone())
                .map_err(|error| BackendError(error.to_string())),
        };
        guard
            .release()
            .map_err(|error| BackendError(error.to_string()))?;
        outcome
    }
    /// Clears a record whose process is provably gone (after a reboot, or when the daemon it
    /// attached to exited), so a stale pointer cannot wedge every later launch. Never probes
    /// listeners or signals processes; a live PID or unknown liveness keeps the record.
    pub async fn forget_dead(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<ServerState>, BackendError> {
        let Some(pid) = self
            .store
            .read()
            .map_err(|error| BackendError(error.to_string()))?
            .active
            .and_then(|active| active.pid)
        else {
            return Ok(None);
        };
        if !matches!(self.probe.alive(pid).await, Ok(false)) {
            return Ok(None);
        }
        let guard = self.lock(cancel).await?;
        let prior = self
            .store
            .read()
            .map_err(|error| BackendError(error.to_string()))?;
        let outcome = match &prior.active {
            Some(active) if active.pid == Some(pid) => self
                .store
                .compare_and_write(&guard, &prior, &RuntimeState::default())
                .map(|()| prior.active.clone())
                .map_err(|error| BackendError(error.to_string())),
            _ => Ok(None),
        };
        guard
            .release()
            .map_err(|error| BackendError(error.to_string()))?;
        outcome
    }
    pub async fn review(
        &self,
        target: &str,
        cancel: &CancellationToken,
    ) -> Result<Reviewed, BackendError> {
        self.forget_dead(cancel).await?;
        let state = self
            .store
            .read()
            .map_err(|error| BackendError(error.to_string()))?;
        let live = self.live(&state, cancel).await?;
        let snapshot = Confirmation::prepare(
            Operation::ReplaceServer,
            &state,
            Some(target),
            live.as_ref(),
        )
        .map_err(|error| BackendError(error.to_string()))?;
        Ok(Reviewed { state, snapshot })
    }
    async fn lock(&self, cancel: &CancellationToken) -> Result<LockGuard, BackendError> {
        if cancel.is_cancelled() {
            return Err(BackendError("cancelled".into()));
        }
        let config = self.store.config.clone();
        let pending = tokio::task::spawn_blocking(move || {
            StateStore::new(config).lock(Duration::from_secs(10))
        });
        tokio::select! {biased;_=cancel.cancelled()=>Err(BackendError("cancelled".into())),result=pending=>result.map_err(|_|BackendError("state lock task failed".into()))?.map_err(|error|BackendError(error.to_string()))}
    }
    async fn revalidate(
        &self,
        target: &str,
        reviewed: &Reviewed,
        cancel: &CancellationToken,
    ) -> Result<RuntimeState, BackendError> {
        let state = self
            .store
            .read()
            .map_err(|error| BackendError(error.to_string()))?;
        if state != reviewed.state {
            return Err(BackendError(
                "active state changed; retry preparation".into(),
            ));
        }
        let live = self.live(&state, cancel).await?;
        let snapshot = Confirmation::prepare(
            Operation::ReplaceServer,
            &state,
            Some(target),
            live.as_ref(),
        )
        .map_err(|error| BackendError(error.to_string()))?;
        reviewed
            .snapshot
            .verify(&snapshot)
            .map_err(|error| BackendError(error.to_string()))?;
        Ok(state)
    }
    pub async fn replace(
        &self,
        backend: &str,
        request: &ServeRequest,
        reviewed: &Reviewed,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        self.replace_with(backend, request, reviewed, cancel, &Unchanged)
            .await
    }
    pub async fn replace_with(
        &self,
        backend: &str,
        request: &ServeRequest,
        reviewed: &Reviewed,
        cancel: &CancellationToken,
        activation: &dyn Activation,
    ) -> Result<ServerState, BackendError> {
        let adapter = self.registry.get(backend)?;
        crate::cache::launch_delta(request.cache.as_ref(), backend, true)
            .map_err(|error| BackendError(error.to_string()))?;
        let guard = self.lock(cancel).await?;
        let prior = self.revalidate(&request.model_id, reviewed, cancel).await?;
        if let Some(active) = &prior.active
            && active.owned_by_us
        {
            self.stop_verified(active, cancel).await?;
            self.store
                .write(&guard, &RuntimeState::default())
                .map_err(|error| BackendError(error.to_string()))?;
        }
        let expected_state = if prior
            .active
            .as_ref()
            .is_some_and(|active| active.owned_by_us)
        {
            RuntimeState::default()
        } else {
            prior.clone()
        };
        let handle = adapter.serve(request, cancel).await?;
        let outcome = async {
            adapter.ready_handle(request, &handle, cancel).await?;
            let final_state = activation.finalize(&handle, cancel).await?;
            if final_state.pid != handle.pid
                || final_state.endpoint != handle.endpoint
                || final_state.owned_by_us != handle.owned_by_us
                || final_state.backend != backend
                || final_state.model_id != request.model_id
                || final_state.process_executable != handle.process_executable
                || final_state.process_started_at != handle.process_started_at
                || final_state.cache != handle.cache
            {
                return Err(BackendError("activation changed runtime ownership".into()));
            }
            if cancel.is_cancelled() {
                return Err(BackendError("cancelled".into()));
            }
            self.live(&RuntimeState::for_active(final_state.clone()), cancel)
                .await?;
            self.store
                .compare_and_write(
                    &guard,
                    &expected_state,
                    &RuntimeState::for_active(final_state.clone()),
                )
                .map_err(|error| BackendError(error.to_string()))?;
            Ok(final_state)
        }
        .await;
        if outcome.is_err() && handle.owned_by_us {
            adapter.stop(&handle, &CancellationToken::new()).await?;
        }
        guard
            .release()
            .map_err(|error| BackendError(error.to_string()))?;
        outcome
    }
    pub async fn switch_pointer(
        &self,
        target: &str,
        reviewed: &Reviewed,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        let guard = self.lock(cancel).await?;
        let current = self.revalidate(target, reviewed, cancel).await?;
        let mut active = current
            .active
            .clone()
            .ok_or_else(|| BackendError("no active server to switch".into()))?;
        if active.backend != "ollama" {
            return Err(BackendError(
                "single-model or delegated runtime requires up".into(),
            ));
        }
        self.registry
            .get("ollama")?
            .ready_handle(
                &ServeRequest {
                    draft_model_path: None,
                    model_id: target.into(),
                    endpoint: active.endpoint.clone(),
                    model_path: None,
                    context: None,
                    cache: None,
                },
                &active,
                cancel,
            )
            .await?;
        self.revalidate(target, reviewed, cancel).await?;
        active.model_id = target.into();
        active.runtime_model_id = None;
        active.context = None;
        active.integrity = None;
        active.local_manifest_digest = None;
        self.store
            .compare_and_write(&guard, &current, &RuntimeState::for_active(active.clone()))
            .map_err(|error| BackendError(error.to_string()))?;
        guard
            .release()
            .map_err(|error| BackendError(error.to_string()))?;
        Ok(active)
    }
    pub async fn attach_installed(
        &self,
        request: &ServeRequest,
        reviewed: &Reviewed,
        cancel: &CancellationToken,
        activation: &dyn Activation,
    ) -> Result<ServerState, BackendError> {
        if crate::cache::effective(request.cache.as_ref()).is_some() {
            return Err(BackendError(
                "cache settings cannot be applied to an installed model on a running daemon".into(),
            ));
        }
        let guard = self.lock(cancel).await?;
        let prior = self.revalidate(&request.model_id, reviewed, cancel).await?;
        if prior.active.as_ref().is_some_and(|active| {
            active.owned_by_us
                && (active.backend != "ollama" || active.endpoint != request.endpoint)
        }) {
            return Err(BackendError(
                "stop the prior owned runtime before attaching another daemon".into(),
            ));
        }
        let adapter = self.registry.get("ollama")?;
        let handle = adapter.attach_only(request, cancel).await?;
        if handle.owned_by_us {
            adapter.stop(&handle, &CancellationToken::new()).await?;
            return Err(BackendError(
                "installed activation requires an existing daemon".into(),
            ));
        }
        let mut active = activation.finalize(&handle, cancel).await?;
        if active.pid != handle.pid
            || active.endpoint != handle.endpoint
            || active.backend != "ollama"
            || active.owned_by_us
            || active.process_executable != handle.process_executable
            || active.process_started_at != handle.process_started_at
        {
            return Err(BackendError(
                "installed activation changed daemon identity".into(),
            ));
        }
        adapter.ready_handle(request, &active, cancel).await?;
        active.owned_by_us = prior
            .active
            .as_ref()
            .is_some_and(|prior| prior.owned_by_us && prior.pid == active.pid);
        self.live(&RuntimeState::for_active(active.clone()), cancel)
            .await?;
        self.store
            .compare_and_write(&guard, &prior, &RuntimeState::for_active(active.clone()))
            .map_err(|error| BackendError(error.to_string()))?;
        guard
            .release()
            .map_err(|error| BackendError(error.to_string()))?;
        Ok(active)
    }
    pub async fn down(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<ServerState>, BackendError> {
        self.down_with_target(cancel, || Ok(None)).await
    }
    pub async fn down_with_target(
        &self,
        cancel: &CancellationToken,
        resolve_target: impl FnOnce() -> Result<Option<String>, BackendError> + Send,
    ) -> Result<Option<ServerState>, BackendError> {
        if cancel.is_cancelled() {
            return Err(BackendError("cancelled".into()));
        }
        if self
            .store
            .read()
            .map_err(|error| BackendError(error.to_string()))?
            .active
            .is_none()
        {
            return Ok(None);
        }
        let guard = self.lock(cancel).await?;
        let prepared = self
            .store
            .read()
            .map_err(|error| BackendError(error.to_string()))?;
        let Some(active) = &prepared.active else {
            return Ok(None);
        };
        if let Some(target) = resolve_target()?
            && target != active.model_id
        {
            return Err(BackendError(format!(
                "{target} is not the active model ({})",
                active.model_id
            )));
        }
        // The record the caller asked to stop may be cleared once its process is provably gone.
        if let Some(pid) = active.pid
            && matches!(self.probe.alive(pid).await, Ok(false))
        {
            let outcome = self
                .store
                .compare_and_write(&guard, &prepared, &RuntimeState::default())
                .map(|()| prepared.active.clone())
                .map_err(|error| BackendError(error.to_string()));
            guard
                .release()
                .map_err(|error| BackendError(error.to_string()))?;
            return outcome;
        }
        let operation = if active.owned_by_us {
            Operation::Down
        } else {
            Operation::Detach
        };
        let live = self.live(&prepared, cancel).await?;
        let approved = Confirmation::prepare(operation, &prepared, None, live.as_ref())
            .map_err(|error| BackendError(error.to_string()))?;
        let current = self
            .store
            .read()
            .map_err(|error| BackendError(error.to_string()))?;
        let live = self.live(&current, cancel).await?;
        let current_snapshot = Confirmation::prepare(operation, &current, None, live.as_ref())
            .map_err(|error| BackendError(error.to_string()))?;
        approved
            .verify(&current_snapshot)
            .map_err(|error| BackendError(error.to_string()))?;
        if cancel.is_cancelled() {
            return Err(BackendError("cancelled".into()));
        }
        self.store
            .compare_and_write(&guard, &current, &RuntimeState::default())
            .map_err(|error| BackendError(error.to_string()))?;
        if active.owned_by_us
            && let Err(error) = self.stop_verified(active, cancel).await
        {
            self.store.write(&guard, &prepared).map_err(|restore| {
                BackendError(format!("{error}; restoring state failed: {restore}"))
            })?;
            return Err(error);
        }
        guard
            .release()
            .map_err(|error| BackendError(error.to_string()))?;
        Ok(prepared.active)
    }
}
