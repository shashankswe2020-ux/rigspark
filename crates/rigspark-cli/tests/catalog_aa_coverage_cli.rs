use rigspark_core::artificial_analysis::INDEX_URL;
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn setup(root: &Path) {
    let mut catalog: Value = serde_json::from_str(rigspark_core::MODELS_JSON).unwrap();
    let mut model = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model.get("provenance").is_none())
        .unwrap()
        .clone();
    model["source"] = json!({"weights":{"repo":"Publisher/Example","revision":"a".repeat(40),"files":[{"file":"model.safetensors","sha256":"b".repeat(64),"bytes":1000}]}});
    model["availability"] = json!({"status":"advisory-only","reason":"backend-format-unsupported"});
    model["quantizations"] =
        json!([{"name":"BF16","diskBytes":1000,"minRamBytes":1150,"minVramBytes":1150}]);
    catalog["models"] = json!([model]);
    catalog["schemaVersion"] = json!(4);
    fs::write(root.join("catalog.json"), catalog.to_string()).unwrap();
    fs::write(root.join("publishers.json"), json!([{
        "publisher":{"creatorSlug":"qwen","releaseSlug":"qwen-example-32b","repo":"Publisher/Example"},
        "files":["model.safetensors"]
    }]).to_string()).unwrap();
    let fixture = json!({
        INDEX_URL:{"status":200,"text":include_str!("../../rigspark-core/fixtures/artificial-analysis-inventory.html")},
        "https://huggingface.co/api/models/Publisher/Example?blobs=true":{"status":200,"json":{
            "id":"Publisher/Example","sha":"a".repeat(40),"private":false,"gated":false,"tags":["license:apache-2.0"],
            "siblings":[{"rfilename":"model.safetensors","lfs":{"sha256":"b".repeat(64),"size":1000}}]
        }}
    });
    fs::write(root.join("recorded.json"), fixture.to_string()).unwrap();
}
fn command(root: &Path) -> Command {
    command_with_out(root, "coverage.json")
}
fn command_with_out(root: &Path, out: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_llmup-catalog-aa-coverage"));
    command.current_dir(root).env("PATH", "").args([
        "--catalog-path",
        "catalog.json",
        "--publishers-path",
        "publishers.json",
        "--fixture",
        "recorded.json",
        "--now",
        "2026-10-10T00:00:00Z",
    ]);
    command.args(["--out", out]);
    command
}

#[test]
fn recorded_report_is_deterministic_and_check_does_not_write() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    let output = command(root.path()).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["complete"], true);
    assert_eq!(report["uniqueArtifacts"], 1);
    assert_eq!(report["coveredAdvisoryOnly"], 1);
    assert_eq!(report["inventory"].as_array().unwrap().len(), 3);
    assert_eq!(
        fs::read(root.path().join("coverage.json")).unwrap(),
        output.stdout
    );
    fs::write(root.path().join("coverage.json"), b"previous snapshot").unwrap();
    let checked = command(root.path())
        .arg("--check")
        .env("GITHUB_STEP_SUMMARY", root.path().join("summary.md"))
        .output()
        .unwrap();
    assert!(checked.status.success());
    assert_eq!(checked.stdout, output.stdout);
    assert_eq!(
        fs::read(root.path().join("coverage.json")).unwrap(),
        b"previous snapshot"
    );
    assert!(!root.path().join("summary.md").exists());
    fs::remove_file(root.path().join("coverage.json")).unwrap();
    assert!(
        command(root.path())
            .arg("--check")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!root.path().join("coverage.json").exists());
}

#[test]
fn incomplete_coverage_returns_nonzero_in_check_mode_and_names_the_gap() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    fs::write(root.path().join("catalog.json"), rigspark_core::MODELS_JSON).unwrap();
    let output = command(root.path()).arg("--check").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["complete"], false);
    assert_eq!(report["artifacts"][0]["status"], "missing-catalog");
    assert_eq!(
        report["artifacts"][0]["modelIds"],
        json!(["model-config-1", "model-config-2"])
    );
    assert!(!root.path().join("coverage.json").exists());
    fs::write(root.path().join("publishers.json"), "[]").unwrap();
    let output = command(root.path()).arg("--check").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["unmatched"][0]["reason"], "missing-publisher");
}

