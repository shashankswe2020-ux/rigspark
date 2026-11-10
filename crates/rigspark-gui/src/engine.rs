use rigspark_runtime::{
    harness::{
        ChatHarness, DeltaSink, HarnessError, HarnessRequest, LocalHarness, NativeRemoteTransport,
    },
    harness_registry::HarnessRegistry,
    native_runtime::NativeRuntime,
    opencode::NativeOpenCodeRunner,
    state::{Config, StateStore},
};
use std::{collections::BTreeMap, path::PathBuf};
use tokio_util::sync::CancellationToken;
#[async_trait::async_trait]
pub trait Engine: Send + Sync {
    async fn chat(
        &self,
        provider: &str,
        request: &HarnessRequest,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError>;
    async fn agent(
        &self,
        _: &rigspark_runtime::ollama_inference::ChatInput,
        _: &rigspark_runtime::state::RuntimeState,
        _: &CancellationToken,
        _: &mut DeltaSink<'_>,
    ) -> Result<rigspark_runtime::ollama_inference::ChatResult, HarnessError> {
        Err(HarnessError::Unavailable)
    }
}
pub struct NativeEngine {
    pub home: PathBuf,
}
#[async_trait::async_trait]
impl Engine for NativeEngine {
    async fn agent(
        &self,
        request: &rigspark_runtime::ollama_inference::ChatInput,
        expected: &rigspark_runtime::state::RuntimeState,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<rigspark_runtime::ollama_inference::ChatResult, HarnessError> {
        let config = Config::from_home(&self.home).map_err(|_| HarnessError::Invalid)?;
        let state = StateStore::new(config.clone());
        if state.read().map_err(|_| HarnessError::Drift)? != *expected {
            return Err(HarnessError::Drift);
        }
        let runtime = NativeRuntime::new(config)?;
        let adapters = runtime.adapters();
        let registry = adapters.registry();
        let local = LocalHarness {
            state: &state,
            registry: &registry,
            probe: &runtime.probe,
        };
        let mut request = request.clone();
        if ["local", "demo-model"].contains(&request.model.as_str()) {
            request.model.clear();
        }
        let result =
            rigspark_runtime::agent::AgentChat::chat(&local, &request, cancel, sink).await?;
        if state.read().map_err(|_| HarnessError::Drift)? != *expected {
            return Err(HarnessError::Drift);
        }
        Ok(result)
    }
    async fn chat(
        &self,
        provider: &str,
        request: &HarnessRequest,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError> {
        let config = Config::from_home(&self.home).map_err(|_| HarnessError::Invalid)?;
        let runtime = NativeRuntime::new(config.clone())?;
        let adapters = runtime.adapters();
        let registry = adapters.registry();
        let state = StateStore::new(config);
        let remote = NativeRemoteTransport::new()?;
        let mut env: BTreeMap<String, String> = rigspark_runtime::process_control::minimal_env()
            .into_iter()
            .collect();
        for name in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_COMPAT_BASE_URL",
            "OPENAI_COMPAT_API_KEY",
            "RIGSPARK_OPENCODE_UNRESTRICTED",
        ] {
            if let Ok(value) = std::env::var(name) {
                env.insert(name.into(), value);
            }
        }
        let runner = NativeOpenCodeRunner {
            binary: runtime.binary("opencode"),
            env: env.clone(),
            commit_home: Some(self.home.clone()),
        };
        let harnesses = HarnessRegistry::builtins(
            LocalHarness {
                state: &state,
                registry: &registry,
                probe: &runtime.probe,
            },
            &remote,
            &runner,
            &env,
        )?;
        let mut request = request.clone();
        if provider == "local" && ["local", "demo-model"].contains(&request.model.as_str()) {
            request.model = String::new();
        }
        harnesses.get(provider)?.chat(&request, cancel, sink).await
    }
}
pub(crate) struct SelectedEngine<'host> {
    pub name: &'static str,
    pub engine: &'host dyn Engine,
}
pub(crate) struct SelectedAgent<'host> {
    pub engine: &'host dyn Engine,
    pub expected: rigspark_runtime::state::RuntimeState,
}
#[async_trait::async_trait]
impl rigspark_runtime::agent::AgentChat for SelectedAgent<'_> {
    async fn chat(
        &self,
        input: &rigspark_runtime::ollama_inference::ChatInput,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<rigspark_runtime::ollama_inference::ChatResult, HarnessError> {
        self.engine.agent(input, &self.expected, cancel, sink).await
    }
}
#[async_trait::async_trait]
impl ChatHarness for SelectedEngine<'_> {
    fn name(&self) -> &'static str {
        self.name
    }
    async fn available(&self) -> bool {
        true
    }
    async fn chat(
        &self,
        request: &HarnessRequest,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError> {
        self.engine.chat(self.name, request, cancel, sink).await
    }
}
