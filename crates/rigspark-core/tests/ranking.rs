use rigspark_core::{
    catalog::{Catalog, PerfDataset},
    ranking::{AdviceOptions, recommend},
    sizing::Hardware,
};
use serde_json::json;

#[test]
fn ranks_deterministically_at_explicit_and_relative_contexts() {
    let catalog = Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap();
    let perf = PerfDataset::parse(include_str!("../../rigspark-core/data/perf.json")).unwrap();
    let hardware: Hardware = serde_json::from_value(json!({"arch":"arm64","platform":"darwin","totalRamBytes":34359738368_u64,"freeRamBytes":24000000000_u64,"freeDiskBytes":500000000000_u64,"gpu":[]})).unwrap();
    let options = AdviceOptions {
        context: Some(65536.0),
        ..Default::default()
    };
    let result = recommend(&catalog, &hardware, &perf, &options).unwrap();
    assert_eq!(
        result,
        recommend(&catalog, &hardware, &perf, &options).unwrap()
    );
    assert_eq!(
        result["ranked"].as_array().unwrap().len() + result["wontFit"].as_array().unwrap().len(),
        catalog.models.len()
    );
    assert!(
        result["ranked"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["context"] == 65536.0)
    );
    let relative = recommend(
        &catalog,
        &hardware,
        &perf,
        &AdviceOptions {
            context_percent: Some(25),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!relative["ranked"].as_array().unwrap().is_empty());
    assert!(
        recommend(
            &catalog,
            &hardware,
            &perf,
            &AdviceOptions {
                context: Some(1.0),
                max_context: true,
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn ties_use_id_and_available_backend_filters_renumber_results() {
    let mut catalog = Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap();
    let mut first = catalog
        .models
        .iter()
        .find(|model| model.id == "llama3.1:8b")
        .unwrap()
        .clone();
    first.id = "tie:z".into();
    let mut second = first.clone();
    second.id = "tie:a".into();
    catalog.models = vec![first, second];
    let hardware: Hardware = serde_json::from_value(json!({"arch":"x64","platform":"linux","totalRamBytes":68719476736_u64,"freeRamBytes":60000000000_u64,"freeDiskBytes":500000000000_u64,"gpu":[{"vendor":"nvidia","vramBytes":25769803776_u64}]})).unwrap();
    let perf = PerfDataset::parse(include_str!("../../rigspark-core/data/perf.json")).unwrap();
    let result = recommend(&catalog, &hardware, &perf, &AdviceOptions::default()).unwrap();
    assert_eq!(result["ranked"][0]["id"], "tie:a");
    let result = recommend(
        &catalog,
        &hardware,
        &perf,
        &AdviceOptions {
            available_backends: Some(vec![]),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(result["ranked"].as_array().unwrap().is_empty());
    assert!(result["command"].is_null());
}

mod scoring {
    use rigspark_core::{
        catalog::{Catalog, CatalogModel, PerfDataset},
        ranking::{AdviceOptions, backends, recommend, recommend_detailed},
        sizing::{Hardware, parse_param_count, quant_bits},
    };
    use serde_json::{Value, json};

    const GIB: u64 = 1024 * 1024 * 1024;

    fn catalog() -> Catalog {
        Catalog::parse(rigspark_core::MODELS_JSON).unwrap()
    }
    fn perf() -> PerfDataset {
        PerfDataset::parse(rigspark_core::PERF_JSON).unwrap()
    }
    fn rig(value: Value) -> Hardware {
        serde_json::from_value(value).unwrap()
    }
    fn gpu_rig() -> Hardware {
        rig(
            json!({"arch":"x64","platform":"linux","totalRamBytes":64 * GIB,"freeRamBytes":60 * GIB,
            "freeDiskBytes":500 * GIB,"gpu":[{"vendor":"nvidia","vramBytes":24 * GIB}]}),
        )
    }
    fn cpu_rig() -> Hardware {
        rig(
            json!({"arch":"x64","platform":"linux","totalRamBytes":32 * GIB,"freeRamBytes":28 * GIB,
            "freeDiskBytes":500 * GIB,"gpu":[]}),
        )
    }
    fn mac() -> Hardware {
        rig(
            json!({"arch":"arm64","platform":"darwin","totalRamBytes":64 * GIB,"freeRamBytes":48 * GIB,
            "freeDiskBytes":500 * GIB,"gpu":[]}),
        )
    }
    fn model(catalog: &Catalog, id: &str) -> CatalogModel {
        catalog
            .models
            .iter()
            .find(|model| model.id == id)
            .unwrap()
            .clone()
    }
    fn close(actual: &Value, expected: f64) {
        let actual = actual.as_f64().unwrap();
        assert!((actual - expected).abs() < 1e-12, "{actual} != {expected}");
    }
    fn param_class(params: &str) -> f64 {
        ((parse_param_count(params).unwrap().log10() - 9.0) / 3.0).clamp(0.0, 1.0)
    }

    #[test]
    fn explicit_context_must_be_a_whole_token_count_in_range() {
        for bad in [1.5, 0.0, 20_000_000.0, f64::INFINITY, f64::NAN] {
            let options = AdviceOptions {
                context: Some(bad),
                ..Default::default()
            };
            assert!(options.validate().is_err(), "{bad}");
        }
        for good in [1.0, 4096.0, 10_000_000.0] {
            let options = AdviceOptions {
                context: Some(good),
                ..Default::default()
            };
            assert!(options.validate().is_ok(), "{good}");
        }
    }

    #[test]
    fn context_percent_is_a_share_of_the_model_window() {
        let llama = model(&catalog(), "llama3.1:8b");
        assert_eq!(llama.context_length, 131072.0);
        for (percent, tokens) in [(25, 32768.0), (50, 65536.0), (75, 98304.0), (100, 131072.0)] {
            let options = AdviceOptions {
                context_percent: Some(percent),
                ..Default::default()
            };
            assert_eq!(options.tokens(&llama), Some(tokens), "{percent}%");
        }
    }

    #[test]
    fn backends_follow_sources_and_memory_topology() {
        let catalog = catalog();
        let gguf = catalog
            .models
            .iter()
            .find(|model| model.source.ollama.is_some() && model.source.gguf.is_some())
            .unwrap();
        let mlx = catalog
            .models
            .iter()
            .find(|model| model.source.mlx.is_some())
            .unwrap();
        let mut every = gguf.clone();
        every.source.mlx = mlx.source.mlx.clone();
        assert_eq!(
            backends(&every, &cpu_rig()),
            ["ollama", "llamacpp", "lmstudio"]
        );
        assert_eq!(
            backends(&every, &mac()),
            ["ollama", "llamacpp", "mlx", "lmstudio"]
        );
        let mut ollama_only = every.clone();
        ollama_only.source.gguf = None;
        ollama_only.source.mlx = None;
        assert_eq!(backends(&ollama_only, &mac()), ["ollama"]);
        let mut mlx_only = every;
        mlx_only.source.ollama = None;
        mlx_only.source.gguf = None;
        assert_eq!(backends(&mlx_only, &mac()), ["mlx", "lmstudio"]);
        assert!(backends(&mlx_only, &cpu_rig()).is_empty());
    }

    #[test]
    fn models_without_an_available_backend_are_left_out_entirely() {
        let mut catalog = catalog();
        let mut only = model(&catalog, "llama3.1:8b");
        only.source.gguf = None;
        only.source.mlx = None;
        catalog.models = vec![only];
        let result = recommend(
            &catalog,
            &gpu_rig(),
            &perf(),
            &AdviceOptions {
                available_backends: Some(vec!["llamacpp".into()]),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(result["ranked"].as_array().unwrap().is_empty());
        assert!(result["wontFit"].as_array().unwrap().is_empty());
    }

    #[test]
    fn detailed_scores_follow_the_documented_weights() {
        let mut catalog = catalog();
        let llama = model(&catalog, "llama3.1:8b");
        assert_eq!(llama.release_date.as_deref(), Some("2024-07-23"));
        let mut no_proxy = llama.clone();
        no_proxy.id = "llama3.1:8b-no-proxy".into();
        no_proxy.benchmark_proxy = None;
        catalog.models = vec![llama.clone(), no_proxy];
        // Exactly one year after release: recency is halfway through its two-year decay.
        catalog.generated_at = "2025-07-23T00:00:00.000Z".into();
        let class = param_class("8B");
        let proxy = llama.benchmark_proxy.unwrap();
        for (hardware, bandwidth, kind) in [
            (gpu_rig(), 1.0, "vram"),
            (cpu_rig(), 0.2, "ram"),
            (mac(), 0.5, "ram"),
        ] {
            let result =
                recommend_detailed(&catalog, &hardware, &perf(), &AdviceOptions::default())
                    .unwrap();
            assert_eq!(result["hardware"]["memoryKind"], kind);
            let ranked = result["ranked"].as_array().unwrap();
            assert_eq!(ranked.len(), 2, "{bandwidth}");
            for entry in ranked {
                let scores = &entry["scores"];
                let quality = if entry["id"] == "llama3.1:8b" {
                    0.5 * class + 0.5 * proxy
                } else {
                    class
                };
                close(&scores["quality"], quality);
                let ratio = entry["requiredBytes"].as_f64().unwrap()
                    / entry["usableBytes"].as_f64().unwrap();
                let fit = (1.0 - (ratio - 0.6).abs() / 0.6).clamp(0.0, 1.0);
                assert!(fit > 0.0 && fit < 1.0, "{bandwidth}: fit {fit}");
                close(&scores["fit"], fit);
                let bits = quant_bits(entry["quant"].as_str().unwrap()).unwrap();
                let speed =
                    (bandwidth * (4.7 / bits).clamp(0.0, 1.0) * (7e9 / 8e9)).clamp(0.0, 1.0);
                close(&scores["speed"], speed);
                close(&scores["recency"], 0.5);
                close(&scores["capability"], 1.0);
                close(
                    &entry["score"],
                    0.3 * quality + 0.2 * fit + 0.2 * speed + 0.15 * 0.5 + 0.15,
                );
            }
        }
    }

    #[test]
    fn equal_scores_prefer_a_sourced_benchmark_over_none() {
        let mut catalog = catalog();
        let llama = model(&catalog, "llama3.1:8b");
        let mut unscored = llama.clone();
        unscored.id = "tie:a".into();
        unscored.benchmark_proxy = None;
        let mut scored = llama;
        scored.id = "tie:b".into();
        // A proxy equal to the parameter class leaves quality, and so the score, unchanged.
        scored.benchmark_proxy = Some(param_class("8B"));
        catalog.models = vec![unscored, scored];
        let result = recommend(&catalog, &gpu_rig(), &perf(), &AdviceOptions::default()).unwrap();
        let ranked = result["ranked"].as_array().unwrap();
        assert_eq!(ranked[0]["score"], ranked[1]["score"]);
        assert_eq!(ranked[0]["id"], "tie:b");
        assert_eq!(ranked[1]["id"], "tie:a");
    }

    #[test]
    fn verdict_is_yes_only_for_a_known_mean_of_at_least_ten_tokens_per_second() {
        let catalog = catalog();
        let (mut yes, mut slow_known, mut unknown) = (0, 0, 0);
        for hardware in [gpu_rig(), cpu_rig(), mac()] {
            for backend in ["ollama", "llamacpp", "mlx"] {
                let options = AdviceOptions {
                    backend: Some(backend.into()),
                    ..Default::default()
                };
                let result = recommend(&catalog, &hardware, &perf(), &options).unwrap();
                for entry in result["ranked"].as_array().unwrap() {
                    let estimate = &entry["estTokPerSec"];
                    let expected = if estimate.is_null() {
                        unknown += 1;
                        "slow"
                    } else {
                        let mean = (estimate["lowTokPerSec"].as_f64().unwrap()
                            + estimate["highTokPerSec"].as_f64().unwrap())
                            / 2.0;
                        if mean >= 10.0 {
                            yes += 1;
                            "yes"
                        } else {
                            slow_known += 1;
                            "slow"
                        }
                    };
                    assert_eq!(entry["verdict"], expected, "{entry}");
                }
            }
        }
        assert!(
            yes > 0 && slow_known > 0 && unknown > 0,
            "{yes} {slow_known} {unknown}"
        );
    }
}
