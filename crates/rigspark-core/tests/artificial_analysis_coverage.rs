use rigspark_core::artificial_analysis::{MAX_INDEX_HTML_BYTES, parse_index_html};

const INVENTORY: &str = include_str!("../fixtures/artificial-analysis-inventory.html");

#[test]
fn parses_public_index_inventory_and_preserves_benchmark_configurations() {
    let inventory = parse_index_html(INVENTORY).unwrap();

    assert_eq!(inventory.len(), 3);
    assert_eq!(
        inventory
            .iter()
            .filter(|model| model.is_open_weights)
            .count(),
        2
    );
    assert_eq!(inventory[0].release.slug, "qwen-example-32b");
    assert_eq!(inventory[1].release.slug, "qwen-example-32b");
    assert_ne!(inventory[0].id, inventory[1].id);
    assert_eq!(inventory[0].creator.slug, "qwen");
    assert!(!inventory[2].is_open_weights);
}

#[test]
fn rejects_empty_or_missing_inventory_instead_of_reporting_complete_coverage() {
    let empty = INVENTORY.replace(
        "\\\"initialModels\\\":[{\\\"id\\\":\\\"model-config-1\\\"",
        "\\\"initialModels\\\":[],\\\"ignored\\\":[{\\\"id\\\":\\\"model-config-1\\\"",
    );
    assert_eq!(
        parse_index_html(&empty).unwrap_err().0,
        "Artificial Analysis inventory is empty"
    );
    assert_eq!(
        parse_index_html("<!doctype html><title>changed</title>")
            .unwrap_err()
            .0,
        "Artificial Analysis inventory missing"
    );
}

#[test]
fn rejects_oversized_or_structurally_invalid_inventory() {
    assert_eq!(
        parse_index_html(&"x".repeat(MAX_INDEX_HTML_BYTES + 1))
            .unwrap_err()
            .0,
        "Artificial Analysis index exceeds 4 MiB"
    );
    let invalid = INVENTORY.replacen("\\\"isOpenWeights\\\":true,", "", 1);
    assert!(parse_index_html(&invalid).is_err());
}
use rigspark_core::{
    artificial_analysis::{
        ArtifactObservation, PublisherMapping,
        coverage::{CoverageStatus, evaluate},
    },
    catalog::{Catalog, PinnedFile, PinnedFileSource},
};

fn report_inputs() -> (
    Vec<rigspark_core::artificial_analysis::IndexModel>,
    Vec<PublisherMapping>,
    Vec<ArtifactObservation>,
    Catalog,
) {
    let rows = rigspark_core::artificial_analysis::parse_index_html(include_str!(
        "../fixtures/artificial-analysis-inventory.html"
    ))
    .unwrap();
    let mappings = vec![PublisherMapping {
        creator_slug: "qwen".into(),
        release_slug: "qwen-example-32b".into(),
        repo: "Publisher/Example".into(),
    }];
    let source = PinnedFileSource {
        repo: mappings[0].repo.clone(),
        revision: "a".repeat(40),
        files: vec![PinnedFile {
            file: "model.safetensors".into(),
            sha256: "b".repeat(64),
            bytes: 1000.0,
        }],
    };
    let observations = rows
        .iter()
        .filter(|row| row.is_open_weights)
        .map(|row| ArtifactObservation {
            model_id: row.id.clone(),
            source: source.clone(),
        })
        .collect();
    let mut raw: serde_json::Value = serde_json::from_str(rigspark_core::MODELS_JSON).unwrap();
    let mut model = raw["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model.get("provenance").is_none())
        .unwrap()
        .clone();
    model["source"] = serde_json::json!({"weights":source});
    model["availability"] =
        serde_json::json!({"status":"advisory-only","reason":"backend-format-unsupported"});
    model["quantizations"] = serde_json::json!([{"name":"BF16","diskBytes":1000,"minRamBytes":1150,"minVramBytes":1150}]);
    raw["schemaVersion"] = serde_json::json!(4);
    raw["models"] = serde_json::json!([model]);
    let catalog = Catalog::parse(&raw.to_string()).unwrap();
    (rows, mappings, observations, catalog)
}

#[test]
fn report_counts_rows_artifacts_exclusions_and_collapsed_configurations() {
    let (rows, mappings, observations, catalog) = report_inputs();
    let report = evaluate(&rows, &mappings, &observations, &catalog).unwrap();
    assert!(report.complete);
    assert_eq!(
        (
            report.index_rows,
            report.open_weights_rows,
            report.unique_artifacts
        ),
        (3, 2, 1)
    );
    assert_eq!(
        (
            report.covered_runnable,
            report.covered_advisory_only,
            report.collapsed_configurations
        ),
        (0, 1, 1)
    );
    assert_eq!(report.proprietary_excluded, ["model-config-3"]);
    assert!(report.unmatched.is_empty());
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["unmatchedRows"], 0);
    assert_eq!(json["proprietaryRows"], 1);
    assert!(matches!(
        report.artifacts[0].status,
        CoverageStatus::AdvisoryOnly
    ));
    assert_eq!(
        report.artifacts[0].catalog_ids,
        [catalog.models[0].id.clone()]
    );
}

