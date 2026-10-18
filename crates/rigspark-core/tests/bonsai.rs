use rigspark_core::catalog::Catalog;

#[test]
fn bonsai_8b_pins_official_binary_gguf_without_claiming_other_runtimes() {
    let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    let model = catalog
        .models
        .iter()
        .find(|model| model.id == "bonsai:8b")
        .expect("Bonsai 8B is curated");
    assert_eq!(model.params, "8.19B");
    assert_eq!(model.context_length, Some(65536.0));
    assert_eq!(model.license, "apache-2.0");
    assert!(model.source.ollama.is_none());
    assert!(model.source.mlx.is_none());
    assert!(model.benchmark_proxy.is_none());
    assert!(model.kv_bytes_per_token.is_none());
    let source = model.source.gguf.as_ref().expect("pinned GGUF source");
    assert_eq!(source.repo, "prism-ml/Bonsai-8B-gguf");
    assert_eq!(source.revision, "48516770dd04643643e9f9019a2a349cf26c5dbd");
    assert_eq!(source.file, "Bonsai-8B-Q1_0.gguf");
    assert_eq!(
        source.sha256,
        "284a335aa3fb2ced3b1b01fcb40b08aa783e3b70832767f0dd2e3fdfa134bd54"
    );
    let quant = &model.quantizations[0];
    assert_eq!(quant.name, "Q1_0");
    assert_eq!(quant.disk_bytes, 1_158_654_496.0);
    assert_eq!(quant.min_ram_bytes, 1_332_452_671.0);
    assert_eq!(quant.min_vram_bytes, quant.min_ram_bytes);
    assert!(quant.projectors.is_empty());
}

#[test]
fn bonsai_binary_kernels_do_not_use_generic_throughput_efficiency() {
    let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    let model = catalog
        .models
        .iter()
        .find(|model| model.id == "bonsai:8b")
        .unwrap();
    let hardware = serde_json::from_value(serde_json::json!({"arch":"arm64","platform":"darwin","totalRamBytes":38654705664_u64,"freeRamBytes":30000000000_u64,"freeDiskBytes":500000000000_u64,"gpu":[{"vendor":"apple","vramBytes":0}]})).unwrap();
    let perf = rigspark_core::catalog::PerfDataset::parse(rigspark_core::PERF_JSON).unwrap();
    for backend in ["ollama", "llamacpp", "lmstudio"] {
        let estimate = rigspark_core::advice::throughput(
            model,
            &model.quantizations[0],
            &hardware,
            &perf,
            backend,
        )
        .unwrap();
        assert!(!estimate.known, "uncalibrated Q1_0 estimate for {backend}");
    }
}
