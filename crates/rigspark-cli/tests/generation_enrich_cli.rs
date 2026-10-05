use rigspark_core::{GENERATION_JSON, generation::GenerationCatalog};
use std::{fs, process::Command};

#[test]
fn writes_a_review_report_and_preserves_catalog_on_no_op() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("catalog.json"), GENERATION_JSON).unwrap();
    let catalog = GenerationCatalog::parse(GENERATION_JSON).unwrap();
    let image: Vec<_> = catalog
        .models
        .into_iter()
        .filter(|model| model.kind.name() == "image")
        .collect();
    fs::write(
        root.path().join("candidates.json"),
        serde_json::to_vec(&image).unwrap(),
    )
    .unwrap();
    let before = fs::read(root.path().join("catalog.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_llmup-generation-enrich"))
        .current_dir(root.path())
        .args([
            "image",
            "--catalog-path",
            "catalog.json",
            "--candidates",
            "candidates.json",
            "--report",
            "report.json",
            "--now",
            "2026-10-05T00:00:00Z",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(root.path().join("catalog.json")).unwrap(), before);
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(report["updated"], serde_json::json!([]));
    assert_eq!(report["rejected"], serde_json::json!([]));
}
