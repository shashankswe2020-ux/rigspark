use rigspark_core::catalog::Catalog;
use rigspark_runtime::catalog_quality::evaluate;
use serde_json::{Value, json};

const NOW: &str = "2026-09-30T12:00:00Z";

fn fixture() -> (String, Value) {
    fixture_count(10)
}

fn fixture_count(count: usize) -> (String, Value) {
    let mut catalog = Catalog::parse(include_str!(
        "../../rigspark-core/fixtures/catalog-baseline.json"
    ))
    .unwrap();
    catalog.generated_at = "2026-09-30T00:00:00Z".into();
    let template = catalog.models.remove(0);
    catalog.models = (0..count)
        .map(|index| {
            let mut model = template.clone();
            model.id = format!("fixture:{index}");
            model.source.ollama = Some(model.id.clone());
            model
        })
        .collect();
    let raw = serde_json::to_string(&catalog).unwrap();
    let evidence = json!({
        "policyVersion": 1,
        "scopes": [{"name":"ollama-local-variants", "checkedAt":NOW,
            "source":"https://ollama.com/library", "complete":true,
            "variants":catalog.models.iter().map(|model| &model.id).collect::<Vec<_>>() }],
        "observations":catalog.models.iter().map(|model| json!({"id":model.id, "checkedAt":NOW,
            "sources":["https://huggingface.co/Qwen/Qwen3.6-35B-A3B", format!("https://registry.ollama.ai/v2/library/fixture/manifests/{}", model.id.split(':').nth(1).unwrap())],
            "model": model})).collect::<Vec<_>>()
    });
    (raw, evidence)
}

#[test]
fn ninety_percent_passes_but_eighty_nine_does_not() {
    let (catalog, mut evidence) = fixture_count(100);
    evidence["observations"]
        .as_array_mut()
        .unwrap()
        .truncate(90);
    let report = evaluate(&catalog, &evidence.to_string(), NOW).unwrap();
    assert!(report.passed);
    assert_eq!(report.freshness_percent, Some(90.0));
    assert_eq!(report.correctness_percent, 90.0);
    evidence["observations"].as_array_mut().unwrap().pop();
    let report = evaluate(&catalog, &evidence.to_string(), NOW).unwrap();
    assert!(!report.passed);
    assert_eq!(report.correctness_percent, 89.0);
}

#[test]
fn generation_date_cannot_mask_stale_evidence_or_missing_models() {
    let (catalog, mut evidence) = fixture();
    for observation in evidence["observations"].as_array_mut().unwrap() {
        observation["checkedAt"] = json!("2026-09-20T00:00:00Z");
    }
    let report = evaluate(&catalog, &evidence.to_string(), NOW).unwrap();
    assert!(!report.passed);
    assert_eq!(report.freshness_percent, Some(0.0));
    assert_eq!(report.correctness_percent, 0.0);
    let (catalog, mut evidence) = fixture();
    evidence["scopes"][0]["variants"]
        .as_array_mut()
        .unwrap()
        .extend([json!("missing:1"), json!("missing:2")]);
    let report = evaluate(&catalog, &evidence.to_string(), NOW).unwrap();
    assert!(!report.passed);
    assert!(report.freshness_percent.unwrap() < 90.0);
}

#[test]
fn unknown_inventory_is_not_a_passing_score() {
    let (catalog, mut evidence) = fixture();
    evidence["scopes"][0]["complete"] = json!(false);
    let report = evaluate(&catalog, &evidence.to_string(), NOW).unwrap();
    assert!(!report.passed);
    assert_eq!(report.freshness_percent, None);
}

#[test]
fn one_known_contradiction_blocks_even_at_ninety_percent() {
    let (catalog, mut evidence) = fixture();
    evidence["observations"][0]["model"]["contextLength"] = json!(4096);
    let report = evaluate(&catalog, &evidence.to_string(), NOW).unwrap();
    assert_eq!(report.correctness_percent, 90.0);
    assert!(!report.passed);
    assert!(!report.blockers.is_empty());
}

