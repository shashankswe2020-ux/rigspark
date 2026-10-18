use rigspark_core::{
    advice::{decode_bytes, throughput},
    bootstrap::{BOOTSTRAP_CLOCK, build_catalog},
    catalog::{Catalog, PerfDataset, resolve},
    enrich::parse_candidates,
    ranking::backends,
    sizing::{Hardware, SizingRequest, evaluate},
};
use serde_json::json;

fn hardware() -> Hardware {
    serde_json::from_value(json!({
        "arch": "x64", "platform": "linux",
        "totalRamBytes": 17179869184_u64, "freeRamBytes": 12000000000_u64,
        "freeDiskBytes": 500000000000_u64,
        "gpu": [{"vendor": "nvidia", "vramBytes": 25769803776_u64}]
    }))
    .unwrap()
}

#[test]
fn official_bitnet_is_pinned_and_survives_bootstrap() {
    let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    let model = resolve(&catalog, "bitnet-b1.58:2b").unwrap().model;
    assert_eq!(model.params, "2.4B");
    assert_eq!(model.context_length, Some(4096.0));
    assert_eq!(model.license, "mit");
    assert_eq!(model.release_date.as_deref(), Some("2025-04-14"));
    assert_eq!(model.capabilities, ["chat"]);
    assert!(model.kv_bytes_per_token.is_none());
    assert!(model.benchmark_proxy.is_none());
    assert!(model.source.ollama.is_none());
    assert!(model.source.mlx.is_none());
    let source = model.source.gguf.as_ref().unwrap();
    assert_eq!(source.repo, "microsoft/bitnet-b1.58-2B-4T-gguf");
    assert_eq!(source.revision, "a1f2f1c765812aa8af3f6eda4a313707064bba15");
    assert_eq!(source.file, "ggml-model-i2_s.gguf");
    assert_eq!(
        source.sha256,
        "4221b252fdd5fd25e15847adfeb5ee88886506ba50b8a34548374492884c2162"
    );
    assert_eq!(model.quantizations.len(), 1);
    let quant = &model.quantizations[0];
    assert_eq!(quant.name, "I2_S");
    assert_eq!(quant.disk_bytes, 1_187_801_280.0);
    assert_eq!(quant.min_ram_bytes, 1_365_971_472.0);
    assert_eq!(
        quant.min_vram_bytes, 0.0,
        "CPU-only weights do not require VRAM"
    );
    assert_eq!(quant.sha256.as_deref(), Some(source.sha256.as_str()));
    let candidates = parse_candidates(rigspark_core::REGISTRY_SNAPSHOT_JSON).unwrap();
    let rebuilt = build_catalog(&candidates, BOOTSTRAP_CLOCK).unwrap();
    assert_eq!(
        serde_json::to_value(model).unwrap(),
        serde_json::to_value(resolve(&rebuilt, &model.id).unwrap().model).unwrap()
    );
}

#[test]
fn i2_s_is_bitnet_only_and_never_gets_generic_speed_estimates() {
    let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    let model = resolve(&catalog, "bitnet-b1.58:2b").unwrap().model;
    assert_eq!(backends(model, &hardware()), ["bitnet"]);
    let perf = PerfDataset::parse(rigspark_core::PERF_JSON).unwrap();
    for backend in rigspark_core::catalog::BACKENDS {
        assert!(
            !throughput(model, &model.quantizations[0], &hardware(), &perf, backend)
                .unwrap()
                .known
        );
    }
    assert_eq!(decode_bytes(model, &model.quantizations[0]).unwrap(), None);
}

#[test]
fn cpu_only_bitnet_sizes_against_ram_not_discrete_vram() {
    let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    let model = resolve(&catalog, "bitnet-b1.58:2b").unwrap().model;
    let mut request = SizingRequest {
        model: model.sizing(),
        hardware: hardware(),
        context: None,
    };
    let result = evaluate(&request).unwrap();
    assert_eq!(result.memory_kind, "ram");
    assert!(result.fit.fits);
    request.hardware.free_ram_bytes = 1_000_000_000.0;
    assert!(!evaluate(&request).unwrap().fit.fits);
}

#[test]
fn bitnet_execution_plan_only_offers_cpu_even_with_discrete_or_unified_gpus() {
    let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    let model = resolve(&catalog, "bitnet-b1.58:2b").unwrap().model;
    let perf = PerfDataset::parse(rigspark_core::PERF_JSON).unwrap();
    for unified in [false, true] {
        let mut hardware = hardware();
        hardware.unified_memory = Some(unified);
        let plan = rigspark_core::plan::plan(model, &hardware, &perf, None, "bitnet").unwrap();
        assert_eq!(plan.paths.len(), 1);
        assert_eq!(plan.paths[0].path, "cpu");
        assert_eq!(plan.recommended, Some("cpu"));
        assert!(!plan.paths[0].throughput.known);
    }
}
