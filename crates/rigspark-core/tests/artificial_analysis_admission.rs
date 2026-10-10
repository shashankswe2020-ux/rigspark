use rigspark_core::{
    catalog::{Catalog, PerfDataset},
    ranking::{AdviceOptions, recommend_detailed},
    sizing::{Hardware, SizingRequest, evaluate},
};
use serde_json::{Value, json};

fn unknown_catalog() -> Value {
    let mut raw: Value = serde_json::from_str(rigspark_core::MODELS_JSON).unwrap();
    let mut model = raw["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model.get("provenance").is_none())
        .unwrap()
        .clone();
    model["params"] = json!("unknown");
    model["architecture"] = json!("unknown");
    model.as_object_mut().unwrap().remove("contextLength");
    model.as_object_mut().unwrap().remove("activeParams");
    model.as_object_mut().unwrap().remove("kvBytesPerToken");
    model.as_object_mut().unwrap().remove("benchmarkProxy");
    model["capabilities"] = json!([]);
    model["source"] = json!({"weights":{
        "repo":"Publisher/Example","revision":"a".repeat(40),
        "files":[{"file":"model.safetensors","sha256":"b".repeat(64),"bytes":1_000_000_000.0}]
    }});
    model["availability"] = json!({"status":"advisory-only","reason":"backend-format-unsupported"});
    model["quantizations"] = json!([{"name":"unknown","diskBytes":1_000_000_000.0,"minRamBytes":1_150_000_000.0,"minVramBytes":1_150_000_000.0}]);
    raw["schemaVersion"] = json!(4);
    raw["models"] = json!([model]);
    raw
}

fn hardware() -> Hardware {
    serde_json::from_value(json!({
        "arch":"arm64","platform":"darwin","totalRamBytes":32_000_000_000_u64,
        "freeRamBytes":24_000_000_000_u64,"freeDiskBytes":100_000_000_000_u64,"gpu":[]
    }))
    .unwrap()
}

#[test]
fn advisory_unknown_facts_round_trip_without_inventing_values() {
    let raw = unknown_catalog();
    let catalog = Catalog::parse(&raw.to_string()).unwrap();
    assert_eq!(serde_json::to_value(&catalog).unwrap(), raw);
    let model = &catalog.models[0];
    assert!(model.ensure_runnable().is_err());
    let sized = evaluate(&SizingRequest {
        model: model.sizing(),
        hardware: hardware(),
        context: Some(4096.0),
    })
    .unwrap();
    assert_eq!(sized.weights, [1_000_000_000.0]);
    assert_eq!(sized.at_context, [None]);
    assert_eq!(sized.max_context, [None]);
    let perf = PerfDataset::parse(rigspark_core::PERF_JSON).unwrap();
    let report =
        recommend_detailed(&catalog, &hardware(), &perf, &AdviceOptions::default()).unwrap();
    assert!(report.to_string().contains(&model.id));
    assert!(
        !rigspark_core::advice::throughput(
            model,
            &model.quantizations[0],
            &hardware(),
            &perf,
            "ollama"
        )
        .unwrap()
        .known
    );
}

#[test]
fn unknown_facts_are_not_allowed_in_legacy_or_runnable_catalog_entries() {
    let mut raw = unknown_catalog();
    raw["schemaVersion"] = json!(3);
    raw["models"][0]
        .as_object_mut()
        .unwrap()
        .remove("availability");
    raw["models"][0]["source"] = json!({"hf":"Publisher/Example"});
    assert!(Catalog::parse(&raw.to_string()).is_err());
    let mut raw = unknown_catalog();
    raw["models"][0]["availability"] = json!({"status":"runnable","backend":"ollama"});
    raw["models"][0]["source"] = json!({"ollama":"example:latest"});
    raw["models"][0]["quantizations"][0]["sha256"] = json!("b".repeat(64));
    assert!(Catalog::parse(&raw.to_string()).is_err());
}

fn inputs() -> (
    Vec<rigspark_core::artificial_analysis::IndexModel>,
    Vec<rigspark_core::artificial_analysis::PublisherMapping>,
    Vec<rigspark_core::artificial_analysis::admission::AdmissionArtifact>,
) {
    use rigspark_core::artificial_analysis::{
        PublisherMapping, admission::AdmissionArtifact, parse_index_html,
    };
    let rows = parse_index_html(include_str!(
        "../fixtures/artificial-analysis-inventory.html"
    ))
    .unwrap();
    let model = Catalog::parse(&unknown_catalog().to_string())
        .unwrap()
        .models
        .remove(0);
    let artifacts = vec![AdmissionArtifact {
        model_ids: vec!["model-config-1".into(), "model-config-2".into()],
        source: model.source.weights.unwrap(),
        license: "apache-2.0".into(),
        context_length: None,
    }];
    (
        rows,
        vec![PublisherMapping {
            creator_slug: "qwen".into(),
            release_slug: "qwen-example-32b".into(),
            repo: "Publisher/Example".into(),
        }],
        artifacts,
    )
}

fn base_catalog() -> Catalog {
    let mut catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    catalog.models.retain(|model| {
        matches!(
            model.provenance,
            rigspark_core::catalog::EntryProvenance::Curated
        ) && model
            .quantizations
            .iter()
            .all(|quant| quant.projectors.is_empty())
    });
    catalog.models.truncate(1);
    catalog
}

#[test]
fn admission_merges_once_preserves_existing_facts_and_is_idempotent() {
    use rigspark_core::artificial_analysis::admission::admit;
    let (rows, mappings, artifacts) = inputs();
    let original = base_catalog();
    let prior = serde_json::to_value(&original.models[0]).unwrap();
    let result = admit(
        &original,
        &rows,
        &mappings,
        &artifacts,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    assert_eq!(result.models.len(), original.models.len() + 1);
    let mut retained = serde_json::to_value(&result.models[0]).unwrap();
    retained.as_object_mut().unwrap().remove("availability");
    assert_eq!(retained, prior);
    let added = result
        .models
        .iter()
        .find(|model| model.is_advisory_only())
        .unwrap();
    assert_eq!(added.params, "unknown");
    assert!(added.context_length.is_none());
    assert!(added.ensure_runnable().is_err());
    assert_eq!(added.source.weights.as_ref().unwrap().files.len(), 1);
    let repeat = admit(
        &result,
        &rows,
        &mappings,
        &artifacts,
        "2026-10-12T00:00:00Z",
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(repeat).unwrap(),
        serde_json::to_value(result).unwrap()
    );
    assert_eq!(serde_json::to_value(&original.models[0]).unwrap(), prior);
}

#[test]
fn admission_reuses_exact_runnable_artifacts_instead_of_downgrading_them() {
    use rigspark_core::{
        artificial_analysis::admission::admit,
        catalog::{GgufSource, Source},
    };
    let (rows, mappings, mut artifacts) = inputs();
    artifacts[0].source.files[0].file = "model.gguf".into();
    let mut original = base_catalog();
    let model = &mut original.models[0];
    model.source = Source {
        gguf: Some(GgufSource {
            repo: "Publisher/Example".into(),
            revision: "a".repeat(40),
            file: "model.gguf".into(),
            sha256: "b".repeat(64),
        }),
        ..Source::default()
    };
    model.quantizations.truncate(1);
    model.quantizations[0].disk_bytes = 1_000_000_000.0;
    model.quantizations[0].sha256 = Some("b".repeat(64));
    let result = admit(
        &original,
        &rows,
        &mappings,
        &artifacts,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    assert_eq!(result.models.len(), 1);
    assert!(!result.models[0].is_advisory_only());
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::to_value(original).unwrap()
    );
}

#[test]
fn admission_rejects_unresolved_conflicting_and_unsupported_evidence_without_mutation() {
    use rigspark_core::artificial_analysis::admission::admit;
    let (rows, mappings, artifacts) = inputs();
    let original = base_catalog();
    let before = serde_json::to_value(&original).unwrap();
    assert!(admit(&original, &rows, &[], &artifacts, "2026-10-11T00:00:00Z").is_err());
    assert!(admit(&original, &rows, &mappings, &[], "2026-10-11T00:00:00Z").is_err());
    let mut conflict = artifacts.clone();
    conflict[0].license = "proprietary".into();
    assert!(
        admit(
            &original,
            &rows,
            &mappings,
            &conflict,
            "2026-10-11T00:00:00Z"
        )
        .is_err()
    );
    conflict[0].license = "mit".into();
    conflict.extend(artifacts.clone());
    assert!(
        admit(
            &original,
            &rows,
            &mappings,
            &conflict,
            "2026-10-11T00:00:00Z"
        )
        .is_err()
    );
    let mut incomplete = artifacts;
    incomplete[0].source.files[0].file = "model-00001-of-00002.safetensors".into();
    assert!(
        admit(
            &original,
            &rows,
            &mappings,
            &incomplete,
            "2026-10-11T00:00:00Z"
        )
        .is_err()
    );
    assert_eq!(serde_json::to_value(original).unwrap(), before);
}

#[test]
fn admission_never_fabricates_pins_when_upgrading_a_legacy_catalog() {
    use rigspark_core::artificial_analysis::admission::admit;
    let (rows, mappings, artifacts) = inputs();
    let mut original = base_catalog();
    original.models[0].quantizations[0].sha256 = None;
    original.models[0].source = rigspark_core::catalog::Source {
        hf: Some("Publisher/Legacy".into()),
        ..Default::default()
    };
    let error = admit(
        &original,
        &rows,
        &mappings,
        &artifacts,
        "2026-10-11T00:00:00Z",
    )
    .unwrap_err();
    assert!(error.to_string().contains(&original.models[0].id));
    assert!(error.to_string().contains("pin"));
}

#[test]
fn unknown_architecture_is_not_printed_as_dense_and_no_install_command_is_suggested() {
    let catalog = Catalog::parse(&unknown_catalog().to_string()).unwrap();
    let text = rigspark_core::reports::catalog_text(&catalog, &hardware(), true).unwrap();
    assert!(!text.contains("dense"));
    let perf = PerfDataset::parse(rigspark_core::PERF_JSON).unwrap();
    let options = AdviceOptions {
        available_backends: Some(vec!["ollama".into()]),
        max_context: true,
        ..Default::default()
    };
    let report = recommend_detailed(&catalog, &hardware(), &perf, &options).unwrap();
    assert_eq!(report["ranked"].as_array().unwrap().len(), 1);
    assert!(report["ranked"][0]["maxContextTokens"].is_null());
    assert!(report["ranked"][0]["estTokPerSec"].is_null());
    assert!(report["command"].is_null());
}

#[test]
fn admission_is_deterministic_for_permuted_alias_evidence_and_distinct_revisions() {
    use rigspark_core::artificial_analysis::admission::admit;
    let (mut rows, mappings, mut artifacts) = inputs();
    let original = base_catalog();
    let mut alias = artifacts[0].clone();
    alias.source.files[0].file = "alias.safetensors".into();
    artifacts.push(alias);
    let mut distinct = artifacts[0].clone();
    distinct.source.revision = "c".repeat(40);
    artifacts.push(distinct);
    let first = admit(
        &original,
        &rows,
        &mappings,
        &artifacts,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    rows.reverse();
    artifacts.reverse();
    let second = admit(
        &original,
        &rows,
        &mappings,
        &artifacts,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    assert_eq!(first.models.len(), original.models.len() + 2);
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(second).unwrap()
    );
}

#[test]
fn admission_preserves_sourced_context_and_never_overwrites_an_id_collision() {
    use rigspark_core::artificial_analysis::admission::admit;
    let (rows, mappings, mut artifacts) = inputs();
    artifacts[0].context_length = Some(32768);
    let mut original = base_catalog();
    original.models[0].id = "aa-qwen-example-32b-aaaaaaaaaaaa-bbbbbbbbbbbbbbbb".into();
    let result = admit(
        &original,
        &rows,
        &mappings,
        &artifacts,
        "2026-10-11T00:00:00Z",
    )
    .unwrap();
    assert_eq!(result.models[0].id, original.models[0].id);
    assert_eq!(result.models[1].id, format!("{}-1", original.models[0].id));
    assert_eq!(result.models[1].context_length, Some(32768.0));
    assert_eq!(
        result.models[0].source.gguf.as_ref().unwrap().sha256,
        original.models[0].source.gguf.as_ref().unwrap().sha256
    );
    artifacts[0].context_length = Some(9_007_199_254_740_992);
    assert!(
        admit(
            &original,
            &rows,
            &mappings,
            &artifacts,
            "2026-10-11T00:00:00Z"
        )
        .is_err()
    );
}
