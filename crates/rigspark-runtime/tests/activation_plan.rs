use rigspark_core::{
    catalog::{Catalog, CatalogModel},
    sizing::{CpuArch, Hardware, Platform},
};
use rigspark_runtime::application::{
    PlanOptions, auto_backend_order, check_backend, plan_quantization, preferred_backend,
};

fn catalog() -> Catalog {
    Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap()
}

/// A catalog model carrying every source kind, so each backend is individually selectable.
fn universal() -> CatalogModel {
    let catalog = catalog();
    let mut model = catalog
        .models
        .iter()
        .find(|model| model.source.ollama.is_some())
        .unwrap()
        .clone();
    if model.quantizations.len() == 1 {
        let mut larger = model.quantizations[0].clone();
        larger.name = "Q8_0".into();
        larger.disk_bytes *= 2.0;
        larger.min_ram_bytes *= 2.0;
        larger.min_vram_bytes *= 2.0;
        model.quantizations.push(larger);
    }
    model.source.gguf = catalog
        .models
        .iter()
        .find_map(|model| model.source.gguf.clone());
    model.source.mlx = catalog
        .models
        .iter()
        .find_map(|model| model.source.mlx.clone());
    assert!(model.source.gguf.is_some() && model.source.mlx.is_some());
    model
}

fn hardware(platform: Platform, arch: CpuArch, ram: f64, disk: f64) -> Hardware {
    Hardware {
        arch,
        platform,
        total_ram_bytes: ram,
        free_ram_bytes: ram,
        free_disk_bytes: disk,
        gpu: vec![],
        unified_memory: None,
    }
}

fn roomy() -> Hardware {
    hardware(Platform::Linux, CpuArch::X64, 256e9, 4e12)
}

fn options(bypass: bool, context: Option<u32>) -> PlanOptions {
    PlanOptions {
        simple_switch: false,
        bypass,
        context,
        kv_cache: None,
    }
}

fn error(result: Result<impl std::fmt::Debug, rigspark_runtime::adapters::BackendError>) -> String {
    result.unwrap_err().0
}

#[test]
fn advisory_only_is_never_auto_selected_or_activated_even_with_bypass() {
    use rigspark_core::catalog::{AdvisoryReason, Availability};
    let mut model = universal();
    model.availability = Some(Availability::AdvisoryOnly {
        reason: AdvisoryReason::BackendFormatUnsupported,
    });
    assert!(auto_backend_order(&model, &roomy()).is_empty());
    for backend in ["ollama", "llamacpp", "mlx", "lmstudio"] {
        assert!(
            error(check_backend(&model, backend, &roomy(), true)).contains("not yet installable")
        );
        for bypass in [false, true] {
            assert!(
                error(plan_quantization(
                    &model,
                    None,
                    backend,
                    &roomy(),
                    options(bypass, None)
                ))
                .contains("not yet installable")
            );
        }
    }
}

#[tokio::test]
async fn advisory_activation_fails_before_state_access_or_installed_fallback() {
    use rigspark_core::catalog::{AdvisoryReason, Availability};
    use rigspark_runtime::{
        application::{LifecycleOptions, run_native_with_config},
        state::Config,
    };
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("uncreated-home");
    let mut config = Config::from_home(&home).unwrap();
    // If preflight reads state first, this invalid state file produces a different error.
    config.state = temp.path().join("invalid-state.json");
    std::fs::write(&config.state, "not a state file").unwrap();
    let mut model = universal();
    model.availability = Some(Availability::AdvisoryOnly {
        reason: AdvisoryReason::BackendFormatUnsupported,
    });
    let source_alias = model.source.ollama.clone().unwrap();
    model.id = "advisory:example".into();
    let mut catalog = catalog();
    catalog.models = vec![model];
    for command in ["up", "switch"] {
        for query in ["advisory:example", source_alias.as_str()] {
            for installed in [false, true] {
                let options = LifecycleOptions {
                    command: command.into(),
                    model: Some(query.into()),
                    backend: None,
                    port: None,
                    context: None,
                    installed,
                    bypass: installed,
                    cache: Default::default(),
                };
                let result = run_native_with_config(
                    &options,
                    &catalog,
                    None,
                    &tokio_util::sync::CancellationToken::new(),
                    config.clone(),
                )
                .await;
                assert!(
                    error(result).contains("not yet installable"),
                    "{command} {query}"
                );
                assert!(!home.exists());
                assert_eq!(
                    std::fs::read_to_string(&config.state).unwrap(),
                    "not a state file"
                );
            }
        }
    }
}

