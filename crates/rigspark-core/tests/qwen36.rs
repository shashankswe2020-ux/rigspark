use rigspark_core::{bootstrap::build_catalog, catalog::Catalog, enrich::parse_candidates};

#[test]
fn qwen36_preserves_projector_integrity_and_unknown_hybrid_geometry() {
    let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    let model = catalog
        .models
        .iter()
        .find(|model| model.id == "qwen3.6:35b")
        .expect("Qwen 3.6 is curated");
    assert_eq!(model.active_params.as_deref(), Some("3B"));
    assert_eq!(model.context_length, Some(262144.0));
    assert!(model.kv_bytes_per_token.is_none());
    assert!(model.benchmark_proxy.is_none());
    let quant = &model.quantizations[0];
    assert_eq!(quant.disk_bytes, 22_621_302_688.0);
    assert_eq!(quant.projectors.len(), 1);
    assert_eq!(quant.projectors[0].bytes, 902_821_728);
    assert_eq!(
        quant.sha256.as_deref(),
        Some("d372de8e934898a59e6ccfabc3368474711384d8f1fd4d22d87a3f0a45400cdc")
    );
    assert_eq!(
        quant.projectors[0].sha256,
        "a62390d25b4b4a2d8afd7cc3c90021c11935c1fdd48d7cb0723ab71cd02a598e"
    );
    let candidates = parse_candidates(rigspark_core::REGISTRY_SNAPSHOT_JSON).unwrap();
    let candidate = candidates
        .into_iter()
        .find(|model| model.id == "qwen3.6:35b")
        .unwrap();
    let rebuilt = build_catalog(&[candidate], "2026-09-30T00:00:00Z").unwrap();
    assert_eq!(
        serde_json::to_value(model).unwrap(),
        serde_json::to_value(&rebuilt.models[0]).unwrap()
    );
}

#[test]
fn bootstrap_adds_projectors_after_the_model_parameter_floor() {
    let candidates = parse_candidates(rigspark_core::REGISTRY_SNAPSHOT_JSON).unwrap();
    let mut candidate = candidates
        .into_iter()
        .find(|model| model.id == "qwen3.6:35b")
        .unwrap();
    candidate.quantizations[0].disk_bytes = 1_000_000_000.0;
    let rebuilt = build_catalog(&[candidate], "2026-09-30T00:00:00Z").unwrap();
    let model_floor = (35e9 * rigspark_core::sizing::quant_bits("Q4_K_M").unwrap() / 8.0).ceil();
    let resident = model_floor + 902_821_728.0;
    assert_eq!(
        rebuilt.models[0].quantizations[0].min_ram_bytes,
        resident + (resident * 0.15).ceil()
    );
}
