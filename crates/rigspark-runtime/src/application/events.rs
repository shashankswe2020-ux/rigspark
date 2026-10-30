use crate::{
    adapters::{BackendAdapter, BackendError, ServeRequest},
    identity::ProcessIdentity,
    state::ServerState,
};
use std::{
    collections::VecDeque,
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_util::sync::CancellationToken;

pub const EVENT_CAPACITY: usize = 64;

#[derive(Clone, Copy)]
pub(crate) enum LifecycleWarning {
    InstalledBypass,
    EstimatedFit,
    SizeOnly,
    DrafterSkipped,
}

impl LifecycleWarning {
    fn message(self) -> &'static str {
        match self {
            Self::InstalledBypass => {
                "up: bypassing estimated fit; local content integrity is not catalog verification; throughput may be unknown"
            }
            Self::EstimatedFit => {
                "up: requested quantization may not fit this hardware; continuing because it was explicitly requested"
            }
            Self::SizeOnly => {
                "up: weights passed a size-floor check; no catalog SHA-256 was available"
            }
            Self::DrafterSkipped => {
                "up: skipped the speculative drafter because it does not fit beside the model"
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct DiagnosticObserver {
    state: Arc<Mutex<DiagnosticSnapshot>>,
}

#[derive(Clone, Default)]
pub struct DiagnosticSnapshot {
    warnings: [u64; 3],
    progress: VecDeque<(u64, u64)>,
    omitted_progress: u64,
}

impl DiagnosticObserver {
    pub fn snapshot(&self) -> DiagnosticSnapshot {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    fn warning(&self, warning: LifecycleWarning) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let count = &mut state.warnings[warning as usize];
        *count = count.saturating_add(1);
    }

    fn progress(&self, completed: u64, total: u64) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.progress.len() == EVENT_CAPACITY {
            state.progress.pop_front();
            state.omitted_progress = state.omitted_progress.saturating_add(1);
        }
        state.progress.push_back((completed, total));
    }
}

impl DiagnosticSnapshot {
    pub fn text(&self) -> String {
        let mut lines = Vec::new();
        for warning in [
            LifecycleWarning::InstalledBypass,
            LifecycleWarning::EstimatedFit,
            LifecycleWarning::SizeOnly,
        ] {
            let count = self.warnings[warning as usize];
            if count > 0 {
                lines.push(format!("{} ({count} occurrences)", warning.message()));
            }
        }
        if self.omitted_progress > 0 {
            lines.push(format!(
                "{} earlier progress diagnostics omitted",
                self.omitted_progress
            ));
        }
        for (completed, total) in &self.progress {
            lines.push(format!("  [redacted filename]: {completed}/{total} bytes"));
        }
        if lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", lines.join("\n"))
        }
    }
}

pub(crate) fn warning(observer: Option<&LifecycleObserver>, warning: LifecycleWarning) {
    match observer.and_then(|observer| observer.diagnostics.as_ref()) {
        Some(diagnostics) => diagnostics.warning(warning),
        None => eprintln!("{}", warning.message()),
    }
}

pub(crate) fn download_progress(
    diagnostics: Option<&DiagnosticObserver>,
    completed: u64,
    total: u64,
    file: &str,
) {
    match diagnostics {
        Some(diagnostics) => diagnostics.progress(completed, total),
        None => eprintln!(
            "  {}: {completed}/{total} bytes",
            rigspark_core::reports::strip_control(file)
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleScope {
    Runtime,
    AcquisitionDaemon,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleStage {
    Acquisition,
    Verification,
    AcquisitionVerification,
    Start,
    Attach,
    Activation,
    Readiness,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleStatus {
    Started,
    Completed,
    Failed,
}

/// Operation-level evidence, not command success or state-commit confirmation.
/// Start covers the adapter's serve operation, which may reuse an existing server.
/// AcquisitionVerification covers acquisition APIs that verify before returning.
/// No strings are carried (maximum string payload: 0 bytes), including diagnostics,
/// model identifiers, paths, endpoints, credentials, or subprocess output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleEvent {
    pub scope: LifecycleScope,
    pub stage: LifecycleStage,
    pub status: LifecycleStatus,
}

/// Best-effort, nonblocking observations. Full or closed queues drop new events.
/// Consumers must use the command result, not events, to determine final success.
pub struct LifecycleObserver {
    sender: Sender<LifecycleEvent>,
    pub(crate) diagnostics: Option<DiagnosticObserver>,
}

impl LifecycleObserver {
    pub fn channel() -> (Self, Receiver<LifecycleEvent>) {
        let (sender, receiver) = mpsc::channel(EVENT_CAPACITY);
        (
            Self {
                sender,
                diagnostics: None,
            },
            receiver,
        )
    }

    pub fn with_diagnostics(mut self, diagnostics: DiagnosticObserver) -> Self {
        self.diagnostics = Some(diagnostics);
        self
    }

    fn emit(&self, scope: LifecycleScope, stage: LifecycleStage, status: LifecycleStatus) {
        let _ = self.sender.try_send(LifecycleEvent {
            scope,
            stage,
            status,
        });
    }
}

pub(crate) async fn observe<T>(
    observer: Option<&LifecycleObserver>,
    scope: LifecycleScope,
    stage: LifecycleStage,
    operation: impl Future<Output = Result<T, BackendError>>,
) -> Result<T, BackendError> {
    if let Some(observer) = observer {
        observer.emit(scope, stage, LifecycleStatus::Started);
    }
    let result = operation.await;
    if let Some(observer) = observer {
        observer.emit(
            scope,
            stage,
            if result.is_ok() {
                LifecycleStatus::Completed
            } else {
                LifecycleStatus::Failed
            },
        );
    }
    result
}

pub(super) struct ObservedAdapter<'runtime> {
    inner: &'runtime dyn BackendAdapter,
    observer: Option<&'runtime LifecycleObserver>,
    scope: LifecycleScope,
}

impl<'runtime> ObservedAdapter<'runtime> {
    pub(super) fn new(
        inner: &'runtime dyn BackendAdapter,
        observer: Option<&'runtime LifecycleObserver>,
        scope: LifecycleScope,
    ) -> Self {
        Self {
            inner,
            observer,
            scope,
        }
    }
}

#[async_trait::async_trait]
impl BackendAdapter for ObservedAdapter<'_> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn trusts(&self, identity: &ProcessIdentity) -> bool {
        self.inner.trusts(identity)
    }
    fn can_embed(&self) -> bool {
        self.inner.can_embed()
    }
    fn can_stream(&self) -> bool {
        self.inner.can_stream()
    }

    async fn serve(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        observe(
            self.observer,
            self.scope,
            LifecycleStage::Start,
            self.inner.serve(request, cancel),
        )
        .await
    }
    async fn attach_only(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        observe(
            self.observer,
            self.scope,
            LifecycleStage::Attach,
            self.inner.attach_only(request, cancel),
        )
        .await
    }
    async fn ready(
        &self,
        request: &ServeRequest,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        observe(
            self.observer,
            self.scope,
            LifecycleStage::Readiness,
            self.inner.ready(request, cancel),
        )
        .await
    }
    async fn ready_handle(
        &self,
        request: &ServeRequest,
        handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        observe(
            self.observer,
            self.scope,
            LifecycleStage::Readiness,
            self.inner.ready_handle(request, handle, cancel),
        )
        .await
    }
    async fn stop(
        &self,
        handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<(), BackendError> {
        observe(
            self.observer,
            self.scope,
            LifecycleStage::Stop,
            self.inner.stop(handle, cancel),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adapters::{BackendAdapter, ServeRequest},
        identity::ProcessIdentity,
        state::{RuntimeState, ServerState},
    };
    use std::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn diagnostic_sink_bounds_progress_without_evicting_security_warnings() {
        let diagnostics = DiagnosticObserver::default();
        diagnostics.warning(LifecycleWarning::InstalledBypass);
        diagnostics.warning(LifecycleWarning::EstimatedFit);
        diagnostics.warning(LifecycleWarning::SizeOnly);
        for completed in 0..100 {
            download_progress(
                Some(&diagnostics),
                completed,
                100,
                "\u{1b}]52;c;secret\u{7}/token=secret.bin",
            );
        }
        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.progress.len(), EVENT_CAPACITY);
        assert_eq!(snapshot.omitted_progress, 36);
        let text = snapshot.text();
        assert!(text.contains("no catalog SHA-256"));
        assert!(text.contains("bypassing estimated fit"));
        assert!(text.contains("may not fit"));
        assert!(text.contains("99/100 bytes"));
        assert!(text.contains("36 earlier progress diagnostics omitted"));
        assert!(text.contains("[redacted filename]"));
        assert!(!text.contains("secret"));
        assert!(!text.contains('\u{1b}'));
    }

    #[test]
    fn diagnostic_sink_is_injected_and_keeps_repeated_warning_counts() {
        let (plain, _) = LifecycleObserver::channel();
        assert!(plain.diagnostics.is_none());
        let diagnostics = DiagnosticObserver::default();
        let observed = plain.with_diagnostics(diagnostics.clone());
        for _ in 0..100 {
            warning(Some(&observed), LifecycleWarning::SizeOnly);
        }
        assert!(diagnostics.snapshot().text().contains("100 occurrences"));
        assert!(DiagnosticObserver::default().snapshot().text().is_empty());
    }

    struct Adapter {
        fail: Option<LifecycleStage>,
        calls: Mutex<Vec<LifecycleStage>>,
    }

    impl Adapter {
        fn attempt(
            &self,
            stage: LifecycleStage,
            cancel: &CancellationToken,
        ) -> Result<(), BackendError> {
            self.calls.lock().unwrap().push(stage);
            if cancel.is_cancelled() {
                Err(BackendError("cancelled".into()))
            } else if self.fail == Some(stage) {
                Err(BackendError("mock operation failed".into()))
            } else {
                Ok(())
            }
        }
    }

    fn handle() -> ServerState {
        RuntimeState::parse(r#"{"schemaVersion":2,"active":{"backend":"ollama","modelId":"model:latest","endpoint":"http://127.0.0.1:11434","port":11434,"ownedByUs":true,"pid":123}}"#)
            .unwrap().active.unwrap()
    }

    fn request() -> ServeRequest {
        ServeRequest {
            draft_model_path: None,
            model_id: "model:latest".into(),
            endpoint: "http://127.0.0.1:11434".into(),
            model_path: None,
            context: Some(4096),
            cache: None,
        }
    }

    #[async_trait::async_trait]
    impl BackendAdapter for Adapter {
        fn name(&self) -> &'static str {
            "ollama"
        }
        fn trusts(&self, _identity: &ProcessIdentity) -> bool {
            false
        }
        async fn serve(
            &self,
            request: &ServeRequest,
            cancel: &CancellationToken,
        ) -> Result<ServerState, BackendError> {
            assert_eq!(
                request.context,
                if request.model_id == "llmup-acquisition" {
                    None
                } else {
                    Some(4096)
                }
            );
            self.attempt(LifecycleStage::Start, cancel)?;
            Ok(handle())
        }
        async fn attach_only(
            &self,
            _request: &ServeRequest,
            cancel: &CancellationToken,
        ) -> Result<ServerState, BackendError> {
            self.attempt(LifecycleStage::Attach, cancel)?;
            let mut handle = handle();
            handle.owned_by_us = false;
            Ok(handle)
        }
        async fn ready(
            &self,
            _request: &ServeRequest,
            _cancel: &CancellationToken,
        ) -> Result<(), BackendError> {
            panic!("ready_handle override must be forwarded")
        }
        async fn ready_handle(
            &self,
            _request: &ServeRequest,
            active: &ServerState,
            cancel: &CancellationToken,
        ) -> Result<(), BackendError> {
            assert_eq!(active.pid, Some(123));
            self.attempt(LifecycleStage::Readiness, cancel)
        }
        async fn stop(
            &self,
            active: &ServerState,
            cancel: &CancellationToken,
        ) -> Result<(), BackendError> {
            assert!(active.owned_by_us);
            self.attempt(LifecycleStage::Stop, cancel)
        }
    }

    #[tokio::test]
    async fn runtime_events_adapter_orders_real_calls_and_preserves_failures() {
        for failure in [
            None,
            Some(LifecycleStage::Start),
            Some(LifecycleStage::Readiness),
            Some(LifecycleStage::Stop),
        ] {
            let (observer, mut receiver) = LifecycleObserver::channel();
            let adapter = Adapter {
                fail: failure,
                calls: Mutex::new(Vec::new()),
            };
            let observed = ObservedAdapter::new(&adapter, Some(&observer), LifecycleScope::Runtime);
            let cancel = CancellationToken::new();
            let outcome = async {
                let active = observed.serve(&request(), &cancel).await?;
                observed.ready_handle(&request(), &active, &cancel).await?;
                observed.stop(&active, &cancel).await
            }
            .await;
            assert_eq!(outcome.is_err(), failure.is_some());
            if let Err(error) = outcome {
                assert_eq!(error.0, "mock operation failed");
            }
            let mut expected = Vec::new();
            for stage in [
                LifecycleStage::Start,
                LifecycleStage::Readiness,
                LifecycleStage::Stop,
            ] {
                expected.push(stage);
                assert_eq!(
                    receiver.try_recv().unwrap(),
                    LifecycleEvent {
                        scope: LifecycleScope::Runtime,
                        stage,
                        status: LifecycleStatus::Started
                    }
                );
                assert_eq!(
                    receiver.try_recv().unwrap(),
                    LifecycleEvent {
                        scope: LifecycleScope::Runtime,
                        stage,
                        status: if failure == Some(stage) {
                            LifecycleStatus::Failed
                        } else {
                            LifecycleStatus::Completed
                        }
                    }
                );
                if failure == Some(stage) {
                    break;
                }
            }
            assert_eq!(*adapter.calls.lock().unwrap(), expected);
            assert!(receiver.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn runtime_events_adapter_preserves_attach_and_cancellation() {
        let (observer, mut receiver) = LifecycleObserver::channel();
        let adapter = Adapter {
            fail: None,
            calls: Mutex::new(Vec::new()),
        };
        let observed =
            ObservedAdapter::new(&adapter, Some(&observer), LifecycleScope::AcquisitionDaemon);
        let cancel = CancellationToken::new();
        assert!(
            !observed
                .attach_only(&request(), &cancel)
                .await
                .unwrap()
                .owned_by_us
        );
        assert_eq!(receiver.try_recv().unwrap().stage, LifecycleStage::Attach);
        assert_eq!(
            receiver.try_recv().unwrap().status,
            LifecycleStatus::Completed
        );
        cancel.cancel();
        assert_eq!(
            observed.serve(&request(), &cancel).await.unwrap_err().0,
            "cancelled"
        );
        assert_eq!(
            receiver.try_recv().unwrap().scope,
            LifecycleScope::AcquisitionDaemon
        );
        assert_eq!(receiver.try_recv().unwrap().status, LifecycleStatus::Failed);
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn saturated_or_closed_observation_never_retries_or_replaces_runtime_result() {
        for closed in [false, true] {
            let (observer, mut receiver) = LifecycleObserver::channel();
            for _ in 0..EVENT_CAPACITY {
                observer.emit(
                    LifecycleScope::Runtime,
                    LifecycleStage::Start,
                    LifecycleStatus::Started,
                );
            }
            if closed {
                receiver.close();
            }
            let calls = std::sync::atomic::AtomicUsize::new(0);
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                observe(
                    Some(&observer),
                    LifecycleScope::Runtime,
                    LifecycleStage::Readiness,
                    async {
                        calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        Err::<(), _>(BackendError("authoritative runtime failure".into()))
                    },
                ),
            )
            .await
            .unwrap();
            assert_eq!(result.unwrap_err().0, "authoritative runtime failure");
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(receiver.len(), EVENT_CAPACITY);
            let result = observe(
                Some(&observer),
                LifecycleScope::Runtime,
                LifecycleStage::Stop,
                async { Ok::<_, BackendError>("authoritative success") },
            )
            .await
            .unwrap();
            assert_eq!(result, "authoritative success");
            assert_eq!(receiver.len(), EVENT_CAPACITY);
        }
    }

    #[tokio::test]
    async fn runtime_events_temporary_daemon_cleanup_does_not_claim_pull_success() {
        let (observer, mut receiver) = LifecycleObserver::channel();
        let adapter = Adapter {
            fail: None,
            calls: Mutex::new(Vec::new()),
        };
        let observed =
            ObservedAdapter::new(&adapter, Some(&observer), LifecycleScope::AcquisitionDaemon);
        let cancel = CancellationToken::new();
        let operation = observe(
            Some(&observer),
            LifecycleScope::Runtime,
            LifecycleStage::Acquisition,
            async {
                cancel.cancel();
                Err::<(), _>(BackendError("pull cancelled".into()))
            },
        );
        let result = crate::pull::with_ollama_daemon(
            &observed,
            "http://127.0.0.1:11434",
            &cancel,
            operation,
        )
        .await;
        assert_eq!(result.unwrap_err().0, "pull cancelled");
        assert_eq!(
            *adapter.calls.lock().unwrap(),
            [LifecycleStage::Start, LifecycleStage::Stop]
        );
        for (scope, stage, status) in [
            (
                LifecycleScope::AcquisitionDaemon,
                LifecycleStage::Start,
                LifecycleStatus::Started,
            ),
            (
                LifecycleScope::AcquisitionDaemon,
                LifecycleStage::Start,
                LifecycleStatus::Completed,
            ),
            (
                LifecycleScope::Runtime,
                LifecycleStage::Acquisition,
                LifecycleStatus::Started,
            ),
            (
                LifecycleScope::Runtime,
                LifecycleStage::Acquisition,
                LifecycleStatus::Failed,
            ),
            (
                LifecycleScope::AcquisitionDaemon,
                LifecycleStage::Stop,
                LifecycleStatus::Started,
            ),
            (
                LifecycleScope::AcquisitionDaemon,
                LifecycleStage::Stop,
                LifecycleStatus::Completed,
            ),
        ] {
            assert_eq!(
                receiver.try_recv().unwrap(),
                LifecycleEvent {
                    scope,
                    stage,
                    status
                }
            );
        }
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn runtime_events_bracket_success_and_failure_without_false_completion() {
        let (observer, mut receiver) = LifecycleObserver::channel();
        let operation = async {
            assert_eq!(
                receiver.try_recv().unwrap(),
                LifecycleEvent {
                    scope: LifecycleScope::Runtime,
                    stage: LifecycleStage::Verification,
                    status: LifecycleStatus::Started,
                }
            );
            Ok::<_, BackendError>(42)
        };
        assert_eq!(
            observe(
                Some(&observer),
                LifecycleScope::Runtime,
                LifecycleStage::Verification,
                operation
            )
            .await
            .unwrap(),
            42
        );
        assert_eq!(
            receiver.try_recv().unwrap().status,
            LifecycleStatus::Completed
        );
        let error = observe(
            Some(&observer),
            LifecycleScope::Runtime,
            LifecycleStage::Start,
            async { Err::<(), _>(BackendError("private diagnostic".into())) },
        )
        .await
        .unwrap_err();
        assert_eq!(error.0, "private diagnostic");
        assert_eq!(
            receiver.try_recv().unwrap().status,
            LifecycleStatus::Started
        );
        let failed = receiver.try_recv().unwrap();
        assert_eq!(failed.status, LifecycleStatus::Failed);
        assert!(!format!("{failed:?}").contains("private diagnostic"));
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn runtime_events_full_closed_or_absent_observer_never_blocks_operations() {
        let (observer, mut receiver) = LifecycleObserver::channel();
        for _ in 0..EVENT_CAPACITY * 2 {
            observe(
                Some(&observer),
                LifecycleScope::Runtime,
                LifecycleStage::Readiness,
                async { Ok::<_, BackendError>(()) },
            )
            .await
            .unwrap();
        }
        assert_eq!(receiver.len(), EVENT_CAPACITY);
        receiver.close();
        for observer in [Some(&observer), None] {
            assert_eq!(
                observe(
                    observer,
                    LifecycleScope::Runtime,
                    LifecycleStage::Stop,
                    async { Ok::<_, BackendError>(7) }
                )
                .await
                .unwrap(),
                7
            );
        }
    }

    #[tokio::test]
    async fn runtime_events_dropped_operation_has_no_completion() {
        let (observer, mut receiver) = LifecycleObserver::channel();
        {
            let operation = observe(
                Some(&observer),
                LifecycleScope::Runtime,
                LifecycleStage::Readiness,
                std::future::pending::<Result<(), BackendError>>(),
            );
            tokio::pin!(operation);
            tokio::select! {
                biased;
                _ = &mut operation => panic!("pending operation completed"),
                event = receiver.recv() => assert_eq!(event.unwrap().status, LifecycleStatus::Started),
            }
        }
        assert!(receiver.try_recv().is_err());
    }
}
