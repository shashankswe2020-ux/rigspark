use rigspark_runtime::application::LifecycleOptions;

#[tokio::test]
async fn switch_without_active_server_fails_before_hardware_or_runtime_work() {
    use rigspark_core::catalog::Catalog;
    use rigspark_runtime::{application::run_native_with_config, state::Config};
    let home = tempfile::tempdir().unwrap();
    let config = Config::from_home(home.path()).unwrap();
    let catalog = Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap();
    let options = LifecycleOptions {
        command: "switch".into(),
        model: Some("qwen2.5:7b".into()),
        backend: None,
        port: None,
        context: None,
        installed: false,
        bypass: false,
        cache: Default::default(),
    };
    let error = run_native_with_config(
        &options,
        &catalog,
        None,
        &tokio_util::sync::CancellationToken::new(),
        config,
    )
    .await
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "no active server to switch; run up first"
    );
    assert!(home.path().read_dir().unwrap().next().is_none());
}

#[test]
fn down_accepts_optional_model_but_rejects_selection_options() {
    let mut options = LifecycleOptions {
        command: "down".into(),
        model: Some("llama3.1".into()),
        backend: None,
        port: None,
        context: None,
        installed: false,
        bypass: false,
        cache: Default::default(),
    };
    options.validate().unwrap();
    for query in ["", " ", "bad\nmodel", &"x".repeat(8193)] {
        options.model = Some(query.into());
        assert!(options.validate().is_err());
    }
    options.model = None;
    options.validate().unwrap();
    options.backend = Some("ollama".into());
    assert!(options.validate().is_err());
    options.backend = None;
    options.port = Some(11435);
    assert!(options.validate().is_err());
    options.port = None;
    options.context = Some(8192);
    assert!(options.validate().is_err());
    options.context = None;
    options.installed = true;
    assert!(options.validate().is_err());
    options.installed = false;
    options.bypass = true;
    assert!(options.validate().is_err());
    options.bypass = false;
    options.command = "doctor".into();
    options.model = Some("llama3.1".into());
    assert!(options.validate().is_err());
}

#[test]
fn lifecycle_options_reject_invalid_or_unsupported_combinations() {
    let mut options = LifecycleOptions {
        command: "up".into(),
        model: Some("test:latest".into()),
        backend: Some("ollama".into()),
        port: Some(11435),
        context: Some(8192),
        installed: false,
        bypass: false,
        cache: Default::default(),
    };
    options.validate().unwrap();
    options.port = Some(0);
    assert!(options.validate().is_err());
    options.port = Some(11435);
    options.context = Some(0);
    assert!(options.validate().is_err());
    options.context = None;
    options.installed = true;
    assert!(options.validate().is_err());
    options.bypass = true;
    options.validate().unwrap();
    options.backend = Some("unknown".into());
    assert!(options.validate().is_err());
}

#[test]
fn activation_request_boundaries_and_installed_dependencies_match_native_contract() {
    for command in ["up", "switch"] {
        for (port, context) in [(1, 1), (65535, 10000000)] {
            for backend in rigspark_core::catalog::BACKENDS {
                let mut options = LifecycleOptions {
                    command: command.into(),
                    model: Some("local:latest".into()),
                    backend: Some(backend.into()),
                    port: Some(port),
                    context: Some(context),
                    installed: false,
                    bypass: false,
                    cache: Default::default(),
                };
                options.validate().unwrap();
                options.context = Some(10000001);
                assert!(options.validate().is_err());
                options.context = Some(context);
                options.installed = true;
                assert!(options.validate().is_err());
                options.bypass = true;
                assert_eq!(options.validate().is_ok(), backend == "ollama");
                options.backend = None;
                options.validate().unwrap();
            }
        }
    }
}

#[tokio::test]
async fn invalid_activation_requests_fail_before_reading_corrupt_state() {
    use rigspark_core::catalog::Catalog;
    use rigspark_runtime::{application::run_native_with_config, state::Config};
    use tokio_util::sync::CancellationToken;
    let catalog = Catalog {
        schema_version: 1,
        generated_at: "2026-01-01T00:00:00Z".into(),
        models: vec![],
    };
    let home = tempfile::tempdir().unwrap();
    let config = Config::from_home(home.path()).unwrap();
    std::fs::write(&config.state, b"invalid state").unwrap();
    for command in ["up", "switch"] {
        for (port, context, installed, bypass, backend) in [
            (Some(0), None, false, false, "ollama"),
            (None, Some(0), false, false, "ollama"),
            (None, Some(10000001), false, false, "ollama"),
            (None, None, true, false, "ollama"),
            (None, None, true, true, "mlx"),
            (None, None, false, false, "unknown"),
        ] {
            let options = LifecycleOptions {
                command: command.into(),
                model: Some("local:latest".into()),
                backend: Some(backend.into()),
                port,
                context,
                installed,
                bypass,
                cache: Default::default(),
            };
            let expected = options.validate().unwrap_err().to_string();
            assert_eq!(
                run_native_with_config(
                    &options,
                    &catalog,
                    None,
                    &CancellationToken::new(),
                    config.clone()
                )
                .await
                .unwrap_err()
                .to_string(),
                expected,
            );
            assert_eq!(std::fs::read(&config.state).unwrap(), b"invalid state");
            assert_eq!(home.path().read_dir().unwrap().count(), 1);
        }
    }
}