#[test]
fn upstream_errors_and_empty_inventory_preserve_the_previous_report_and_inputs() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    let original = fs::read(root.path().join("catalog.json")).unwrap();
    let recorded: Value =
        serde_json::from_slice(&fs::read(root.path().join("recorded.json")).unwrap()).unwrap();
    fs::write(
        root.path().join("coverage.json"),
        b"previous verified snapshot",
    )
    .unwrap();
    for response in [
        json!({"status":401,"text":"unauthorized"}),
        json!({"status":200,"text":"<html>no inventory</html>"}),
        json!({"status":200,"text":"<script>self.__next_f.push([1,\"{\\\"initialModels\\\":[]}\"])</script>"}),
    ] {
        let mut fixture = recorded.clone();
        fixture[INDEX_URL] = response;
        fs::write(root.path().join("recorded.json"), fixture.to_string()).unwrap();
        let output = command(root.path()).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
        assert_eq!(
            fs::read(root.path().join("coverage.json")).unwrap(),
            b"previous verified snapshot"
        );
        assert_eq!(
            fs::read(root.path().join("catalog.json")).unwrap(),
            original
        );
    }
}

#[test]
fn input_output_collisions_are_rejected_without_mutation() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    for name in ["catalog.json", "publishers.json", "recorded.json"] {
        let original = fs::read(root.path().join(name)).unwrap();
        let output = command_with_out(root.path(), name).output().unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read(root.path().join(name)).unwrap(), original);
    }
}

#[cfg(unix)]
#[test]
fn symlink_outputs_cannot_overwrite_inputs() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    let original = fs::read(root.path().join("catalog.json")).unwrap();
    std::os::unix::fs::symlink("catalog.json", root.path().join("coverage.json")).unwrap();
    assert!(!command(root.path()).output().unwrap().status.success());
    assert_eq!(
        fs::read(root.path().join("catalog.json")).unwrap(),
        original
    );
}

#[test]
fn hardlink_output_aliases_are_rejected_by_file_identity() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    let original = fs::read(root.path().join("catalog.json")).unwrap();
    fs::hard_link(
        root.path().join("catalog.json"),
        root.path().join("coverage.json"),
    )
    .unwrap();
    let output = command(root.path()).output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("alias an input"));
    assert_eq!(
        fs::read(root.path().join("catalog.json")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(root.path().join("coverage.json")).unwrap(),
        original
    );
}

#[test]
fn ambiguous_catalog_coverage_fails_check_and_publisher_errors_preserve_snapshot() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    let mut catalog: Value =
        serde_json::from_slice(&fs::read(root.path().join("catalog.json")).unwrap()).unwrap();
    let mut duplicate = catalog["models"][0].clone();
    duplicate["id"] = json!("duplicate-artifact");
    catalog["models"].as_array_mut().unwrap().push(duplicate);
    fs::write(root.path().join("catalog.json"), catalog.to_string()).unwrap();
    let output = command(root.path()).arg("--check").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["artifacts"][0]["status"], "ambiguous-catalog");
    fs::write(root.path().join("coverage.json"), b"verified snapshot").unwrap();
    let mut fixture: Value =
        serde_json::from_slice(&fs::read(root.path().join("recorded.json")).unwrap()).unwrap();
    fixture["https://huggingface.co/api/models/Publisher/Example?blobs=true"]["status"] =
        json!(403);
    fs::write(root.path().join("recorded.json"), fixture.to_string()).unwrap();
    let output = command(root.path()).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("Publisher/Example"));
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::read(root.path().join("coverage.json")).unwrap(),
        b"verified snapshot"
    );
}
