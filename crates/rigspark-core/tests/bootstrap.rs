use rigspark_core::{
    bootstrap::{BOOTSTRAP_CLOCK, build_catalog, derive_benchmark_proxy},
    catalog::Catalog,
    enrich::parse_candidates,
};

#[test]
fn snapshot_entries_build_independently_and_have_pinned_family_offsets() {
    let candidates = parse_candidates(include_str!("../fixtures/registry-snapshot.json")).unwrap();
    let expected = Catalog::parse(include_str!("../fixtures/bootstrap-oracle.json")).unwrap();
    for candidate in candidates {
        if matches!(
            candidate.id.as_str(),
            "qwen3.6:35b" | "qwen3.5:4b" | "bonsai:8b"
        ) {
            assert!(rigspark_core::bootstrap::family_quality_offset(&candidate.family).is_none());
            continue;
        }
        assert!(
            rigspark_core::bootstrap::family_quality_offset(&candidate.family).is_some(),
            "{}",
            candidate.family
        );
        let actual = build_catalog(std::slice::from_ref(&candidate), BOOTSTRAP_CLOCK).unwrap();
        let expected = expected
            .models
            .iter()
            .find(|model| model.id == candidate.id)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&actual.models[0]).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
}

#[test]
fn committed_curated_fields_match_bootstrap_except_recorded_corrections() {
    let candidates = parse_candidates(include_str!("../fixtures/registry-snapshot.json")).unwrap();
    let mut actual = build_catalog(&candidates, BOOTSTRAP_CLOCK).unwrap();
    let mut committed =
        Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap();
    // Auto-admitted entries are not bootstrap output; curated ones may only differ by a
    // recorded, cited correction of what the shipped artifact contains.
    committed
        .models
        .retain(|model| model.provenance == rigspark_core::catalog::EntryProvenance::Curated);
    let record: serde_json::Value = serde_json::from_str(include_str!(
        "../../../docs/references/catalog-corrections.json"
    ))
    .unwrap();
    for correction in record["corrections"].as_array().unwrap() {
        assert!(
            correction["sources"]
                .as_array()
                .is_some_and(|sources| !sources.is_empty())
        );
        let model = actual
            .models
            .iter_mut()
            .find(|model| model.id == correction["id"])
            .unwrap();
        let to = &correction["to"];
        match correction["field"].as_str().unwrap() {
            "contextLength" => model.context_length = Some(to.as_f64().unwrap()),
            "license" => model.license = to.as_str().unwrap().into(),
            "kvBytesPerToken" => model.kv_bytes_per_token = to.as_f64(),
            "defaultQuantization" => {
                let index = rigspark_core::registry_collector::pulled_quantization(model).unwrap();
                let name = to["name"].as_str().unwrap().to_string();
                model.quantizations[index].name = name.clone();
                let mut position = 0;
                model.quantizations.retain(|quant| {
                    let keep = position == index || quant.name != name;
                    position += 1;
                    keep
                });
            }
            other => panic!("unexpected correction field {other}"),
        }
    }
    let skeleton = |catalog: Catalog| {
        let mut value = serde_json::to_value(catalog).unwrap();
        value.as_object_mut().unwrap().remove("generatedAt");
        for model in value["models"].as_array_mut().unwrap() {
            for quant in model["quantizations"].as_array_mut().unwrap() {
                let name = quant["name"].clone();
                *quant = serde_json::json!({"name": name});
            }
        }
        value
    };
    assert_eq!(skeleton(actual), skeleton(committed));
}

#[test]
fn complete_bootstrap_matches_frozen_typescript_oracle() {
    let mut candidates =
        parse_candidates(include_str!("../fixtures/registry-snapshot.json")).unwrap();
    candidates.retain(|candidate| {
        !matches!(
            candidate.id.as_str(),
            "qwen3.6:35b" | "qwen3.5:4b" | "bonsai:8b"
        )
    });
    let expected = Catalog::parse(include_str!("../fixtures/bootstrap-oracle.json")).unwrap();
    let actual = build_catalog(&candidates, BOOTSTRAP_CLOCK).unwrap();
    assert_eq!(actual.models.len(), 66);
    assert_eq!(
        serde_json::to_value(&actual).unwrap(),
        serde_json::to_value(&expected).unwrap()
    );
    assert_eq!(
        serde_json::to_string(&actual).unwrap(),
        serde_json::to_string(&build_catalog(&candidates, BOOTSTRAP_CLOCK).unwrap()).unwrap()
    );
    assert!(build_catalog(&[], BOOTSTRAP_CLOCK).is_err());
    assert!(build_catalog(&candidates, "invalid").is_err());
}

