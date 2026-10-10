use rigspark_core::catalog::{Catalog, resolve};
use serde_json::{Value, json};

fn legacy() -> Value {
    let mut catalog: Value = serde_json::from_str(rigspark_core::MODELS_JSON).unwrap();
    let model = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model.get("provenance").is_none())
        .unwrap()
        .clone();
    catalog["models"] = json!([model]);
    catalog
}

fn advisory() -> Value {
    let mut catalog = legacy();
    catalog["schemaVersion"] = json!(4);
    let model = &mut catalog["models"][0];
    model["availability"] = json!({"status":"advisory-only","reason":"backend-format-unsupported"});
    model["source"] = json!({"weights":{
        "repo":"publisher/model",
        "revision":"a".repeat(40),
        "files":[{"file":"model.safetensors","bytes":1000.0,"sha256":"b".repeat(64)}]
    }});
    model["quantizations"] = json!([{
        "name":"BF16","diskBytes":1000.0,"minRamBytes":1150.0,"minVramBytes":1150.0
    }]);
    catalog
}

#[test]
fn schema_v4_advisory_weights_round_trip_without_an_ollama_source() {
    let raw = advisory();
    let loaded = Catalog::parse(&raw.to_string()).unwrap();
    assert_eq!(serde_json::to_value(&loaded).unwrap(), raw);
    assert!(resolve(&loaded, &loaded.models[0].id).is_ok());
    assert!(loaded.models[0].source.ollama.is_none());
    assert_eq!(
        loaded.models[0].ensure_runnable(),
        Err(rigspark_core::catalog::ModelUnavailable)
    );
    assert_eq!(
        loaded.models[0].sizing().quantizations[0].disk_bytes,
        1000.0
    );
}

#[test]
fn auto_advisory_entries_are_pinned_by_their_file_manifest() {
    let mut raw = advisory();
    raw["models"][0]["provenance"] = json!("auto");
    raw["models"][0]["addedAt"] = json!("2026-10-10");
    raw["models"][0]
        .as_object_mut()
        .unwrap()
        .remove("benchmarkProxy");
    assert!(Catalog::parse(&raw.to_string()).is_ok());
    raw["models"][0]["source"]["weights"]["files"][0]
        .as_object_mut()
        .unwrap()
        .remove("sha256");
    assert!(Catalog::parse(&raw.to_string()).is_err());
}

#[test]
fn legacy_schemas_keep_their_wire_shape_and_reject_v4_fields() {
    for version in [2, 3] {
        let mut raw = legacy();
        raw["schemaVersion"] = json!(version);
        assert_eq!(
            serde_json::to_value(Catalog::parse(&raw.to_string()).unwrap()).unwrap(),
            raw
        );
        raw["models"][0]["availability"] = json!({"status":"runnable","backend":"ollama"});
        assert!(Catalog::parse(&raw.to_string()).is_err());
        let mut raw = advisory();
        raw["schemaVersion"] = json!(version);
        raw["models"][0]
            .as_object_mut()
            .unwrap()
            .remove("availability");
        assert!(Catalog::parse(&raw.to_string()).is_err());
    }
}

#[test]
fn v4_requires_explicit_strict_availability() {
    let invalid = [
        Value::Null,
        json!({"status":"runnable"}),
        json!({"status":"advisory-only"}),
        json!({"status":"runnable","backend":"vllm"}),
        json!({"status":"advisory-only","reason":"unknown"}),
        json!({"status":"advisory-only","reason":"backend-format-unsupported","backend":"ollama"}),
        json!({"status":"runnable","backend":"ollama","reason":"backend-format-unsupported"}),
        json!({"status":"runnable","backend":"ollama\u{1b}"}),
    ];
    for availability in invalid {
        let mut raw = advisory();
        raw["models"][0]["availability"] = availability;
        assert!(Catalog::parse(&raw.to_string()).is_err(), "{raw}");
    }
    let mut raw = legacy();
    raw["schemaVersion"] = json!(4);
    assert!(Catalog::parse(&raw.to_string()).is_err());
}

#[test]
fn runnable_availability_requires_a_matching_integrity_pinned_source() {
    let mut raw = advisory();
    for backend in ["ollama", "llamacpp", "mlx", "lmstudio"] {
        raw["models"][0]["availability"] = json!({"status":"runnable","backend":backend});
        assert!(Catalog::parse(&raw.to_string()).is_err(), "{backend}");
    }
    raw["models"][0]["availability"] = json!({"status":"runnable","backend":"ollama"});
    raw["models"][0]["source"] = json!({"ollama":"example:1b"});
    assert!(Catalog::parse(&raw.to_string()).is_err());
    raw["models"][0]["quantizations"][0]["sha256"] = json!("b".repeat(64));
    assert!(Catalog::parse(&raw.to_string()).is_ok());
    raw["models"][0]["availability"]["backend"] = json!("llamacpp");
    raw["models"][0]["source"] = json!({"gguf":{
        "repo":"publisher/model","revision":"a".repeat(40),
        "file":"model.gguf","sha256":"b".repeat(64)
    }});
    assert!(Catalog::parse(&raw.to_string()).is_ok());
}

#[test]
fn advisory_weights_reject_unsafe_incomplete_or_inconsistent_manifests() {
    let source = advisory()["models"][0]["source"]["weights"].clone();
    for (field, value) in [
        ("revision", json!("main")),
        ("repo", json!("../../private")),
        ("files", json!([])),
        (
            "files",
            json!([{"file":"../model.safetensors","bytes":1000,"sha256":"b".repeat(64)}]),
        ),
        (
            "files",
            json!([{"file":"model.py","bytes":1000,"sha256":"b".repeat(64)}]),
        ),
        (
            "files",
            json!([{"file":"config.json","bytes":1000,"sha256":"b".repeat(64)}]),
        ),
        (
            "files",
            json!([{"file":"model.safetensors","bytes":999,"sha256":"b".repeat(64)}]),
        ),
        (
            "files",
            json!([{"file":"model.safetensors","bytes":1000,"sha256":"wrong"}]),
        ),
        (
            "files",
            json!([source["files"][0].clone(), source["files"][0].clone()]),
        ),
    ] {
        let mut raw = advisory();
        raw["models"][0]["source"]["weights"][field] = value;
        assert!(Catalog::parse(&raw.to_string()).is_err(), "{raw}");
    }
    let mut raw = advisory();
    raw["models"][0]["source"] = json!({"hf":"publisher/model"});
    assert!(Catalog::parse(&raw.to_string()).is_err());
}

#[test]
fn sharded_advisory_weights_have_exact_sizes_and_no_invented_aggregate_digest() {
    let mut raw = advisory();
    let first = &raw["models"][0]["source"]["weights"]["files"][0];
    let mut second = first.clone();
    second["file"] = json!("model-00002.safetensors");
    second["sha256"] = json!("c".repeat(64));
    raw["models"][0]["source"]["weights"]["files"] = json!([first, second]);
    raw["models"][0]["quantizations"][0]["diskBytes"] = json!(2000);
    assert!(Catalog::parse(&raw.to_string()).is_ok());
    raw["models"][0]["quantizations"][0]["sha256"] = json!("d".repeat(64));
    assert!(Catalog::parse(&raw.to_string()).is_err());
    let mut raw = advisory();
    raw["models"][0]["quantizations"][0]["sha256"] = json!("d".repeat(64));
    assert!(Catalog::parse(&raw.to_string()).is_err());
    raw["models"][0]["quantizations"][0]["sha256"] = json!("b".repeat(64));
    assert!(Catalog::parse(&raw.to_string()).is_ok());
}
