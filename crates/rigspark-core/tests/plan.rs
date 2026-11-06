use rigspark_core::{
    catalog::{Catalog, PerfDataset},
    plan::plan,
    sizing::Hardware,
};
use serde_json::{Value, json};

const GIB: u64 = 1024 * 1024 * 1024;

fn data() -> (Catalog, PerfDataset) {
    (
        Catalog::parse(rigspark_core::MODELS_JSON).unwrap(),
        PerfDataset::parse(rigspark_core::PERF_JSON).unwrap(),
    )
}

fn hardware(value: Value) -> Hardware {
    serde_json::from_value(value).unwrap()
}

fn paths(catalog: &Catalog, perf: &PerfDataset, id: &str, hardware: &Hardware) -> Value {
    let model = catalog.models.iter().find(|model| model.id == id).unwrap();
    serde_json::to_value(plan(model, hardware, perf, None, "llamacpp").unwrap()).unwrap()
}

#[test]
fn two_gpus_pool_vram_for_a_model_one_gpu_cannot_hold_without_inventing_speed() {
    let (catalog, perf) = data();
    let rig = hardware(json!({
        "arch": "x64", "platform": "linux",
        "totalRamBytes": 64 * GIB, "freeRamBytes": 60 * GIB, "freeDiskBytes": 500 * GIB,
        "gpu": [{"vendor": "nvidia", "vramBytes": 24 * GIB}, {"vendor": "nvidia", "vramBytes": 24 * GIB}]
    }));
    let result = paths(&catalog, &perf, "mixtral:8x7b", &rig);
    let by_path = |name: &str| {
        result["paths"]
            .as_array()
            .unwrap()
            .iter()
            .find(|path| path["path"] == name)
            .cloned()
            .unwrap()
    };
    let single = by_path("gpu");
    assert_eq!(single["fits"], false);
    assert_eq!(single["reason"], "vram-bound");
    assert!(single["shortfallBytes"].as_f64().unwrap() > 0.0);
    let pooled = by_path("multi-gpu");
    assert_eq!(pooled["fits"], true);
    assert_eq!(pooled["gpuCount"], 2);
    assert_eq!(pooled["usableBytes"], json!((48 * GIB) as f64));
    assert_eq!(pooled["throughput"]["known"], false);
    assert!(
        pooled["throughputBasis"]
            .as_str()
            .unwrap()
            .starts_with("unknown")
    );
    let offload = by_path("cpu-offload");
    assert_eq!(offload["fits"], true);
    assert_eq!(offload["throughput"]["known"], false);
    assert_eq!(by_path("cpu")["throughput"]["known"], true);
    assert_eq!(result["recommended"], "multi-gpu");
    assert_eq!(result["unifiedMemory"], false);
}