#[test]
fn preserves_exact_geometry_and_unknown_attention_honesty_gate() {
    let candidates = parse_candidates(include_str!("../fixtures/registry-snapshot.json")).unwrap();
    let actual = build_catalog(&candidates, BOOTSTRAP_CLOCK).unwrap();
    let geometry = [
        ("llama3.1:8b", 32, 8, 128),
        ("llama3.1:70b", 80, 8, 128),
        ("llama3.3:70b", 80, 8, 128),
        ("llama3.2:1b", 16, 8, 64),
        ("llama3.2:3b", 28, 8, 128),
        ("qwen2.5:0.5b", 24, 2, 64),
        ("qwen2.5:0.5b-mlx", 24, 2, 64),
        ("qwen2.5:1.5b", 28, 2, 128),
        ("qwen2.5:3b", 36, 2, 128),
        ("qwen2.5:7b", 28, 4, 128),
        ("qwen2.5:14b", 48, 8, 128),
        ("qwen2.5:32b", 64, 8, 128),
        ("qwen2.5:72b", 80, 8, 128),
        ("qwen2.5-coder:7b", 28, 4, 128),
        ("qwen2.5-coder:32b", 64, 8, 128),
        ("mistral:7b", 32, 8, 128),
        ("mistral-nemo:12b", 40, 8, 128),
        ("mistral-small:24b", 40, 8, 128),
    ];
    assert_eq!(
        actual
            .models
            .iter()
            .filter(|model| model.kv_bytes_per_token.is_some())
            .count(),
        geometry.len()
    );
    for (id, layers, heads, dimension) in geometry {
        let model = actual.models.iter().find(|model| model.id == id).unwrap();
        assert_eq!(
            model.kv_bytes_per_token,
            Some(f64::from(4 * layers * heads * dimension)),
            "{id}"
        );
    }
    for id in [
        "deepseek-v3",
        "deepseek-r1:671b",
        "gemma2:9b",
        "gemma3:12b",
        "gemma4:e4b-it-qat",
    ] {
        assert_eq!(
            actual
                .models
                .iter()
                .find(|model| model.id == id)
                .unwrap()
                .kv_bytes_per_token,
            None
        );
    }
}

#[test]
fn includes_the_official_qwen35_4b_ollama_artifact() {
    // The frozen baseline pins the reviewed artifact; the shipped entry keeps its identity.
    let shipped = Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap();
    let live = shipped
        .models
        .iter()
        .find(|model| model.id == "qwen3.5:4b")
        .unwrap();
    assert_eq!(live.source.ollama.as_deref(), Some("qwen3.5:4b"));
    assert!(
        live.quantizations
            .iter()
            .all(|quant| quant.sha256.is_some())
    );
    let catalog = Catalog::parse(include_str!("../fixtures/catalog-baseline.json")).unwrap();
    let model = catalog
        .models
        .iter()
        .find(|model| model.id == "qwen3.5:4b")
        .unwrap();
    assert_eq!(model.family, "qwen3.5");
    assert_eq!(model.params, "4.66B");
    assert_eq!(model.context_length, Some(262_144.0));
    assert_eq!(model.source.ollama.as_deref(), Some("qwen3.5:4b"));
    assert_eq!(model.source.hf.as_deref(), Some("Qwen/Qwen3.5-4B"));
    assert_eq!(model.kv_bytes_per_token, None);
    assert_eq!(model.quantizations.len(), 1);
    let quant = &model.quantizations[0];
    assert_eq!(quant.name, "Q4_K_M");
    assert_eq!(quant.disk_bytes, 3_389_971_840.0);
    assert_eq!(
        quant.sha256.as_deref(),
        Some("81fb60c7daa80fc1123380b98970b320ae233409f0f71a72ed7b9b0d62f40490")
    );
}

#[test]
fn reproduces_every_curated_catalog_proxy() {
    let catalog = Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap();
    for model in catalog
        .models
        .into_iter()
        .filter(|model| model.provenance == rigspark_core::catalog::EntryProvenance::Curated)
    {
        assert_eq!(
            rigspark_core::bootstrap::family_quality_offset(&model.family)
                .map(|_| derive_benchmark_proxy(&model.family, &model.params).unwrap()),
            model.benchmark_proxy,
            "{}",
            model.id
        );
    }
}

#[test]
fn parameter_ladder_uses_exclusive_ceilings() {
    for (params, expected) in [
        ("0.49B", 0.05),
        ("0.5B", 0.12),
        ("1B", 0.2),
        ("2B", 0.28),
        ("4B", 0.36),
        ("7B", 0.44),
        ("10B", 0.52),
        ("15B", 0.58),
        ("25B", 0.64),
        ("40B", 0.7),
        ("80B", 0.78),
        ("200B", 0.85),
    ] {
        assert_eq!(
            derive_benchmark_proxy("unknown-family", params).unwrap(),
            expected
        );
    }
}

#[test]
fn family_offsets_and_total_moe_parameters_match_bootstrap() {
    assert_eq!(derive_benchmark_proxy("qwen3", "30B").unwrap(), 0.74);
    assert_eq!(
        derive_benchmark_proxy("kimi-k2-thinking", "1T").unwrap(),
        0.99
    );
    assert_eq!(derive_benchmark_proxy("phi4", "14B").unwrap(), 0.64);
    assert!(derive_benchmark_proxy("qwen3", "invalid").is_err());
}
