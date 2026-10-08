use rigspark_core::catalog::{Catalog, EntryProvenance, PerfDataset, RecencyBasis, resolve};
use serde_json::{Value, json};

fn catalog() -> Value {
    serde_json::from_str(include_str!("../../rigspark-core/data/models.json")).unwrap()
}

#[test]
fn loads_shipped_catalog_and_performance_evidence() {
    let loaded = Catalog::parse(&catalog().to_string()).unwrap();
    assert_eq!(loaded.models.len(), 69);
    assert!(
        !PerfDataset::parse(include_str!("../../rigspark-core/data/perf.json"))
            .unwrap()
            .classes
            .is_empty()
    );
    let entry = &loaded.models[0];
    assert_eq!(resolve(&loaded, &entry.id).unwrap().model.id, entry.id);
    let query = format!("{}-{}", entry.id, entry.quantizations[0].name);
    assert_eq!(
        resolve(&loaded, &query.to_uppercase())
            .unwrap()
            .quant
            .unwrap()
            .name,
        entry.quantizations[0].name
    );
}

#[test]
fn resolver_preserves_precedence_and_rejects_unsafe_input() {
    let mut value = catalog();
    value["models"] = json!([value["models"][0].clone(), value["models"][0].clone()]);
    value["models"][0]["id"] = json!("sample:small");
    value["models"][1]["id"] = json!("sample:large");
    value["models"][0]["family"] = json!("sample");
    value["models"][1]["family"] = json!("sample");
    let loaded = Catalog::parse(&value.to_string()).unwrap();
    assert_eq!(
        resolve(&loaded, " SAMPLE:SMALL ").unwrap().model.id,
        "sample:small"
    );
    assert_eq!(
        resolve(&loaded, "sample:l").unwrap().model.id,
        "sample:large"
    );
    assert_eq!(
        resolve(&loaded, "sample").unwrap_err().candidates,
        ["sample:large", "sample:small"]
    );
    assert!(
        resolve(&loaded, "missing")
            .unwrap_err()
            .candidates
            .is_empty()
    );
    for query in ["", "../escape", "name;command", "-flag"] {
        assert!(resolve(&loaded, query).is_err());
    }
}

#[test]
fn rejects_schema_and_source_integrity_failures() {
    for (field, bad) in [
        ("architecture", json!("other")),
        ("openWeight", json!(false)),
        ("releaseDate", json!("2026-02-30")),
        ("license", json!("closed")),
        ("capabilities", json!([])),
        ("source", json!({})),
        ("unexpected", json!(true)),
    ] {
        let mut value = catalog();
        value["models"][0][field] = bad;
        assert!(Catalog::parse(&value.to_string()).is_err(), "{field}");
    }
    let mut value = catalog();
    value["models"][0]["source"] = json!({"gguf":{"repo":"owner/repo","revision":"a".repeat(40),"file":"../bad.gguf","sha256":"b".repeat(64)}});
    assert!(Catalog::parse(&value.to_string()).is_err());
    value["models"][0]["source"] = json!({"mlx":{"repo":"owner/repo","revision":"a".repeat(40),"files":[
        {"file":"config.json","bytes":1,"sha256":"b".repeat(64)},
        {"file":"tokenizer_config.json","bytes":1,"sha256":"b".repeat(64)},
        {"file":"model.py","bytes":1,"sha256":"b".repeat(64)}]}});
    assert!(Catalog::parse(&value.to_string()).is_err());
}

#[test]
fn rejects_overlapping_performance_classes_and_untrusted_efficiency() {
    let mut value: Value =
        serde_json::from_str(include_str!("../../rigspark-core/data/perf.json")).unwrap();
    value["classes"][1] = value["classes"][0].clone();
    assert!(PerfDataset::parse(&value.to_string()).is_err());
    value["classes"][1]["id"] = json!("another");
    assert!(PerfDataset::parse(&value.to_string()).is_err());
    value["classes"] = json!([value["classes"][0].clone()]);
    value["classes"][0]["sources"]["efficiencyByBackend"]["llamacpp"]["trustTier"] =
        json!("low-confidence");
    assert!(PerfDataset::parse(&value.to_string()).is_err());
}

