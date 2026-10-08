use rigspark_core::{
    catalog::Catalog,
    enrich::{Mode, enrich, parse_candidates},
};
use serde_json::{Value, json};

fn candidate(id: &str, date: &str) -> Value {
    json!({"id":id,"family":"fixture","params":"1B","architecture":"dense","license":"mit","openWeight":true,"contextLength":8192,"capabilities":["chat"],"releaseDate":date,"source":{"ollama":id},"quantizations":[{"name":"Q4_K_M","diskBytes":600000000}]})
}
#[test]
fn embedded_snapshot_is_structurally_valid_and_matches_catalog_refresh_contract() {
    let candidates = parse_candidates(include_str!("../fixtures/registry-snapshot.json")).unwrap();
    assert_eq!(candidates.len(), 69);
    let catalog = Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap();
    let result = enrich(
        &catalog,
        &candidates,
        Mode::Incremental,
        "2026-09-18T00:00:00.000Z",
        None,
    )
    .unwrap();
    assert!(result.diff.added.is_empty());
    assert!(result.diff.updated.is_empty());
    assert!(result.diff.removed.is_empty());
    assert_eq!(result.catalog.generated_at, catalog.generated_at);
}
fn empty() -> Catalog {
    Catalog {
        schema_version: 2,
        generated_at: "2026-01-01T00:00:00.000Z".into(),
        models: Vec::new(),
    }
}
fn raw(values: Vec<Value>) -> Vec<rigspark_core::enrich::RawModel> {
    parse_candidates(&serde_json::to_string(&values).unwrap()).unwrap()
}
#[test]
fn frozen_typescript_oracle_matches_complete_native_catalogs_and_diffs() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../fixtures/enrichment-parity.json")).unwrap();
    assert_eq!(cases.len(), 90);
    for case in cases {
        let input = &case["input"];
        let existing: Catalog = serde_json::from_value(input["existing"].clone()).unwrap();
        let candidates = parse_candidates(&input["candidates"].to_string()).unwrap();
        let mode = serde_json::from_value(input["mode"].clone()).unwrap();
        let result = enrich(
            &existing,
            &candidates,
            mode,
            input["now"].as_str().unwrap(),
            input["maxModels"].as_u64().map(|value| value as usize),
        )
        .unwrap();
        let mut expected_catalog: Catalog =
            serde_json::from_value(case["expected"]["catalog"].clone()).unwrap();
        // The frozen oracle predates schema v3; the version stamp is the only intended change.
        expected_catalog.schema_version = rigspark_core::catalog::SCHEMA_VERSION;
        assert_eq!(
            serde_json::to_value(&result.catalog).unwrap(),
            serde_json::to_value(expected_catalog).unwrap(),
            "catalog: {}",
            case["name"]
        );
        assert_eq!(
            serde_json::to_value(&result.diff).unwrap(),
            case["expected"]["diff"],
            "diff: {}",
            case["name"]
        );
    }
}

#[test]
fn invalid_existing_catalogs_and_null_candidate_fields_fail_closed() {
    let mut catalog = empty();
    catalog.generated_at = "not-a-date".into();
    assert!(
        enrich(
            &catalog,
            &[],
            Mode::Incremental,
            "2026-09-18T00:00:00Z",
            None
        )
        .is_err()
    );
    let mut catalog = Catalog::parse(include_str!("../../rigspark-core/data/models.json")).unwrap();
    catalog.models[0].open_weight = false;
    assert!(
        enrich(
            &catalog,
            &[],
            Mode::Incremental,
            "2026-09-18T00:00:00Z",
            None
        )
        .is_err()
    );
    let mut input = candidate("invalid:1b", "2026-01-01");
    input["activeParams"] = Value::Null;
    assert!(parse_candidates(&json!([input]).to_string()).is_err());
}
#[test]
fn backfill_sizes_and_is_idempotent_with_frozen_clock() {
    let candidates = raw(vec![candidate("fixture:1b", "2026-01-01")]);
    let first = enrich(
        &empty(),
        &candidates,
        Mode::Backfill,
        "2026-09-18T00:00:00.000Z",
        None,
    )
    .unwrap();
    assert_eq!(first.diff.added, ["fixture:1b"]);
    assert_eq!(
        first.catalog.models[0].quantizations[0].min_ram_bytes,
        690000000.0
    );
    let second = enrich(
        &first.catalog,
        &candidates,
        Mode::Backfill,
        "2026-09-19T00:00:00.000Z",
        None,
    )
    .unwrap();
    assert!(second.diff.updated.is_empty());
    assert_eq!(second.catalog.generated_at, first.catalog.generated_at);
}
#[test]
fn incremental_admits_newer_only_but_removes_withdrawn_licenses() {
    let existing = enrich(
        &empty(),
        &raw(vec![candidate("old:1b", "2026-02-01")]),
        Mode::Backfill,
        "2026-09-18T00:00:00.000Z",
        None,
    )
    .unwrap()
    .catalog;
    let mut withdrawn = candidate("old:1b", "2026-02-01");
    withdrawn["license"] = json!("proprietary");
    let result = enrich(
        &existing,
        &raw(vec![
            withdrawn,
            candidate("stale:1b", "2026-01-01"),
            candidate("new:1b", "2026-03-01"),
            candidate("future:1b", "2099-01-01"),
        ]),
        Mode::Incremental,
        "2026-09-18T00:00:00.000Z",
        None,
    )
    .unwrap();
    assert_eq!(result.diff.removed, ["old:1b"]);
    assert_eq!(result.diff.added, ["new:1b"]);
    assert_eq!(result.diff.skipped, ["future:1b"]);
}
#[test]
fn duplicates_use_last_value_and_caps_do_not_report_discarded_additions() {
    let result = enrich(
        &empty(),
        &raw(vec![
            candidate("a:1b", "2026-01-01"),
            candidate("b:1b", "2026-02-01"),
            candidate("a:1b", "2026-03-01"),
        ]),
        Mode::Backfill,
        "2026-09-18T00:00:00.000Z",
        Some(1),
    )
    .unwrap();
    assert_eq!(result.catalog.models[0].id, "a:1b");
    assert_eq!(result.diff.added, ["a:1b"]);
    assert_eq!(result.diff.capped, ["b:1b"]);
    assert!(enrich(&empty(), &[], Mode::Backfill, "invalid", None).is_err());
    assert!(
        enrich(
            &empty(),
            &[],
            Mode::Backfill,
            "2026-09-18T00:00:00Z",
            Some(0)
        )
        .is_err()
    );
    assert!(parse_candidates(r#"[{"id":"bad"}]"#).is_err());
}
