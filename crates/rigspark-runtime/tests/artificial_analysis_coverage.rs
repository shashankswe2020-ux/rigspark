use rigspark_core::{
    artificial_analysis::{INDEX_URL, PublisherMapping},
    catalog::Catalog,
};
use rigspark_runtime::{
    admission::RecordedAdmissionTransport,
    artificial_analysis::{PublisherSelection, collect_coverage},
};
use serde_json::json;
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

const NOW: &str = "2026-10-10T00:00:00Z";

fn selection() -> PublisherSelection {
    PublisherSelection {
        publisher: PublisherMapping {
            creator_slug: "qwen".into(),
            release_slug: "qwen-example-32b".into(),
            repo: "Publisher/Example".into(),
        },
        files: vec!["model.safetensors".into()],
    }
}
fn upstream(status: u16) -> RecordedAdmissionTransport {
    let mut responses = BTreeMap::new();
    responses.insert(
        INDEX_URL.into(),
        (
            status,
            include_bytes!("../../rigspark-core/fixtures/artificial-analysis-inventory.html")
                .to_vec(),
        ),
    );
    responses.insert("https://huggingface.co/api/models/Publisher/Example?blobs=true".into(), (200, json!({
        "id":"Publisher/Example","sha":"a".repeat(40),"private":false,"gated":false,"tags":["license:apache-2.0"],
        "siblings":[{"rfilename":"model.safetensors","lfs":{"sha256":"b".repeat(64),"size":1000}}]
    }).to_string().into_bytes()));
    RecordedAdmissionTransport::new(responses).unwrap()
}

#[tokio::test]
async fn collection_replays_verified_sources_and_reports_uncovered_artifacts() {
    let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    let report = collect_coverage(
        &catalog,
        &[selection()],
        &upstream(200),
        NOW,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!report.coverage.complete);
    assert_eq!(report.coverage.unique_artifacts, 1);
    assert_eq!(report.coverage.collapsed_configurations, 1);
    assert_eq!(report.inventory.len(), 3);
    assert_eq!(report.evidence.len(), 1);
    use sha2::{Digest, Sha256};
    assert_eq!(
        report.source.sha256,
        format!(
            "{:x}",
            Sha256::digest(include_bytes!(
                "../../rigspark-core/fixtures/artificial-analysis-inventory.html"
            ))
        )
    );
    assert_eq!(report.source.checked_at, NOW);
    let repeated = collect_coverage(
        &catalog,
        &[selection(), selection()],
        &upstream(200),
        NOW,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        serde_json::to_value(repeated).unwrap()
    );
}

#[tokio::test]
async fn collection_never_substitutes_empty_inventory_for_upstream_failure() {
    let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    for status in [401, 403, 429, 500, 206] {
        assert!(
            collect_coverage(
                &catalog,
                &[selection()],
                &upstream(status),
                NOW,
                &CancellationToken::new()
            )
            .await
            .is_err()
        );
    }

    assert!(
        collect_coverage(
            &catalog,
            &[selection()],
            &upstream(200),
            "not-a-date",
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        collect_coverage(&catalog, &[selection()], &upstream(200), NOW, &cancel)
            .await
            .is_err()
    );
}

#[test]
fn inventory_transport_allowlist_accepts_only_the_exact_public_index() {
    use rigspark_runtime::admission::allowed_request;
    use url::Url;
    assert!(allowed_request(&Url::parse(INDEX_URL).unwrap()));
    for url in [
        format!("{INDEX_URL}?token=secret"),
        format!("{INDEX_URL}/private"),
        INDEX_URL.replace("https:", "http:"),
        INDEX_URL.replace("artificialanalysis.ai", "evil.test"),
        INDEX_URL.replace("https://", "https://user@"),
    ] {
        assert!(!allowed_request(&Url::parse(&url).unwrap()));
    }
}

#[tokio::test]
async fn admission_collects_verified_weights_and_blocks_activation_of_new_entries() {
    use rigspark_runtime::artificial_analysis::collect_admission;
    let mut catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
    catalog.models.retain(|model| {
        model.source.ollama.is_some()
            && model
                .quantizations
                .iter()
                .all(|quant| quant.sha256.is_some())
    });
    catalog.models.truncate(1);
    let before = serde_json::to_value(&catalog).unwrap();
    let report = collect_admission(
        &catalog,
        &[selection()],
        &upstream(200),
        NOW,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(report.coverage.complete);
    assert_eq!(report.coverage.covered_advisory_only, 1);
    let admitted = report.catalog.unwrap();
    let model = admitted
        .models
        .iter()
        .find(|model| model.is_advisory_only())
        .unwrap();
    assert!(model.ensure_runnable().is_err());
    assert_eq!(model.params, "unknown");
    assert!(model.context_length.is_none());
    assert_eq!(serde_json::to_value(&catalog).unwrap(), before);
    assert!(
        collect_admission(
            &catalog,
            &[selection()],
            &upstream(403),
            NOW,
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
}