#[test]
fn mlx_platform_gate_is_applied_before_preparation() {
    use rigspark_core::sizing::{CpuArch, Platform};
    assert!(
        rigspark_runtime::application::validate_backend_platform(
            "mlx",
            Platform::Linux,
            CpuArch::X64
        )
        .is_err()
    );
    assert!(
        rigspark_runtime::application::validate_backend_platform(
            "mlx",
            Platform::Darwin,
            CpuArch::X64
        )
        .is_err()
    );
    rigspark_runtime::application::validate_backend_platform(
        "mlx",
        Platform::Darwin,
        CpuArch::Arm64,
    )
    .unwrap();
    rigspark_runtime::application::validate_backend_platform(
        "ollama",
        Platform::Linux,
        CpuArch::X64,
    )
    .unwrap();
}

#[test]
fn catalog_quantization_suffixes_reach_the_resolver() {
    let options = LifecycleOptions {
        command: "up".into(),
        model: Some("llama3.1:8b-Q4_K_M".into()),
        backend: Some("ollama".into()),
        port: None,
        context: None,
        installed: false,
        bypass: false,
        cache: Default::default(),
    };
    options.validate().unwrap();
}

fn down_options(model: Option<&str>) -> LifecycleOptions {
    LifecycleOptions {
        command: "down".into(),
        model: model.map(str::to_owned),
        backend: None,
        port: None,
        context: None,
        installed: false,
        bypass: false,
        cache: Default::default(),
    }
}

