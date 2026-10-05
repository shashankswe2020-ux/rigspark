use rigspark_core::{
    GENERATION_JSON,
    generation::{GenerationCatalog, GenerationKind, GenerationModel, enrich_generation_catalog},
};

fn catalog() -> GenerationCatalog {
    GenerationCatalog::parse(GENERATION_JSON).unwrap()
}

fn candidate(id: &str, kind: GenerationKind) -> GenerationModel {
    let catalog = catalog();
    let mut model = catalog
        .models
        .into_iter()
        .find(|model| model.kind == kind)
        .unwrap();
    model.id = id.into();
    model.default = false;
    model
}

#[test]
fn enriches_only_the_requested_allowlisted_workflow_family() {
    let mut replacement = candidate("flux1-schnell:fp8", GenerationKind::Image);
    replacement.files[0].bytes += 1;
    let addition = candidate("flux1-schnell:test-addition", GenerationKind::Image);
    let wrong_kind = candidate("wan2.1-t2v:test", GenerationKind::Video);
    let mut unknown_family = candidate("sdxl:test", GenerationKind::Image);
    unknown_family.family = "sdxl".into();

    let result = enrich_generation_catalog(
        &catalog(),
        vec![replacement, addition, wrong_kind, unknown_family],
        GenerationKind::Image,
        "2026-10-05T00:00:00Z",
    )
    .unwrap();

    assert_eq!(
        result.updated,
        vec!["flux1-schnell:fp8", "flux1-schnell:test-addition"]
    );
    assert_eq!(result.rejected.len(), 2);
    assert!(
        result
            .rejected
            .iter()
            .any(|entry| entry.id == "wan2.1-t2v:test" && entry.reason.contains("requested kind"))
    );
    assert!(
        result
            .rejected
            .iter()
            .any(|entry| entry.id == "sdxl:test" && entry.reason.contains("built-in workflow"))
    );
    assert_eq!(result.catalog.generated_at, "2026-10-05T00:00:00Z");
    assert!(
        result
            .catalog
            .models
            .iter()
            .find(|model| model.id == "flux1-schnell:fp8")
            .unwrap()
            .default
    );
    GenerationCatalog::parse(&serde_json::to_string(&result.catalog).unwrap()).unwrap();
}

#[test]
fn invalid_candidates_are_reported_and_a_no_op_preserves_the_catalog() {
    let mut invalid = candidate("flux1-schnell:broken", GenerationKind::Image);
    invalid.files[0].sha256 = "not-a-digest".into();
    let original = catalog();
    let result = enrich_generation_catalog(
        &original,
        vec![invalid],
        GenerationKind::Image,
        "2026-10-05T00:00:00Z",
    )
    .unwrap();

    assert!(result.updated.is_empty());
    assert_eq!(result.rejected.len(), 1);
    assert!(result.rejected[0].reason.contains("SHA-256"));
    assert_eq!(
        serde_json::to_string(&result.catalog).unwrap(),
        serde_json::to_string(&original).unwrap()
    );
}

#[test]
fn rejects_invalid_clock_before_processing_candidates() {
    let error = enrich_generation_catalog(&catalog(), vec![], GenerationKind::Video, "not-a-time")
        .unwrap_err();
    assert!(error.to_string().contains("timestamp"));
}