#[test]
fn backend_preference_is_flag_then_environment_then_user_config() {
    assert_eq!(
        preferred_backend(Some("mlx"), Some("llamacpp"), Some("lmstudio")).as_deref(),
        Some("mlx")
    );
    assert_eq!(
        preferred_backend(None, Some("llamacpp"), Some("lmstudio")).as_deref(),
        Some("llamacpp")
    );
    assert_eq!(
        preferred_backend(None, Some("  "), Some("lmstudio")).as_deref(),
        Some("lmstudio")
    );
    assert_eq!(preferred_backend(None, None, None), None);
}

#[test]
fn attach_intent_overrides_must_match_the_active_backend() {
    use rigspark_runtime::application::check_attach_override;
    check_attach_override("switch", "ollama", None, None).unwrap();
    check_attach_override("switch", "ollama", Some("ollama"), Some("ollama")).unwrap();
    check_attach_override("down", "ollama", None, Some("   ")).unwrap();
    let flag = check_attach_override("switch", "ollama", Some("mlx"), None)
        .unwrap_err()
        .0;
    assert!(flag.contains("from ollama to mlx"), "{flag}");
    for (flag, env) in [(None, "llamacpp"), (Some("ollama"), "llamacpp")] {
        let error = check_attach_override("switch", "ollama", flag, Some(env))
            .unwrap_err()
            .0;
        assert!(error.contains("RIGSPARK_BACKEND=llamacpp"), "{error}");
    }
    let hostile = check_attach_override("down", "ollama", None, Some("x\u{1b}[31m"))
        .unwrap_err()
        .0;
    assert!(!hostile.contains('\u{1b}'));
}

#[test]
fn auto_selection_prefers_mlx_only_on_apple_silicon_and_skips_incompatible_sources() {
    let model = universal();
    let apple = hardware(Platform::Darwin, CpuArch::Arm64, 64e9, 1e12);
    let intel_mac = hardware(Platform::Darwin, CpuArch::X64, 64e9, 1e12);
    assert_eq!(
        auto_backend_order(&model, &apple),
        ["mlx", "ollama", "llamacpp"]
    );
    assert_eq!(
        auto_backend_order(&model, &intel_mac),
        ["ollama", "llamacpp"]
    );
    assert_eq!(auto_backend_order(&model, &roomy()), ["ollama", "llamacpp"]);
    let mut gguf_only = model.clone();
    gguf_only.source.ollama = None;
    gguf_only.source.mlx = None;
    assert_eq!(auto_backend_order(&gguf_only, &apple), ["llamacpp"]);
    // LM Studio is attach-only: it serves only a model the user loaded, so it is never auto-selected.
    let mut studio_only = model.clone();
    studio_only.source.ollama = None;
    studio_only.source.gguf = None;
    assert!(auto_backend_order(&studio_only, &roomy()).is_empty());
}

#[test]
fn backend_gates_run_before_any_pull() {
    let model = universal();
    let linux = roomy();
    let apple = hardware(Platform::Darwin, CpuArch::Arm64, 64e9, 1e12);
    for backend in ["ollama", "llamacpp", "lmstudio"] {
        check_backend(&model, backend, &linux, true).unwrap();
    }
    check_backend(&model, "mlx", &apple, true).unwrap();
    assert!(error(check_backend(&model, "mlx", &linux, true)).contains("Apple Silicon"));
    let mut mlx_only = model.clone();
    mlx_only.source.gguf = None;
    mlx_only.source.ollama = None;
    assert!(error(check_backend(&mlx_only, "lmstudio", &linux, true)).contains("Apple Silicon"));
    check_backend(&mlx_only, "lmstudio", &apple, true).unwrap();
    assert!(error(check_backend(&model, "ollama", &linux, false)).contains("unavailable"));
    assert!(error(check_backend(&mlx_only, "llamacpp", &linux, true)).contains("unsupported"));
    assert_eq!(
        error(check_backend(&model, "vllm", &linux, true)),
        "invalid backend selection"
    );
}

#[test]
fn fitting_models_use_the_fit_quant_and_explicit_quants_are_honoured() {
    let model = universal();
    let plan = plan_quantization(&model, None, "ollama", &roomy(), options(false, None)).unwrap();
    assert!(!plan.estimated_fit);
    assert!(
        model
            .quantizations
            .iter()
            .any(|quant| quant.name == plan.quant.name)
    );
    let explicit = model.quantizations.last().unwrap();
    let plan = plan_quantization(
        &model,
        Some(explicit),
        "ollama",
        &roomy(),
        options(false, None),
    )
    .unwrap();
    assert_eq!(plan.quant.name, explicit.name);
    assert!(!plan.estimated_fit);
}

