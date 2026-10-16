//! `cargo workflow-harvest` end to end against recorded upstream responses.
use rigspark_core::workflow::{Registry, Status, WORKFLOWS_JSON};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

const COMMIT: &str = "99e3d43745926b78466d99f937c0cd2bb622423a";
const FIXTURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../rigspark-runtime/tests/fixtures"
);

fn fixture(path: &str) -> String {
    fs::read_to_string(format!("{FIXTURES}/{path}")).unwrap()
}

fn setup(root: &Path) {
    let raw = |path: &str| {
        format!(
            "https://raw.githubusercontent.com/Comfy-Org/workflow_templates/{COMMIT}/templates/{path}"
        )
    };
    let hf = |repo: &str| format!("https://huggingface.co/api/models/{repo}?blobs=true");
    let hub: Value = serde_json::from_str(include_str!(
        "../../rigspark-core/fixtures/admission/hf-models.json"
    ))
    .unwrap();
    let mut recorded = json!({
        "https://api.github.com/repos/Comfy-Org/workflow_templates/commits/main": {"status": 200, "text": COMMIT},
        raw("index.json"): {"status": 200, "text": fixture("harvest/index.json")},
        hf("Comfy-Org/Wan_2.1_ComfyUI_repackaged"): {"status": 200, "text": fixture("harvest/hf-wan21.json")},
    });
    for template in [
        "text_to_video_wan",
        "image_qwen_image",
        "image_ming_image_01_design_t2i",
        "video_wan2_2_14B_t2v",
    ] {
        recorded[raw(&format!("{template}.json"))] =
            json!({"status": 200, "text": fixture(&format!("templates/{template}.json"))});
    }
    for repo in [
        "Comfy-Org/Qwen-Image_ComfyUI",
        "Comfy-Org/Wan_2.2_ComfyUI_Repackaged",
        "Comfy-Org/Ming-Image",
    ] {
        recorded[hf(repo)] = json!({"status": 200, "json": hub[repo]});
    }
    fs::write(root.join("upstream.json"), recorded.to_string()).unwrap();
    fs::write(root.join("workflows.json"), WORKFLOWS_JSON).unwrap();
    fs::write(root.join("generation.json"), rigspark_core::GENERATION_JSON).unwrap();
    fs::write(
        root.join("object-info.json"),
        fixture("comfyui-0.38.0-object-info.json"),
    )
    .unwrap();
}

fn harvest(root: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_llmup-workflow-harvest"))
        .current_dir(root)
        .env("PATH", "")
        .args([
            "--workflows-path",
            "workflows.json",
            "--catalog-path",
            "generation.json",
            "--object-info",
            "cpu=object-info.json",
            "--report-path",
            "report.json",
            "--fixture",
            "upstream.json",
        ])
        .args(extra)
        .output()
        .unwrap()
}

fn report(output: &std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn dry_run_reports_candidates_without_writing_anything() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    let report = report(&harvest(root.path(), &["--dry-run"]));
    assert_eq!(report["commit"], COMMIT);
    assert_eq!(report["candidates"].as_object().unwrap().len(), 3);
    assert_eq!(
        fs::read_to_string(root.path().join("workflows.json")).unwrap(),
        WORKFLOWS_JSON
    );
    assert!(!root.path().join("report.json").exists());
}

#[test]
fn write_mode_appends_candidates_canonically_and_reruns_are_idempotent() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    report(&harvest(root.path(), &[]));
    let written = fs::read_to_string(root.path().join("workflows.json")).unwrap();
    let registry = Registry::parse(&written).unwrap();
    assert_eq!(registry.len(), 9);
    assert_eq!(registry.to_json(), written, "written in canonical form");
    // Existing workflows are untouched; candidates are appended after them.
    assert!(written.starts_with(WORKFLOWS_JSON.trim_end_matches("\n  ]\n}\n")));
    let candidates: Vec<_> = registry
        .files()
        .iter()
        .filter(|file| file.status == Status::Candidate)
        .collect();
    assert_eq!(candidates.len(), 3);
    let first_report: Value =
        serde_json::from_str(&fs::read_to_string(root.path().join("report.json")).unwrap())
            .unwrap();
    assert!(first_report["complete"].as_bool().unwrap());

    let again = report(&harvest(root.path(), &[]));
    assert!(again["candidates"].as_object().unwrap().is_empty());
    assert_eq!(again["skipped"].as_object().unwrap().len(), 3);
    assert_eq!(
        fs::read_to_string(root.path().join("workflows.json")).unwrap(),
        written
    );
}

#[test]
fn invalid_object_info_specs_are_refused() {
    let root = tempfile::tempdir().unwrap();
    setup(root.path());
    for spec in ["object-info.json", "tpu=object-info.json", "cpu="] {
        let output = Command::new(env!("CARGO_BIN_EXE_llmup-workflow-harvest"))
            .current_dir(root.path())
            .env("PATH", "")
            .args([
                "--workflows-path",
                "workflows.json",
                "--catalog-path",
                "generation.json",
                "--fixture",
                "upstream.json",
                "--dry-run",
                "--object-info",
                spec,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{spec}");
    }
}