#[test]
fn performance_figures_citations_and_provenance_are_strictly_validated() {
    let shipped: Value =
        serde_json::from_str(include_str!("../../rigspark-core/data/perf.json")).unwrap();
    assert!(PerfDataset::parse("{not json").is_err());
    type Mutation = (&'static str, fn(&mut Value));
    let mutations: Vec<Mutation> = vec![
        ("negative bandwidth", |class| {
            class["memBandwidthGBps"] = json!(-1)
        }),
        ("missing bandwidth", |class| {
            class.as_object_mut().unwrap().remove("memBandwidthGBps");
        }),
        ("efficiency above 1", |class| {
            class["efficiency"] = json!(1.1)
        }),
        ("zero efficiency", |class| class["efficiency"] = json!(0)),
        ("missing citation", |class| {
            class["sources"]
                .as_object_mut()
                .unwrap()
                .remove("bandwidth");
        }),
        ("empty citation", |class| {
            class["sources"]["efficiency"] = json!("")
        }),
        ("inverted range", |class| {
            class["maxBytes"] = class["minBytes"].clone()
        }),
        ("scalar without provenance", |class| {
            class["sources"]
                .as_object_mut()
                .unwrap()
                .remove("efficiencyByBackend");
        }),
        ("provenance value differs", |class| {
            class["sources"]["efficiencyByBackend"]["llamacpp"]["value"] = json!(0.59)
        }),
        ("scalar above 1", |class| {
            class["efficiencyByBackend"]["llamacpp"] = json!(1.5);
            class["sources"]["efficiencyByBackend"]["llamacpp"]["value"] = json!(1.5);
        }),
        ("zero scalar", |class| {
            class["efficiencyByBackend"]["llamacpp"] = json!(0);
            class["sources"]["efficiencyByBackend"]["llamacpp"]["value"] = json!(0);
        }),
        ("unknown backend", |class| {
            class["efficiencyByBackend"] = json!({"vllm": 0.6});
            class["sources"]["efficiencyByBackend"] =
                json!({"vllm": class["sources"]["efficiencyByBackend"]["llamacpp"].clone()});
        }),
        ("unknown provenance key", |class| {
            class["sources"]["efficiencyByBackend"]["llamacpp"]["extra"] = json!(true)
        }),
        ("non-URL provenance", |class| {
            class["sources"]["efficiencyByBackend"]["llamacpp"]["url"] = json!("not a url")
        }),
    ];
    for (label, mutate) in mutations {
        let mut value = shipped.clone();
        mutate(&mut value["classes"][0]);
        assert!(PerfDataset::parse(&value.to_string()).is_err(), "{label}");
    }
    let mut optional = shipped.clone();
    optional["classes"][0]
        .as_object_mut()
        .unwrap()
        .remove("efficiencyByBackend");
    optional["classes"][0]["sources"]
        .as_object_mut()
        .unwrap()
        .remove("efficiencyByBackend");
    assert!(PerfDataset::parse(&optional.to_string()).is_ok());
    let mut version = shipped;
    version["schemaVersion"] = json!(2);
    assert!(PerfDataset::parse(&version.to_string()).is_err());
}

#[test]
fn sanitizes_loaded_display_fields_and_checks_sanitized_duplicates() {
    let mut value = catalog();
    value["models"][0]["family"] = json!("\u{1b}[31mfamily\u{1b}[0m\u{202e}");
    assert_eq!(
        Catalog::parse(&value.to_string()).unwrap().models[0].family,
        "family"
    );
    value["models"] = json!([value["models"][0].clone(), value["models"][0].clone()]);
    value["models"][1]["id"] = json!(format!(
        "{}\u{202e}",
        value["models"][0]["id"].as_str().unwrap()
    ));
    assert!(Catalog::parse(&value.to_string()).is_err());
    let mut perf: Value =
        serde_json::from_str(include_str!("../../rigspark-core/data/perf.json")).unwrap();
    perf["classes"][0]["label"] = json!("\u{1b}[31mlabel\u{1b}[0m");
    assert_eq!(
        PerfDataset::parse(&perf.to_string()).unwrap().classes[0].label,
        "label"
    );
}

fn auto_entry(mut value: Value) -> Value {
    let model = &mut value["models"][0];
    let object = model.as_object_mut().unwrap();
    object.remove("releaseDate");
    object.remove("benchmarkProxy");
    object.insert("provenance".into(), json!("auto"));
    object.insert("addedAt".into(), json!("2026-10-07"));
    value["schemaVersion"] = json!(3);
    value
}

#[test]
fn schema_v3_accepts_auto_entries_and_keeps_v2_compatible() {
    let v2 = catalog();
    let mut legacy = v2.clone();
    legacy["schemaVersion"] = json!(2);
    assert!(
        Catalog::parse(&legacy.to_string()).is_ok(),
        "v2 still parses"
    );

    let loaded = Catalog::parse(&auto_entry(v2.clone()).to_string()).unwrap();
    let model = &loaded.models[0];
    assert_eq!(model.provenance, EntryProvenance::Auto);
    assert_eq!(model.release_date, None);
    assert_eq!(model.recency(), Some(("2026-10-07", RecencyBasis::Added)));
    let curated = &loaded.models[1];
    assert_eq!(curated.provenance, EntryProvenance::Curated);
    assert_eq!(curated.recency().unwrap().1, RecencyBasis::Released);

    let encoded = serde_json::to_value(&loaded).unwrap();
    assert_eq!(encoded["models"][0]["provenance"], "auto");
    assert!(
        encoded["models"][1].get("provenance").is_none(),
        "curated is the implicit default"
    );
}

#[test]
fn schema_v3_rejects_unsourced_or_unpinned_auto_entries_and_v3_fields_in_v2() {
    let base = auto_entry(catalog());
    let reject = |edit: &dyn Fn(&mut Value)| {
        let mut value = base.clone();
        edit(&mut value);
        Catalog::parse(&value.to_string()).is_err()
    };
    assert!(reject(&|value| {
        value["models"][0]
            .as_object_mut()
            .unwrap()
            .remove("addedAt");
    }));
    assert!(reject(
        &|value| value["models"][0]["benchmarkProxy"] = json!(0.5)
    ));
    assert!(reject(&|value| {
        value["models"][0]["quantizations"][0]
            .as_object_mut()
            .unwrap()
            .remove("sha256");
    }));
    assert!(reject(
        &|value| value["models"][0]["addedAt"] = json!("2026-13-01")
    ));
    assert!(
        reject(&|value| value["schemaVersion"] = json!(2)),
        "v2 cannot carry auto entries"
    );
    assert!(reject(&|value| value["schemaVersion"] = json!(4)));
    assert!(
        reject(&|value| {
            value["models"][1]
                .as_object_mut()
                .unwrap()
                .remove("releaseDate");
        }),
        "curated entries keep a release date"
    );
}

#[test]
fn recency_window_keeps_entries_released_or_added_within_calendar_months() {
    use rigspark_core::catalog::months_before;
    assert_eq!(
        months_before("2026-03-31", 1).unwrap().to_string(),
        "2026-02-28"
    );
    assert_eq!(
        months_before("2026-01-15", 2).unwrap().to_string(),
        "2025-11-15"
    );
    assert!(months_before("2026-01-15", 0).is_err() && months_before("2026-01-15", 4).is_err());

    let mut value = auto_entry(catalog());
    value["models"][1]["releaseDate"] = json!("2026-09-20");
    value["models"][2]["releaseDate"] = json!("2026-06-01");
    let mut loaded = Catalog::parse(&value.to_string()).unwrap();
    let total = loaded.models.len();
    loaded.retain_recent("2026-10-07", 1).unwrap();
    let ids: Vec<&str> = loaded
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect();
    assert!(
        ids.contains(&value["models"][0]["id"].as_str().unwrap()),
        "added 2026-10-07 counts"
    );
    assert!(
        ids.contains(&value["models"][1]["id"].as_str().unwrap()),
        "released 2026-09-20 counts"
    );
    assert!(!ids.contains(&value["models"][2]["id"].as_str().unwrap()));
    assert!(loaded.models.len() < total);
}
