use crate::{
    harness::{
        DeltaSink, HarnessError, HarnessMessage, HarnessRequest, LocalHarness,
        NativeRemoteTransport,
    },
    harness_registry::HarnessRegistry,
    library::Library,
    memory::{CaptureOptions, MemoryStore},
    memory_backend::BackendEmbedder,
    opencode::NativeOpenCodeRunner,
    process_control::minimal_env,
    state::{Config, StateStore},
};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

pub struct NativeChatOptions {
    pub provider: String,
    pub model: Option<String>,
    pub message: String,
    pub agent: Option<String>,
    pub skills: Vec<String>,
    pub capture: bool,
}
pub fn timestamp() -> Result<String, HarnessError> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| HarnessError::Invalid)
}

fn local_model(
    requested: Option<&str>,
    active: &crate::state::ServerState,
    catalog: &rigspark_core::catalog::Catalog,
) -> Result<(String, String), HarnessError> {
    let runtime_id = active
        .runtime_model_id
        .as_deref()
        .unwrap_or(&active.model_id);
    let Some(requested) = requested else {
        return Ok((runtime_id.into(), active.model_id.clone()));
    };
    if requested == active.model_id || requested == runtime_id {
        return Ok((runtime_id.into(), active.model_id.clone()));
    }
    let resolved =
        rigspark_core::catalog::resolve(catalog, requested).map_err(|_| HarnessError::Invalid)?;
    if active.backend != "ollama" && resolved.model.id != active.model_id {
        return Err(HarnessError::Invalid);
    }
    let runtime_id = if resolved.model.id == active.model_id {
        runtime_id.to_owned()
    } else {
        resolved
            .model
            .source
            .ollama
            .clone()
            .ok_or(HarnessError::Invalid)?
    };
    Ok((runtime_id, resolved.model.id.clone()))
}
pub async fn run(
    options: &NativeChatOptions,
    cancel: &CancellationToken,
    sink: &mut DeltaSink<'_>,
) -> Result<serde_json::Value, HarnessError> {
    run_with_history(options, None, None, cancel, sink).await
}
pub async fn run_with_history(
    options: &NativeChatOptions,
    history: Option<&[HarnessMessage]>,
    expected: Option<&crate::state::RuntimeState>,
    cancel: &CancellationToken,
    sink: &mut DeltaSink<'_>,
) -> Result<serde_json::Value, HarnessError> {
    if let Some(history) = history {
        if history.len() > 19
            || history
                .iter()
                .any(|message| !["user", "assistant"].contains(&message.role.as_str()))
        {
            return Err(HarnessError::Invalid);
        }
        HarnessRequest {
            model: String::new(),
            messages: history.to_vec(),
            temperature: None,
        }
        .validate()?;
    }
    if options.message.trim().is_empty() || options.message.len() > 1024 * 1024 {
        return Err(HarnessError::Invalid);
    }
    let config = Config::load().map_err(|_| HarnessError::Invalid)?;
    let state = StateStore::new(config.clone());
    let current = state.read().map_err(|_| HarnessError::Unavailable)?;
    if expected.is_some_and(|expected| expected != &current) {
        return Err(HarnessError::Drift);
    }
    let active = current.active;
    let runtime = crate::native_runtime::NativeRuntime::new(config.clone())?;
    let adapters = runtime.adapters();
    let backends = adapters.registry();
    let probe = &runtime.probe;
    let remote = NativeRemoteTransport::new()?;
    let mut env: BTreeMap<String, String> = minimal_env().into_iter().collect();
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
    let opencode = NativeOpenCodeRunner {
        binary: runtime.binary("opencode"),
        env: env.clone(),
    };
    let registry = HarnessRegistry::builtins(
        LocalHarness {
            state: &state,
            registry: &backends,
            probe,
        },
        &remote,
        &opencode,
        &env,
    )?;
    let harness = registry.get(&options.provider)?;
    if !harness.available().await {
        return Err(HarnessError::Unavailable);
    }
    let (model, memory_owner) = if options.provider == "local" {
        let loaded = crate::catalog_update::CatalogStore::official(&config.home)
            .load()
            .map_err(|_| HarnessError::Invalid)?;
        local_model(
            options.model.as_deref(),
            active.as_ref().ok_or(HarnessError::Unavailable)?,
            &loaded.catalog,
        )?
    } else {
        (options.model.clone().unwrap_or_default(), String::new())
    };
    let mut messages = Vec::new();
    if let Some(prompt) = Library::new(&config.home)
        .compose(options.agent.as_deref(), &options.skills)
        .map_err(|_| HarnessError::Invalid)?
    {
        messages.push(HarnessMessage {
            role: "system".into(),
            content: prompt,
        });
    }
    let memory = if options.provider == "local" && options.capture {
        let memory = MemoryStore::open(&config.home, &memory_owner, &timestamp()?)
            .map_err(|_| HarnessError::Invalid)?;
        let source = memory.load().map_err(|_| HarnessError::Invalid)?;
        if let Some(persona) = source.system_prompt {
            messages.push(HarnessMessage {
                role: "system".into(),
                content: persona,
            });
        }
        if source.facts_present {
            messages.push(HarnessMessage {
                role: "system".into(),
                content: source.facts_text,
            });
        }
        if history.is_none() {
            messages.extend(source.turns.into_iter().map(|turn| HarnessMessage {
                role: turn.role,
                content: turn.content,
            }));
        }
        Some(memory)
    } else {
        None
    };
    if let Some(history) = history {
        messages.extend_from_slice(history);
    }
    messages.push(HarnessMessage {
        role: "user".into(),
        content: options.message.clone(),
    });
    let mut emitted_bytes = 0usize;
    let mut bounded_sink = |text: &str| {
        emitted_bytes = emitted_bytes.saturating_add(text.len());
        if history.is_some() && emitted_bytes > 1024 * 1024 {
            return Err(HarnessError::Limit);
        }
        sink(text)
    };
    let response = harness
        .chat(
            &HarnessRequest {
                model,
                messages,
                temperature: None,
            },
            cancel,
            &mut bounded_sink,
        )
        .await?;
    if history.is_some() && response.len() > 1024 * 1024 {
        return Err(HarnessError::Limit);
    }
    if cancel.is_cancelled() {
        return Err(HarnessError::Cancelled);
    }
    let mut captured = None;
    let mut embedding_skipped = None;
    if let Some(memory) = memory {
        let backend = active
            .as_ref()
            .ok_or(HarnessError::Unavailable)?
            .backend
            .as_str();
        let unsupported = !backends
            .get(backend)
            .map_err(|_| HarnessError::Unavailable)?
            .can_embed();
        let embedder = BackendEmbedder {
            state: &state,
            registry: &backends,
            probe,
            model: memory
                .read_meta()
                .map_err(|_| HarnessError::Invalid)?
                .embedding
                .map(|meta| meta.model)
                .unwrap_or_else(|| "nomic-embed-text".into()),
        };
        let outcome = memory
            .capture(
                &options.message,
                &response,
                CaptureOptions {
                    timestamp: &timestamp()?,
                    embedder: (!unsupported).then_some(&embedder as &dyn crate::memory::Embedder),
                    embedding_unsupported: unsupported,
                },
                cancel,
            )
            .await;
        embedding_skipped = outcome
            .as_ref()
            .ok()
            .and_then(|result| result.embedding_skipped.clone());
        captured = Some(outcome.is_ok());
    }
    let mut report = serde_json::json!({"content":response,"harness":options.provider,"memoryCaptured":captured});
    if let Some(reason) = embedding_skipped {
        report["embeddingSkipped"] = serde_json::json!(reason);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_model(
        requested: Option<&str>,
        active: &crate::state::ServerState,
    ) -> Result<(String, String), HarnessError> {
        let catalog = rigspark_core::catalog::Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
        super::local_model(requested, active, &catalog)
    }

    fn active(backend: &str) -> crate::state::ServerState {
        serde_json::from_value(serde_json::json!({
            "backend": backend, "modelId": "llama3.1:8b", "endpoint": "http://127.0.0.1:11434",
            "port": 11434, "ownedByUs": false
        }))
        .unwrap()
    }

    #[test]
    fn local_cli_model_resolves_fuzzy_ids_and_keeps_memory_with_selected_model() {
        let current = active("ollama");
        assert_eq!(
            local_model(Some("llama3.1:8"), &current).unwrap(),
            ("llama3.1:8b".into(), "llama3.1:8b".into())
        );
        assert!(local_model(Some("llama3.1"), &current).is_err());
        assert_eq!(
            local_model(Some("qwen2.5:7b"), &current).unwrap(),
            ("qwen2.5:7b".into(), "qwen2.5:7b".into())
        );
        assert!(local_model(Some("unknown-fixture-model"), &current).is_err());
    }

    #[test]
    fn installed_context_variants_do_not_require_catalog_resolution() {
        let mut current = active("ollama");
        current.model_id = "uncatalogued:local".into();
        current.runtime_model_id = Some("llmup-context-test:65536".into());
        for requested in [
            None,
            Some("uncatalogued:local"),
            Some("llmup-context-test:65536"),
        ] {
            assert_eq!(
                local_model(requested, &current).unwrap(),
                (
                    "llmup-context-test:65536".into(),
                    "uncatalogued:local".into()
                )
            );
        }
    }

    #[test]
    fn single_model_backends_reject_other_models_before_inference() {
        for backend in ["mlx", "llamacpp", "lmstudio"] {
            assert!(local_model(Some("qwen2.5:7b"), &active(backend)).is_err());
            assert!(local_model(Some("llama3.1:8"), &active(backend)).is_ok());
        }
    }
}