#[test]
fn report_never_counts_unresolved_rows_or_name_only_matches_as_covered() {
    let (rows, mappings, observations, mut catalog) = report_inputs();
    let report = evaluate(&rows, &[], &[], &catalog).unwrap();
    assert!(!report.complete);
    assert_eq!(report.unmatched.len(), 2);
    assert_eq!(report.unique_artifacts, 0);
    let report = evaluate(&rows, &mappings, &observations[..1], &catalog).unwrap();
    assert!(!report.complete);
    assert_eq!(report.unmatched[0].model_id, "model-config-2");
    catalog.models[0].source.weights.as_mut().unwrap().revision = "c".repeat(40);
    let report = evaluate(&rows, &mappings, &observations, &catalog).unwrap();
    assert!(!report.complete);
    assert!(matches!(
        report.artifacts[0].status,
        CoverageStatus::MissingCatalog
    ));
}

#[test]
fn report_ambiguous_catalog_entries_fail_the_gate() {
    let (rows, mappings, observations, mut catalog) = report_inputs();
    let mut duplicate = catalog.models[0].clone();
    duplicate.id = "second-entry".into();
    catalog.models.push(duplicate);
    let report = evaluate(&rows, &mappings, &observations, &catalog).unwrap();
    assert!(!report.complete);
    assert!(matches!(
        report.artifacts[0].status,
        CoverageStatus::AmbiguousCatalog
    ));
    assert_eq!(report.artifacts[0].catalog_ids.len(), 2);
    assert_eq!(report.covered_advisory_only, 0);
    assert_eq!(
        serde_json::to_value(report).unwrap()["ambiguousCatalogArtifacts"],
        1
    );
}

#[test]
fn report_counts_collapsed_rows_once_even_with_multiple_exports() {
    let (mut rows, mappings, mut observations, catalog) = report_inputs();
    let mut exports = observations.clone();
    for observation in &mut exports {
        observation.source.files[0].sha256 = "c".repeat(64);
    }
    observations.extend(exports);
    let expected = evaluate(&rows, &mappings, &observations, &catalog).unwrap();
    assert_eq!(expected.collapsed_configurations, 1);
    assert_eq!(expected.unique_artifacts, 2);
    assert!(!expected.complete);
    rows.reverse();
    observations.reverse();
    observations.push(observations[0].clone());
    let reordered = evaluate(&rows, &mappings, &observations, &catalog).unwrap();
    assert_eq!(
        serde_json::to_value(expected).unwrap(),
        serde_json::to_value(reordered).unwrap()
    );
}

#[test]
fn report_matches_runnable_gguf_by_revision_digest_and_exact_size() {
    use rigspark_core::catalog::{Availability, CatalogBackend, GgufSource};
    let (rows, mappings, mut observations, mut catalog) = report_inputs();
    for observation in &mut observations {
        observation.source.files[0].file = "model.gguf".into();
    }
    let model = &mut catalog.models[0];
    model.source.weights = None;
    model.source.gguf = Some(GgufSource {
        repo: mappings[0].repo.clone(),
        revision: "a".repeat(40),
        file: "alias.gguf".into(),
        sha256: "b".repeat(64),
    });
    model.availability = Some(Availability::Runnable {
        backend: CatalogBackend::Llamacpp,
    });
    let report = evaluate(&rows, &mappings, &observations, &catalog).unwrap();
    assert!(report.complete);
    assert_eq!(report.covered_runnable, 1);
    catalog.models[0].quantizations[0].disk_bytes += 1.0;
    assert!(
        !evaluate(&rows, &mappings, &observations, &catalog)
            .unwrap()
            .complete
    );
}

#[test]
fn report_mlx_identity_excludes_configuration_and_tokenizer_files() {
    use rigspark_core::catalog::{Availability, CatalogBackend};
    let (rows, mappings, observations, mut catalog) = report_inputs();
    let model = &mut catalog.models[0];
    let mut source = model.source.weights.take().unwrap();
    for file in ["config.json", "tokenizer_config.json"] {
        source.files.push(PinnedFile {
            file: file.into(),
            sha256: "d".repeat(64),
            bytes: 100.0,
        });
    }
    model.source.mlx = Some(source);
    model.quantizations[0].disk_bytes = 1200.0;
    model.availability = Some(Availability::Runnable {
        backend: CatalogBackend::Mlx,
    });
    let report = evaluate(&rows, &mappings, &observations, &catalog).unwrap();
    assert!(report.complete);
    assert_eq!(report.covered_runnable, 1);
}

#[test]
fn report_rejects_empty_scope_and_ambiguous_publisher_associations() {
    let (mut rows, mappings, observations, catalog) = report_inputs();
    let mut conflicting = mappings.clone();
    let mut extra = mappings[0].clone();
    extra.repo = "Another/Publisher".into();
    conflicting.push(extra);
    assert!(evaluate(&rows, &conflicting, &observations, &catalog).is_err());
    rows.retain(|row| !row.is_open_weights);
    assert!(evaluate(&rows, &[], &[], &catalog).is_err());
    assert!(evaluate(&[], &[], &[], &catalog).is_err());
}

#[test]
fn inventory_rejects_deferred_manifest_instead_of_admitting_only_initial_rows() {
    let rows = rigspark_core::artificial_analysis::parse_index_html(include_str!(
        "../fixtures/artificial-analysis-inventory.html"
    ))
    .unwrap();
    let envelope = serde_json::json!({
        "slug":"artificial-analysis-intelligence-index",
        "initialModels":rows,
        "defaultSlugs":["qwen-example"],
        "manifest":{"path":"/data/public-manifest.txt","key":"synthetic-public-client-key"}
    });
    let html = format!(
        "<script>self.__next_f.push({})</script>",
        serde_json::json!([1, envelope.to_string()])
    );
    let error = rigspark_core::artificial_analysis::parse_index_html(&html).unwrap_err();
    assert!(error.to_string().contains("partial"));
}
