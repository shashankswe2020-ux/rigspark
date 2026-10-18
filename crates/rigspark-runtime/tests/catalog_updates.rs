use ed25519_dalek::{Signer, SigningKey};
use rigspark_runtime::catalog_update::{MAX_ARTIFACT_BYTES, verify};
use serde_json::{Value, json};

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

fn fixture_catalog() -> String {
    let mut catalog: Value = serde_json::from_str(include_str!(
        "../../rigspark-core/fixtures/catalog-baseline.json"
    ))
    .unwrap();
    catalog["generatedAt"] = json!("2026-09-30T00:00:00Z");
    catalog.to_string()
}

fn payload(revision: u64) -> Value {
    json!({
        "formatVersion": 1,
        "revision": revision,
        "publishedAt": "2026-09-30T00:00:00Z",
        "catalog": fixture_catalog(),
    })
}

fn signed(value: Value) -> Vec<u8> {
    let payload = serde_json::to_string(&value).unwrap();
    let signature = key().sign(payload.as_bytes());
    let signature: String = signature
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    serde_json::to_vec(&json!({"payload": payload, "signature": signature})).unwrap()
}

#[test]
fn signed_catalog_is_verified_before_use() {
    let snapshot = verify(&signed(payload(1)), &key().verifying_key().to_bytes()).unwrap();
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.catalog.models.len(), 69);
    assert_eq!(snapshot.digest.len(), 64);
}

#[test]
fn signed_v4_catalog_preserves_advisory_availability_and_rejects_missing_status() {
    let mut catalog: Value = serde_json::from_str(&fixture_catalog()).unwrap();
    catalog["schemaVersion"] = json!(4);
    catalog["models"] = json!([catalog["models"][0].clone()]);
    catalog["models"][0]["availability"] =
        json!({"status":"advisory-only","reason":"backend-format-unsupported"});
    catalog["models"][0]["source"] = json!({"weights":{
        "repo":"publisher/model","revision":"a".repeat(40),
        "files":[{"file":"model.safetensors","bytes":1000,"sha256":"b".repeat(64)}]
    }});
    catalog["models"][0]["quantizations"] =
        json!([{"name":"BF16","diskBytes":1000,"minRamBytes":1150,"minVramBytes":1150}]);
    let mut document = payload(2);
    document["catalog"] = json!(catalog.to_string());
    let verified = verify(&signed(document.clone()), &key().verifying_key().to_bytes()).unwrap();
    assert!(verified.catalog.models[0].is_advisory_only());
    catalog["models"][0]
        .as_object_mut()
        .unwrap()
        .remove("availability");
    document["catalog"] = json!(catalog.to_string());
    assert!(verify(&signed(document), &key().verifying_key().to_bytes()).is_err());
}

#[test]
fn rejects_tampering_wrong_key_and_unsigned_catalogs() {
    let artifact = signed(payload(1));
    let mut changed: Value = serde_json::from_slice(&artifact).unwrap();
    changed["payload"] = json!(
        changed["payload"]
            .as_str()
            .unwrap()
            .replace("2026-09-30", "2026-09-29")
    );
    assert!(
        verify(
            &serde_json::to_vec(&changed).unwrap(),
            &key().verifying_key().to_bytes()
        )
        .is_err()
    );
    assert!(
        verify(
            &artifact,
            &SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes()
        )
        .is_err()
    );
    assert!(
        verify(
            rigspark_core::MODELS_JSON.as_bytes(),
            &key().verifying_key().to_bytes()
        )
        .is_err()
    );
}

#[test]
fn rejects_incompatible_or_invalid_signed_payloads() {
    for (field, value) in [
        ("formatVersion", json!(2)),
        ("revision", json!(0)),
        ("publishedAt", json!("yesterday")),
        ("catalog", json!("{}")),
    ] {
        let mut invalid = payload(1);
        invalid[field] = value;
        assert!(
            verify(&signed(invalid), &key().verifying_key().to_bytes()).is_err(),
            "{field}"
        );
    }
    let mut invalid = payload(1);
    let mut catalog: Value = serde_json::from_str(&fixture_catalog()).unwrap();
    catalog["schemaVersion"] = json!(99);
    invalid["catalog"] = json!(serde_json::to_string(&catalog).unwrap());
    assert!(verify(&signed(invalid), &key().verifying_key().to_bytes()).is_err());
}

#[test]
fn publication_before_catalog_generation_is_rejected() {
    let mut invalid = payload(1);
    let mut catalog: Value = serde_json::from_str(&fixture_catalog()).unwrap();
    catalog["generatedAt"] = json!("2026-10-01T00:00:00Z");
    invalid["catalog"] = json!(catalog.to_string());
    assert!(verify(&signed(invalid), &key().verifying_key().to_bytes()).is_err());
}

#[test]
fn rejects_oversized_artifacts_before_parsing() {
    assert!(
        verify(
            &vec![b' '; MAX_ARTIFACT_BYTES + 1],
            &key().verifying_key().to_bytes()
        )
        .is_err()
    );
}

#[test]
fn publication_round_trips_and_refuses_wrong_signing_key() {
    use rigspark_runtime::catalog_update::{CatalogPayload, sign_catalog};
    let document = || CatalogPayload {
        format_version: 1,
        revision: 9,
        published_at: "2026-09-30T00:00:00Z".into(),
        catalog: fixture_catalog(),
    };
    let artifact = sign_catalog(
        document(),
        &key().to_bytes(),
        &key().verifying_key().to_bytes(),
    )
    .unwrap();
    assert_eq!(
        verify(&artifact, &key().verifying_key().to_bytes())
            .unwrap()
            .revision,
        9
    );
    assert!(sign_catalog(document(), &[8; 32], &key().verifying_key().to_bytes()).is_err());
    let mut invalid = document();
    invalid.catalog = "{}".into();
    assert!(
        sign_catalog(
            invalid,
            &key().to_bytes(),
            &key().verifying_key().to_bytes()
        )
        .is_err()
    );
}