#[test]
fn single_gpu_estimates_come_only_from_sourced_classes() {
    let (catalog, perf) = data();
    let desktop = hardware(json!({
        "arch": "x64", "platform": "linux",
        "totalRamBytes": 32 * GIB, "freeRamBytes": 28 * GIB, "freeDiskBytes": 500 * GIB,
        "gpu": [{"vendor": "nvidia", "vramBytes": 24 * GIB}]
    }));
    let result = paths(&catalog, &perf, "qwen3:8b", &desktop);
    let names: Vec<_> = result["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| path["path"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["gpu", "cpu-offload", "cpu"]);
    assert_eq!(result["paths"][0]["throughput"]["known"], true);
    assert_eq!(result["paths"][0]["throughputBasis"], "estimated");
    assert_eq!(result["recommended"], "gpu");
}

#[test]
fn unified_memory_topology_is_used_beyond_apple_silicon_but_stays_honest() {
    let (catalog, perf) = data();
    let spark = hardware(json!({
        "arch": "arm64", "platform": "linux",
        "totalRamBytes": 128 * GIB, "freeRamBytes": 100 * GIB, "freeDiskBytes": 1000 * GIB,
        "gpu": [{"vendor": "nvidia", "vramBytes": 0}], "unifiedMemory": true
    }));
    let result = paths(&catalog, &perf, "llama3.1:70b", &spark);
    assert_eq!(result["unifiedMemory"], true);
    assert_eq!(result["paths"][0]["path"], "unified");
    assert_eq!(result["paths"][0]["fits"], true);
    assert_eq!(result["paths"][0]["throughput"]["known"], false);
    assert_eq!(
        result["paths"][0]["throughputBasis"],
        "unknown: no sourced throughput class for this hardware and backend"
    );
    let mac = hardware(json!({
        "arch": "arm64", "platform": "darwin",
        "totalRamBytes": 36 * GIB, "freeRamBytes": 20 * GIB, "freeDiskBytes": 500 * GIB, "gpu": []
    }));
    let result = paths(&catalog, &perf, "qwen3:8b", &mac);
    assert_eq!(result["paths"][0]["path"], "unified");
    assert_eq!(result["paths"][0]["throughput"]["known"], true);
    assert_eq!(result["kvCacheKnown"], true);
    let model = catalog
        .models
        .iter()
        .find(|model| model.id == "qwen3:30b-a3b")
        .unwrap();
    assert!(model.kv_bytes_per_token.is_none());
    let at_context = plan(model, &mac, &perf, Some(32768.0), "llamacpp").unwrap();
    assert!(!at_context.kv_cache_known);
    assert!(
        rigspark_core::plan::format_plan(&at_context)
            .contains("KV cache at this context is unknown")
    );
    let without = hardware(json!({
        "arch": "arm64", "platform": "linux",
        "totalRamBytes": 128 * GIB, "freeRamBytes": 100 * GIB, "freeDiskBytes": 1000 * GIB,
        "gpu": []
    }));
    assert_eq!(
        paths(&catalog, &perf, "qwen3:8b", &without)["unifiedMemory"],
        false
    );
}

#[test]
fn nothing_fitting_reports_every_shortfall_and_no_recommendation() {
    let (catalog, perf) = data();
    let small = hardware(json!({
        "arch": "x64", "platform": "win32",
        "totalRamBytes": 8 * GIB, "freeRamBytes": 6 * GIB, "freeDiskBytes": 500 * GIB,
        "gpu": [{"vendor": "nvidia", "vramBytes": 8 * GIB}]
    }));
    let result = paths(&catalog, &perf, "llama3.1:70b", &small);
    assert_eq!(result["recommended"], Value::Null);
    for path in result["paths"].as_array().unwrap() {
        assert_eq!(path["fits"], false, "{path}");
        assert!(path["shortfallBytes"].as_f64().unwrap() > 0.0, "{path}");
        assert_eq!(path["throughputBasis"], "unknown: does not fit");
    }
}

fn path_names(result: &Value) -> Vec<String> {
    result["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| path["path"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn only_discrete_gpus_with_vram_get_gpu_paths() {
    let (catalog, perf) = data();
    // An Apple GPU on hardware reported as non-unified is not a discrete device to offload to.
    let apple = hardware(json!({
        "arch": "x64", "platform": "darwin", "unifiedMemory": false,
        "totalRamBytes": 32 * GIB, "freeRamBytes": 28 * GIB, "freeDiskBytes": 500 * GIB,
        "gpu": [{"vendor": "apple", "vramBytes": 16 * GIB}]
    }));
    assert_eq!(
        path_names(&paths(&catalog, &perf, "qwen3:8b", &apple)),
        ["cpu"]
    );
    // A detected GPU that reports no VRAM cannot hold any layers.
    let empty = hardware(json!({
        "arch": "x64", "platform": "linux", "unifiedMemory": false,
        "totalRamBytes": 32 * GIB, "freeRamBytes": 28 * GIB, "freeDiskBytes": 500 * GIB,
        "gpu": [{"vendor": "nvidia", "vramBytes": 0}]
    }));
    assert_eq!(
        path_names(&paths(&catalog, &perf, "qwen3:8b", &empty)),
        ["cpu"]
    );
}

#[test]
fn shortfall_is_the_usable_memory_missing_after_headroom() {
    let (catalog, perf) = data();
    let rig = hardware(json!({
        "arch": "x64", "platform": "linux",
        "totalRamBytes": 64 * GIB, "freeRamBytes": 60 * GIB, "freeDiskBytes": 500 * GIB,
        "gpu": [{"vendor": "nvidia", "vramBytes": 24 * GIB}, {"vendor": "nvidia", "vramBytes": 24 * GIB}]
    }));
    let model = catalog
        .models
        .iter()
        .find(|model| model.id == "mixtral:8x7b")
        .unwrap();
    let result = plan(model, &rig, &perf, None, "llamacpp").unwrap();
    let single = result.paths.iter().find(|path| path.path == "gpu").unwrap();
    assert!(!single.fits);
    let required = single.required_bytes.unwrap();
    let expected = (required / (1.0 - rigspark_core::sizing::HEADROOM) - single.usable_bytes)
        .max(0.0)
        .ceil();
    assert!(expected > 0.0);
    assert_eq!(single.shortfall_bytes, Some(expected));
}

#[test]
fn formatted_plan_labels_gpu_counts_sizes_and_speed_basis() {
    use rigspark_core::{
        advice::Throughput,
        plan::{PathPlan, Plan, format_plan},
    };
    let gib = GIB as f64;
    let path = |name: &'static str, fits: bool, gpu_count: Option<usize>| PathPlan {
        path: name,
        fits,
        quant: fits.then(|| "Q4_K_M".to_owned()),
        required_bytes: fits.then_some(20.0 * gib),
        usable_bytes: 48.0 * gib,
        shortfall_bytes: None,
        reason: None,
        gpu_count,
        throughput: Throughput::unknown(),
        throughput_basis: "unknown: does not fit",
    };
    let mut gpu = path("gpu", false, Some(1));
    gpu.shortfall_bytes = Some(1.5 * gib);
    gpu.reason = Some("vram-bound");
    let mut multi = path("multi-gpu", true, Some(2));
    multi.throughput_basis =
        "unknown: split across devices is not modeled; measure with llmup bench";
    let mut offload = path("cpu-offload", true, Some(2));
    offload.throughput_basis = "unknown: no sourced throughput class for this hardware and backend";
    let mut unified = path("unified", true, None);
    unified.throughput = Throughput {
        low_tok_per_sec: 12.0,
        high_tok_per_sec: 18.0,
        known: true,
    };
    unified.throughput_basis = "estimated";
    let mut cpu = path("cpu", false, None);
    cpu.reason = Some("ram-bound");
    let plan = Plan {
        model: "demo:8b".into(),
        context: Some(4096.0),
        backend: "llamacpp".into(),
        kv_cache_type: None,
        unified_memory: false,
        kv_cache_known: true,
        paths: vec![gpu, multi, offload, unified, cpu],
        recommended: Some("multi-gpu"),
    };
    assert_eq!(
        format_plan(&plan),
        "Plan for demo:8b (4096 tokens, llamacpp backend)\n\n\
         \x20 gpu (1 GPU)            does not fit; needs 1.5 GiB more usable memory\n\
         \x20 multi-gpu (2 GPUs)     fits Q4_K_M using 20.0 GiB of 48.0 GiB; speed unknown (not modeled; measure with llmup bench)\n\
         \x20 cpu-offload            fits Q4_K_M using 20.0 GiB of 48.0 GiB; speed unknown (no sourced throughput data)\n\
         \x20 unified                fits Q4_K_M using 20.0 GiB of 48.0 GiB; 12-18 tok/s estimated\n\
         \x20 cpu                    does not fit (ram-bound)\n\
         \nRecommended path: multi-gpu\n"
    );
}