#[test]
fn rejects_future_duplicate_and_unbound_evidence() {
    let (catalog, mut evidence) = fixture();
    let mut future_catalog: Value = serde_json::from_str(&catalog).unwrap();
    future_catalog["generatedAt"] = json!("2026-10-01T00:00:00Z");
    assert!(evaluate(&future_catalog.to_string(), &evidence.to_string(), NOW).is_err());
    evidence["observations"][0]["checkedAt"] = json!("2026-10-01T00:00:00Z");
    assert!(evaluate(&catalog, &evidence.to_string(), NOW).is_err());
    let (_, mut evidence) = fixture();
    let duplicate = evidence["observations"][0].clone();
    evidence["observations"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(evaluate(&catalog, &evidence.to_string(), NOW).is_err());
    let (_, mut evidence) = fixture();
    evidence["observations"][0]["id"] = json!("not-in-catalog");
    assert!(evaluate(&catalog, &evidence.to_string(), NOW).is_err());
}

#[test]
fn signed_quality_report_binds_exact_catalog_artifact_and_evidence() {
    use ed25519_dalek::{Signature, SigningKey};
    use rigspark_runtime::catalog_quality::sign_reviewed_catalog;
    use rigspark_runtime::catalog_update::{CatalogPayload, SignedCatalog, decode_hex};
    use sha2::{Digest, Sha256};
    let (catalog, evidence) = fixture();
    let key = SigningKey::from_bytes(&[9; 32]);
    let payload = CatalogPayload {
        format_version: 1,
        revision: 2,
        published_at: NOW.into(),
        catalog,
    };
    let (artifact, quality) = sign_reviewed_catalog(
        payload,
        &evidence.to_string(),
        &key.to_bytes(),
        &key.verifying_key().to_bytes(),
    )
    .unwrap();
    let envelope: SignedCatalog = serde_json::from_slice(&quality).unwrap();
    key.verifying_key()
        .verify_strict(
            envelope.payload.as_bytes(),
            &Signature::from_bytes(&decode_hex(&envelope.signature).unwrap()),
        )
        .unwrap();
    let report: Value = serde_json::from_str(&envelope.payload).unwrap();
    assert_eq!(
        report["catalogArtifactSha256"],
        format!("{:x}", Sha256::digest(&artifact))
    );
    assert_eq!(report["revision"], 2);
    assert_eq!(report["quality"]["freshnessPercent"], 100.0);
    assert_eq!(report["evidence"], evidence.to_string());
    assert_eq!(
        report["quality"]["evidenceSha256"],
        format!(
            "{:x}",
            Sha256::digest(report["evidence"].as_str().unwrap().as_bytes())
        )
    );
}

#[test]
fn unrelated_citations_and_known_geometry_contradictions_cannot_pass() {
    let (catalog, mut evidence) = fixture();
    evidence["observations"][0]["sources"] = json!(["https://example.org/copied-catalog"]);
    assert!(evaluate(&catalog, &evidence.to_string(), NOW).is_err());
    let (_, mut evidence) = fixture();
    let mut catalog: Value = serde_json::from_str(&catalog).unwrap();
    catalog["models"][0]["kvBytesPerToken"] = json!(4096);
    evidence["observations"][0]["model"]["kvBytesPerToken"] = json!(8192);
    assert!(
        !evaluate(&catalog.to_string(), &evidence.to_string(), NOW)
            .unwrap()
            .passed
    );
}

#[test]
fn seven_day_boundary_is_exact_and_stale_inventory_stays_unknown() {
    let (catalog, mut evidence) = fixture();
    for observation in evidence["observations"].as_array_mut().unwrap() {
        observation["checkedAt"] = json!("2026-09-23T12:00:00Z");
    }
    assert!(
        evaluate(&catalog, &evidence.to_string(), NOW)
            .unwrap()
            .passed
    );
    evidence["scopes"][0]["checkedAt"] = json!("2026-09-23T11:59:59Z");
    let report = evaluate(&catalog, &evidence.to_string(), NOW).unwrap();
    assert_eq!(report.freshness_percent, None);
    assert!(!report.passed);
}

#[test]
fn missing_artifact_pins_reduce_verification_and_digest_mismatches_block() {
    let (catalog, mut evidence) = fixture();
    let mut catalog: Value = serde_json::from_str(&catalog).unwrap();
    for index in [0, 1] {
        for document in [
            &mut catalog["models"][index],
            &mut evidence["observations"][index]["model"],
        ] {
            document["quantizations"][0]
                .as_object_mut()
                .unwrap()
                .remove("sha256");
            document["quantizations"][0]
                .as_object_mut()
                .unwrap()
                .remove("projectors");
        }
    }
    let report = evaluate(&catalog.to_string(), &evidence.to_string(), NOW).unwrap();
    assert_eq!(report.correctness_percent, 80.0);
    assert!(!report.passed);
    let (catalog, mut evidence) = fixture();
    evidence["observations"][0]["model"]["quantizations"][0]["sha256"] = json!("a".repeat(64));
    let report = evaluate(&catalog, &evidence.to_string(), NOW).unwrap();
    assert_eq!(report.correctness_percent, 90.0);
    assert!(!report.passed);
}

#[test]
fn gated_signer_refuses_missing_evidence_and_invalid_documents() {
    use ed25519_dalek::SigningKey;
    use rigspark_runtime::{
        catalog_quality::sign_reviewed_catalog, catalog_update::CatalogPayload,
    };
    let (catalog, _) = fixture();
    let key = SigningKey::from_bytes(&[9; 32]);
    let payload = CatalogPayload {
        format_version: 1,
        revision: 2,
        published_at: NOW.into(),
        catalog: catalog.clone(),
    };
    assert!(
        sign_reviewed_catalog(
            payload,
            r#"{"policyVersion":1,"scopes":[],"observations":[]}"#,
            &key.to_bytes(),
            &key.verifying_key().to_bytes()
        )
        .is_err()
    );
    assert!(evaluate(&catalog, "{}", NOW).is_err());
    let (_, mut evidence) = fixture();
    evidence["policyVersion"] = json!(2);
    assert!(evaluate(&catalog, &evidence.to_string(), NOW).is_err());
}

/// Turns every full observation into a partial one that checks only the listed facts.
fn partial(evidence: &mut Value, catalog: &str, fields: &[&str]) {
    let catalog: Value = serde_json::from_str(catalog).unwrap();
    for observation in evidence["observations"].as_array_mut().unwrap() {
        let model = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"] == observation["id"])
            .unwrap();
        let mut facts = serde_json::Map::new();
        for field in fields {
            let value = if *field == "defaultQuantization" {
                let mut quant = model["quantizations"][0].clone();
                for key in ["minRamBytes", "minVramBytes", "digestVerified"] {
                    quant.as_object_mut().unwrap().remove(key);
                }
                quant
            } else {
                model[*field].clone()
            };
            facts.insert(field.to_string(), value);
        }
        let tag = observation["id"]
            .as_str()
            .unwrap()
            .split(':')
            .nth(1)
            .unwrap()
            .to_string();
        let object = observation.as_object_mut().unwrap();
        object.remove("model");
        object.insert("facts".into(), Value::Object(facts));
        // Partial observations read only Ollama, so they cite only those reads.
        object.insert(
            "sources".into(),
            json!([
                format!("https://registry.ollama.ai/v2/library/fixture/manifests/{tag}"),
                format!("https://ollama.com/library/fixture:{tag}")
            ]),
        );
    }
}

#[test]
fn partial_observations_verify_only_the_facts_they_check_and_say_so() {
    let (catalog, mut evidence) = fixture();
    partial(
        &mut evidence,
        &catalog,
        &["defaultQuantization", "contextLength", "architecture"],
    );
    let report = evaluate(&catalog, &evidence.to_string(), NOW).unwrap();
    assert!(report.passed, "{:?}", report.blockers);
    let entry = serde_json::to_value(&report.entries[0]).unwrap();
    assert_eq!(entry["verified"], true);
    assert_eq!(
        entry["checkedFields"],
        json!(["architecture", "contextLength", "defaultQuantization"]),
        "the report names exactly what was verified"
    );

    let mut wrong = evidence.clone();
    wrong["observations"][0]["facts"]["contextLength"] = json!(4096);
    let report = evaluate(&catalog, &wrong.to_string(), NOW).unwrap();
    assert!(!report.passed);
    assert!(
        report
            .blockers
            .iter()
            .any(|blocker| blocker.contains("contradict"))
    );

    let mut digest = evidence.clone();
    digest["observations"][0]["facts"]["defaultQuantization"]["sha256"] = json!("f".repeat(64));
    assert!(!evaluate(&catalog, &digest.to_string(), NOW).unwrap().passed);

    for invalid in [
        json!({"params": "8B"}),
        json!({}),
        json!({"contextLength": 4096, "releaseDate": "2024-01-01"}),
        json!({"contextLength": 262144}),
    ] {
        let mut bad = evidence.clone();
        bad["observations"][0]["facts"] = invalid.clone();
        assert!(
            evaluate(&catalog, &bad.to_string(), NOW).is_err(),
            "{invalid}"
        );
    }
    let mut both = evidence.clone();
    both["observations"][0]["model"] =
        serde_json::from_str::<Value>(&catalog).unwrap()["models"][0].clone();
    assert!(
        evaluate(&catalog, &both.to_string(), NOW).is_err(),
        "model and facts are exclusive"
    );
}