#[test]
fn unfit_models_require_bypass_which_picks_the_smallest_quant_and_warns() {
    let model = universal();
    let tiny = hardware(Platform::Linux, CpuArch::X64, 1e6, 4e12);
    assert!(
        error(plan_quantization(
            &model,
            None,
            "ollama",
            &tiny,
            options(false, None)
        ))
        .contains("--bypass")
    );
    let plan = plan_quantization(&model, None, "ollama", &tiny, options(true, None)).unwrap();
    let smallest = model
        .quantizations
        .iter()
        .min_by(|left, right| left.disk_bytes.total_cmp(&right.disk_bytes))
        .unwrap();
    assert_eq!(plan.quant.name, smallest.name);
    assert!(plan.estimated_fit);
}

#[test]
fn explicit_unfit_quants_warn_but_an_explicit_context_requires_bypass() {
    let model = universal();
    let tiny = hardware(Platform::Linux, CpuArch::X64, 1e6, 4e12);
    let explicit = &model.quantizations[0];
    let warned = plan_quantization(
        &model,
        Some(explicit),
        "ollama",
        &tiny,
        options(false, None),
    )
    .unwrap();
    assert!(warned.estimated_fit);
    assert!(
        error(plan_quantization(
            &model,
            Some(explicit),
            "ollama",
            &tiny,
            options(false, Some(65_536))
        ))
        .contains("requested context")
    );
    let bypassed = plan_quantization(
        &model,
        Some(explicit),
        "ollama",
        &tiny,
        options(true, Some(65_536)),
    )
    .unwrap();
    assert!(bypassed.estimated_fit);
}

#[test]
fn disk_preflight_uses_one_message_for_auto_and_explicit_quants_except_delegated_studio() {
    let model = universal();
    let full = hardware(Platform::Linux, CpuArch::X64, 256e9, 1.0);
    let auto = error(plan_quantization(
        &model,
        None,
        "ollama",
        &full,
        options(false, None),
    ));
    let explicit = error(plan_quantization(
        &model,
        Some(&model.quantizations[0]),
        "llamacpp",
        &full,
        options(false, None),
    ));
    assert_eq!(
        (auto.as_str(), explicit.as_str()),
        ("insufficient disk space", "insufficient disk space")
    );
    plan_quantization(&model, None, "lmstudio", &full, options(true, None)).unwrap();
    let bypass = error(plan_quantization(
        &model,
        None,
        "ollama",
        &full,
        options(true, None),
    ));
    assert_eq!(bypass, "insufficient disk space");
}

#[test]
fn explicit_context_and_pointer_switches_are_ollama_only() {
    let model = universal();
    for backend in ["llamacpp", "mlx", "lmstudio"] {
        assert!(
            error(plan_quantization(
                &model,
                None,
                backend,
                &roomy(),
                options(false, Some(8192))
            ))
            .contains("requires Ollama")
        );
        let switch = PlanOptions {
            simple_switch: true,
            bypass: false,
            context: None,
            kv_cache: None,
        };
        assert!(
            error(plan_quantization(&model, None, backend, &roomy(), switch))
                .contains("require up")
        );
    }
    let switch = PlanOptions {
        simple_switch: true,
        bypass: false,
        context: None,
        kv_cache: None,
    };
    let full = hardware(Platform::Linux, CpuArch::X64, 1e6, 1.0);
    let plan = plan_quantization(&model, None, "ollama", &full, switch).unwrap();
    assert_eq!(plan.quant.name, model.quantizations[0].name);
    assert!(!plan.estimated_fit);
}

#[test]
fn a_quantized_kv_cache_fits_a_long_context_where_f16_does_not() {
    use rigspark_core::sizing::KvCacheType;
    let model = catalog()
        .models
        .into_iter()
        .find(|model| {
            model.source.ollama.is_some()
                && model.kv_bytes_per_token.is_some()
                && model.context_length >= 32768.0
        })
        .unwrap();
    let fits = |kv_cache, ram: f64| {
        let options = PlanOptions {
            kv_cache,
            ..options(false, Some(32768))
        };
        plan_quantization(
            &model,
            None,
            "ollama",
            &hardware(Platform::Linux, CpuArch::X64, ram, 4e12),
            options,
        )
        .is_ok()
    };
    let smallest = (1..=20_000)
        .map(|step| f64::from(step) * 50e6)
        .find(|ram| fits(Some(KvCacheType::Q4_0), *ram))
        .expect("q4_0 fits within 1 TB");
    assert!(!fits(None, smallest), "f16 must need more memory than q4_0");
    assert!(!fits(Some(KvCacheType::Q8_0), smallest));
    assert_eq!(fits(Some(KvCacheType::F16), smallest), fits(None, smallest));
}