#[tokio::test]
async fn switch_rejects_backend_override_before_already_active_or_preparation() {
    use rigspark_core::catalog::Catalog;
    use rigspark_runtime::{
        application::{
            events::{DiagnosticObserver, LifecycleObserver},
            run_native_with_config_observed,
        },
        state::{Config, RuntimeState, StateStore},
    };
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;
    let catalog: Catalog =
        serde_json::from_str(include_str!("../../rigspark-core/data/models.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let mut prior = RuntimeState::parse(r#"{"schemaVersion":2,"active":{"backend":"ollama","modelId":"placeholder","endpoint":"http://127.0.0.1:11435","port":11435,"ownedByUs":false}}"#).unwrap();
    prior.active.as_mut().unwrap().model_id = catalog.models[0].id.clone();
    for active_backend in rigspark_core::catalog::BACKENDS {
        let active = prior.active.as_mut().unwrap();
        active.backend = active_backend.into();
        active.owned_by_us = active_backend != "lmstudio";
        active.pid = Some(123);
        active.process_executable = Some(format!("/trusted/{active_backend}"));
        active.process_started_at = Some("instance".into());
        active.auth_token = (active_backend == "mlx").then(|| "a".repeat(64));
        active.model_path = ["lmstudio", "bitnet"]
            .contains(&active_backend)
            .then(|| "/models/fixture".into());
        active.context = (active_backend == "bitnet").then_some(4096);
        let guard = store.lock(Duration::from_millis(10)).unwrap();
        store.write(&guard, &prior).unwrap();
        guard.release().unwrap();
        let bytes = std::fs::read(&store.config.state).unwrap();
        for backend in rigspark_core::catalog::BACKENDS {
            if backend == active_backend {
                continue;
            }
            for (installed, bypass, context) in [
                (false, false, None),
                (false, true, None),
                (false, false, Some(8192)),
                (true, true, Some(8192)),
            ] {
                if installed && backend != "ollama" {
                    continue;
                }
                let options = LifecycleOptions {
                    command: "switch".into(),
                    model: Some(catalog.models[0].id.clone()),
                    backend: Some(backend.into()),
                    port: Some(11435),
                    context,
                    installed,
                    bypass,
                    cache: Default::default(),
                };
                let diagnostics = DiagnosticObserver::default();
                let (observer, mut receiver) = LifecycleObserver::channel();
                let observer = observer.with_diagnostics(diagnostics.clone());
                let error = run_native_with_config_observed(
                    &options,
                    &catalog,
                    None,
                    &CancellationToken::new(),
                    store.config.clone(),
                    Some(&observer),
                )
                .await
                .unwrap_err();
                assert_eq!(
                    error.to_string(),
                    format!(
                        "switch cannot change the active backend from {active_backend} to {backend}; use up --backend {backend}"
                    )
                );
                assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
                assert!(!store.config.lock.exists());
                assert!(receiver.try_recv().is_err());
                assert!(diagnostics.snapshot().text().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn switch_matching_or_implicit_backend_preserves_already_active_state() {
    use rigspark_core::catalog::Catalog;
    use rigspark_runtime::{
        application::run_native_with_config,
        state::{Config, RuntimeState, StateStore},
    };
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;
    let catalog: Catalog =
        serde_json::from_str(include_str!("../../rigspark-core/data/models.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let mut prior = RuntimeState::parse(r#"{"schemaVersion":2,"active":{"backend":"ollama","modelId":"placeholder","endpoint":"http://127.0.0.1:11435","port":11435,"ownedByUs":true,"pid":123,"runtimeModelId":"llmup-context-fixture:8192","context":8192}}"#).unwrap();
    prior.active.as_mut().unwrap().model_id = catalog.models[0].id.clone();
    for backend in rigspark_core::catalog::BACKENDS {
        let active = prior.active.as_mut().unwrap();
        active.backend = backend.into();
        active.owned_by_us = backend != "lmstudio";
        active.process_executable = Some(format!("/trusted/{backend}"));
        active.process_started_at = Some("instance".into());
        active.auth_token = (backend == "mlx").then(|| "a".repeat(64));
        active.model_path = ["lmstudio", "bitnet"]
            .contains(&backend)
            .then(|| "/models/fixture".into());
        active.context = Some(if backend == "bitnet" { 4096 } else { 8192 });
        let guard = store.lock(Duration::from_millis(10)).unwrap();
        store.write(&guard, &prior).unwrap();
        guard.release().unwrap();
        let bytes = std::fs::read(&store.config.state).unwrap();
        for explicit in [false, true] {
            let options = LifecycleOptions {
                command: "switch".into(),
                model: Some(catalog.models[0].id.clone()),
                backend: explicit.then(|| backend.into()),
                port: Some(11435),
                context: None,
                installed: false,
                bypass: false,
                cache: Default::default(),
            };
            let (value, text) = run_native_with_config(
                &options,
                &catalog,
                None,
                &CancellationToken::new(),
                store.config.clone(),
            )
            .await
            .unwrap();
            assert_eq!(
                value,
                serde_json::json!({
                    "type": "already-active",
                    "modelId": catalog.models[0].id,
                    "endpoint": "http://127.0.0.1:11435",
                })
            );
            assert_eq!(
                text,
                format!("{} is already active.\n", catalog.models[0].id)
            );
            assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
            assert!(!store.config.lock.exists());
        }
    }
}

#[tokio::test]
async fn switch_pointer_rejects_port_override_before_already_active_or_preparation() {
    use rigspark_core::catalog::Catalog;
    use rigspark_runtime::{
        application::run_native_with_config,
        state::{Config, RuntimeState, StateStore},
    };
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;
    let catalog: Catalog =
        serde_json::from_str(include_str!("../../rigspark-core/data/models.json")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let mut prior = RuntimeState::parse(r#"{"schemaVersion":2,"active":{"backend":"ollama","modelId":"placeholder","endpoint":"http://127.0.0.1:11435","port":11435,"ownedByUs":false}}"#).unwrap();
    prior.active.as_mut().unwrap().model_id = catalog.models[0].id.clone();
    let guard = store.lock(Duration::from_millis(10)).unwrap();
    store.write(&guard, &prior).unwrap();
    guard.release().unwrap();
    let bytes = std::fs::read(&store.config.state).unwrap();
    for model in [&catalog.models[0].id, &catalog.models[1].id] {
        let mut options = LifecycleOptions {
            command: "switch".into(),
            model: Some(model.clone()),
            backend: Some("ollama".into()),
            port: Some(11436),
            context: None,
            installed: false,
            bypass: false,
            cache: Default::default(),
        };
        let error = run_native_with_config(
            &options,
            &catalog,
            None,
            &CancellationToken::new(),
            store.config.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "switch without --context or --bypass cannot change the active port; use up --port 11436"
        );
        assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
        assert!(!store.config.lock.exists());
        for (context, bypass) in [(Some(8192), false), (None, true)] {
            options.context = context;
            options.bypass = bypass;
            let error = run_native_with_config(
                &options,
                &catalog,
                None,
                &CancellationToken::new(),
                store.config.clone(),
            )
            .await
            .unwrap_err();
            assert_eq!(error.to_string(), "hardware required for model selection");
            assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
            assert!(!store.config.lock.exists());
        }
    }
}

#[tokio::test]
async fn down_without_active_returns_no_active_even_for_unknown_model() {
    use rigspark_core::catalog::Catalog;
    use rigspark_runtime::{application::run_native_with_config, state::Config};
    use tokio_util::sync::CancellationToken;
    let catalog = Catalog {
        schema_version: 1,
        generated_at: "2026-01-01T00:00:00Z".into(),
        models: vec![],
    };
    let home = tempfile::tempdir().unwrap();
    let config = Config::from_home(home.path().join("absent")).unwrap();
    for model in [None, Some("unknown:latest")] {
        let (value, text) = run_native_with_config(
            &down_options(model),
            &catalog,
            None,
            &CancellationToken::new(),
            config.clone(),
        )
        .await
        .unwrap();
        assert_eq!(value, serde_json::json!({"type":"no-active"}));
        assert_eq!(text, "No active server to stop.\n");
        assert!(!config.state.exists());
        assert!(!config.lock.exists());
        assert!(!config.home.exists());
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        run_native_with_config(&down_options(None), &catalog, None, &cancel, config.clone())
            .await
            .unwrap_err()
            .to_string(),
        "cancelled"
    );
    assert!(!config.home.exists());
}

#[tokio::test]
async fn down_resolves_canonical_ids_and_preserves_state_on_mismatch_or_resolution_error() {
    use rigspark_core::catalog::Catalog;
    use rigspark_runtime::{
        application::run_native_with_config,
        state::{Config, RuntimeState, StateStore},
    };
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;
    let mut catalog: Catalog =
        serde_json::from_str(include_str!("../../rigspark-core/data/models.json")).unwrap();
    catalog.models.truncate(1);
    let model = &mut catalog.models[0];
    model.id = "llama3.1:8b".into();
    model.family = "llama3.1".into();
    let quant_query = format!("{}-{}", model.id, model.quantizations[0].name);
    let mut other = model.clone();
    other.id = "llama3.2:3b".into();
    other.family = "llama3.2".into();
    catalog.models.push(other);
    let home = tempfile::tempdir().unwrap();
    let store = StateStore::new(Config::from_home(home.path()).unwrap());
    let mut prior = RuntimeState::parse(r#"{"schemaVersion":2,"active":{"backend":"ollama","modelId":"unknown:latest","endpoint":"http://127.0.0.1:11435","port":11435,"ownedByUs":false,"pid":123,"processExecutable":"/trusted/ollama","processStartedAt":"instance"}}"#).unwrap();
    for owned in [false, true] {
        prior.active.as_mut().unwrap().owned_by_us = owned;
        let guard = store.lock(Duration::from_millis(10)).unwrap();
        store.write(&guard, &prior).unwrap();
        guard.release().unwrap();
        let bytes = std::fs::read(&store.config.state).unwrap();
        for (query, expected) in [
            (
                "llama3.1",
                "llama3.1:8b is not the active model (unknown:latest)",
            ),
            (
                " LLAMA3.1:8B ",
                "llama3.1:8b is not the active model (unknown:latest)",
            ),
            (
                quant_query.as_str(),
                "llama3.1:8b is not the active model (unknown:latest)",
            ),
            ("unknown:latest", "no model matches"),
            ("llama3", "ambiguous"),
        ] {
            let error = run_native_with_config(
                &down_options(Some(query)),
                &catalog,
                None,
                &CancellationToken::new(),
                store.config.clone(),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{query}: {error}");
            assert_eq!(std::fs::read(&store.config.state).unwrap(), bytes);
            assert!(!store.config.lock.exists());
        }
    }
}

#[test]
fn cache_options_apply_only_to_launches_rigspark_owns() {
    use rigspark_core::sizing::KvCacheType;
    use rigspark_runtime::cache::{CacheFlags, FlashAttention};
    let q8 = CacheFlags {
        kv: Some(KvCacheType::Q8_0),
        ..CacheFlags::default()
    };
    let mut options = LifecycleOptions {
        command: "up".into(),
        model: Some("test:latest".into()),
        backend: None,
        port: None,
        context: None,
        installed: false,
        bypass: false,
        cache: q8,
    };
    options.validate().unwrap();
    options.command = "switch".into();
    options.validate().unwrap();
    options.installed = true;
    options.bypass = true;
    let installed = options.validate().unwrap_err().to_string();
    assert!(installed.contains("--installed"), "{installed}");
    options.installed = false;
    options.bypass = false;
    options.cache.flash_attention = Some(FlashAttention::Off);
    let flash = options.validate().unwrap_err().to_string();
    assert!(flash.contains("flash attention"), "{flash}");
    options.cache = q8;
    for command in ["down", "doctor"] {
        options.command = command.into();
        options.model = None;
        let error = options.validate().unwrap_err().to_string();
        assert!(error.contains("does not accept cache options"), "{error}");
    }
}