#[test]
fn offline_store_activates_updates_and_preserves_data_on_rejection() {
    use rigspark_runtime::catalog_update::CatalogStore;
    let home = tempfile::tempdir().unwrap();
    let store = CatalogStore::new(
        home.path().join("home"),
        Some(key().verifying_key().to_bytes()),
    );
    assert_eq!(store.load().unwrap().status.source, "bundled");
    assert!(!home.path().join("home").exists());
    store.install(&signed(payload(2))).unwrap();
    assert_eq!(store.load().unwrap().status.revision, Some(2));
    assert!(store.install(&signed(payload(1))).is_err());
    let mut reused = payload(2);
    reused["publishedAt"] = json!("2026-10-01T00:00:00Z");
    assert!(store.install(&signed(reused)).is_err());
    assert!(store.install(b"broken").is_err());
    assert_eq!(store.load().unwrap().status.revision, Some(2));
    store.install(&signed(payload(2))).unwrap();
    assert_eq!(store.load().unwrap().status.revision, Some(2));
}

#[test]
fn corrupt_active_snapshot_recovers_previous_without_allowing_rollback() {
    use rigspark_runtime::catalog_update::CatalogStore;
    let home = tempfile::tempdir().unwrap();
    let store = CatalogStore::new(home.path(), Some(key().verifying_key().to_bytes()));
    store.install(&signed(payload(1))).unwrap();
    store.install(&signed(payload(3))).unwrap();
    let path = home.path().join("catalog/snapshots.json");
    let mut state: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    state["current"]["signature"] = json!("00".repeat(64));
    std::fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.status.source, "previous");
    assert_eq!(loaded.status.revision, Some(1));
    assert!(!loaded.status.warnings.is_empty());
    assert!(store.install(&signed(payload(2))).is_err());
    store.install(&signed(payload(4))).unwrap();
    assert_eq!(store.load().unwrap().status.revision, Some(4));
}

#[test]
fn corrupt_store_falls_back_visibly_and_missing_key_fails_closed() {
    use rigspark_runtime::catalog_update::CatalogStore;
    let home = tempfile::tempdir().unwrap();
    let store = CatalogStore::new(home.path(), Some(key().verifying_key().to_bytes()));
    store.install(&signed(payload(1))).unwrap();
    std::fs::write(home.path().join("catalog/snapshots.json"), b"broken").unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.status.source, "bundled");
    assert!(!loaded.status.warnings.is_empty());
    assert!(store.install(&signed(payload(2))).is_err());
    let unconfigured = CatalogStore::new(home.path(), None);
    assert!(unconfigured.install(&signed(payload(2))).is_err());
    assert!(!unconfigured.load().unwrap().status.updates_configured);
}

#[cfg(unix)]
#[test]
fn cache_symlinks_cannot_redirect_writes() {
    use rigspark_runtime::catalog_update::CatalogStore;
    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), home.path().join("catalog")).unwrap();
    let store = CatalogStore::new(home.path(), Some(key().verifying_key().to_bytes()));
    assert!(store.install(&signed(payload(1))).is_err());
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    assert!(!store.load().unwrap().status.warnings.is_empty());
}

struct FixtureTransport(Result<Vec<u8>, &'static str>);

#[async_trait::async_trait]
impl rigspark_runtime::catalog_update::CatalogTransport for FixtureTransport {
    async fn download(
        &self,
    ) -> Result<Vec<u8>, rigspark_runtime::catalog_update::CatalogUpdateError> {
        self.0
            .clone()
            .map_err(|_| rigspark_runtime::catalog_update::CatalogUpdateError::Download)
    }
}

#[tokio::test]
async fn explicit_update_uses_injected_transport_and_preserves_cache_on_failure() {
    use rigspark_runtime::catalog_update::CatalogStore;
    let home = tempfile::tempdir().unwrap();
    let store = CatalogStore::new(home.path(), Some(key().verifying_key().to_bytes()));
    store
        .update(&FixtureTransport(Ok(signed(payload(1)))))
        .await
        .unwrap();
    assert!(
        store
            .update(&FixtureTransport(Err("offline")))
            .await
            .is_err()
    );
    assert!(
        store
            .update(&FixtureTransport(Ok(vec![0; MAX_ARTIFACT_BYTES + 1])))
            .await
            .is_err()
    );
    assert_eq!(store.load().unwrap().status.revision, Some(1));
}

#[test]
fn update_lock_is_exclusive_and_new_models_need_no_binary_rebuild() {
    use rigspark_runtime::catalog_update::CatalogStore;
    let home = tempfile::tempdir().unwrap();
    let store = CatalogStore::new(home.path(), Some(key().verifying_key().to_bytes()));
    store.install(&signed(payload(1))).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(home.path().join("catalog/update.lock"))
        .unwrap();
    lock.lock().unwrap();
    assert!(matches!(
        store.install(&signed(payload(2))),
        Err(rigspark_runtime::catalog_update::CatalogUpdateError::Busy)
    ));
    lock.unlock().unwrap();
    let mut candidate = payload(2);
    let mut catalog: Value = serde_json::from_str(&fixture_catalog()).unwrap();
    catalog["models"][0]["id"] = json!("fixture-new-model:4b");
    candidate["catalog"] = json!(serde_json::to_string(&catalog).unwrap());
    store.install(&signed(candidate)).unwrap();
    assert!(
        store
            .load()
            .unwrap()
            .catalog
            .models
            .iter()
            .any(|model| model.id == "fixture-new-model:4b")
    );
}
