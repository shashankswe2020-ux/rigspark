use std::{fs, process::Command};

#[test]
fn fixture_proposals_are_offline_bounded_and_do_not_change_catalog() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("fixture.json"),
        r#"{"inventory":["new-model"],"responses":{}}"#,
    )
    .unwrap();
    fs::write(root.path().join("catalog.json"), rigspark_core::MODELS_JSON).unwrap();
    let run = |limit: &str| {
        Command::new(env!("CARGO_BIN_EXE_llmup-catalog-propose"))
            .current_dir(root.path())
            .args([
                "--catalog-path",
                "catalog.json",
                "--fixture",
                "fixture.json",
                "--out",
                "proposals.json",
                "--limit",
                limit,
                "--now",
                "2026-09-30T00:00:00Z",
            ])
            .env("PATH", "")
            .env("OPENAI_API_KEY", "test-must-not-be-used")
            .env("OPENAI_CATALOG_MODEL", "test")
            .output()
            .unwrap()
    };
    assert!(!run("11").status.success());
    assert!(!root.path().join("proposals.json").exists());
    let output = run("10");
    assert!(
        output.status.code() == Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join("proposals.json")).unwrap()).unwrap();
    assert_eq!(report["inventoryComplete"], false);
    assert_eq!(report["requiresReview"], true);
    assert_eq!(report["proposals"][0]["status"], "unavailable");
    assert_eq!(
        fs::read_to_string(root.path().join("catalog.json")).unwrap(),
        rigspark_core::MODELS_JSON
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("test-must-not-be-used"));
}

#[test]
fn workflow_enrichment_scopes_credentials_and_keeps_review_and_signing_separate() {
    let workflow = include_str!("../../../.github/workflows/catalog-refresh.yml");
    for required in [
        "github.ref == 'refs/heads/main'",
        "timeout-minutes: 45",
        "cargo test --locked -p rigspark-runtime --test catalog_proposals",
        "cargo build --locked -p rigspark-cli --bin llmup-catalog-propose",
        "OPENAI_API_KEY: ${{ secrets.OPENAI_API_KEY }}",
        "OPENAI_CATALOG_MODEL: ${{ vars.OPENAI_CATALOG_MODEL }}",
        "target/debug/llmup-catalog-propose",
        "--source-only",
        "--resume-from",
        "candidate_limit:",
        "use_ai:",
        "needs.pending-review.outputs.proceed == 'true'",
        "GITHUB_STEP_SUMMARY",
        "nextAfter",
        "extractionError",
        "exit 2",
        "cargo catalog-quality",
        "git add docs/references/catalog-proposals.json",
        "gh pr create --base main",
    ] {
        assert!(workflow.contains(required), "{required}");
    }
    // Signing never happens here; publication is a separate workflow.
    assert!(!workflow.contains("CATALOG_SIGNING_SEED"));
    assert!(!workflow.contains("cargo catalog-sign"));
    // The proposal job only opens review PRs. Merging is confined to the admission job,
    // whose own policy test requires CI and a data-only diff.
    let proposals = workflow.split("\n  admit:\n").next().unwrap();
    assert!(!proposals.contains("gh pr merge"));
    assert!(!workflow.contains("gh pr close"));
    assert!(!proposals.contains("--delete-branch"));
    assert!(!proposals.contains("git add docs/references/catalog-quality-evidence.json"));
    assert!(
        workflow
            .find("cargo build --locked -p rigspark-cli --bin llmup-catalog-propose")
            .unwrap()
            < workflow.find("OPENAI_API_KEY:").unwrap()
    );
}

#[test]
fn fixture_batches_resume_without_overwriting_the_cursor() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("fixture.json"),
        r#"{"inventory":["new-a","new-b"],"responses":{}}"#,
    )
    .unwrap();
    fs::write(root.path().join("catalog.json"), rigspark_core::MODELS_JSON).unwrap();
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_llmup-catalog-propose"))
            .current_dir(root.path())
            .args([
                "--catalog-path",
                "catalog.json",
                "--fixture",
                "fixture.json",
                "--source-only",
                "--limit",
                "1",
                "--now",
                "2026-09-30T00:00:00Z",
            ])
            .args(extra)
            .output()
            .unwrap()
    };
    let first = run(&["--out", "first.json"]);
    assert_eq!(first.status.code(), Some(2));
    let first_bytes = fs::read(root.path().join("first.json")).unwrap();
    let report: serde_json::Value = serde_json::from_slice(&first_bytes).unwrap();
    assert_eq!(report["nextAfter"], "new-a");
    assert_eq!(
        run(&["--resume-from", "first.json", "--out", "second.json"])
            .status
            .code(),
        Some(2)
    );
    let second: serde_json::Value =
        serde_json::from_slice(&fs::read(root.path().join("second.json")).unwrap()).unwrap();
    assert_eq!(second["proposals"][0]["repository"], "new-b");
    assert_eq!(second["nextAfter"], serde_json::Value::Null);
    assert!(
        !run(&["--resume-from", "first.json", "--out", "first.json"])
            .status
            .success()
    );
    assert_eq!(
        fs::read(root.path().join("first.json")).unwrap(),
        first_bytes
    );
}

#[test]
fn fixture_extraction_failure_keeps_source_facts_and_reports_reason() {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("catalog.json"), rigspark_core::MODELS_JSON).unwrap();
    let config = json!({"model_family":"falcon","model_type":"42B","file_type":"Q4_0"}).to_string();
    let digest = format!("{:x}", Sha256::digest(config.as_bytes()));
    let manifest = json!({"config":{"digest":format!("sha256:{digest}"),"size":config.len()},"layers":[{"mediaType":"application/vnd.ollama.image.model","size":1234,"digest":format!("sha256:{}", "a".repeat(64))}]}).to_string();
    for (value, expected_code, expected_state) in [
        ("falcon", 0, "source-matched-needs-review"),
        ("fabricated", 2, "rejected"),
    ] {
        let fixture = json!({"inventory":["new-model"],"responses":{
            "https://registry.ollama.ai/v2/library/new-model/manifests/latest":manifest,
            format!("https://registry.ollama.ai/v2/library/new-model/blobs/sha256:{digest}"):config
        },"extraction":json!({"claims":[{"field":"architecture","pointer":"/model_family","valueJson":json!(value).to_string()}]}).to_string()});
        fs::write(root.path().join("fixture.json"), fixture.to_string()).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_llmup-catalog-propose"))
            .current_dir(root.path())
            .args([
                "--catalog-path",
                "catalog.json",
                "--fixture",
                "fixture.json",
                "--out",
                "report.json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(expected_code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(root.path().join("report.json")).unwrap()).unwrap();
        let proposal = &report["proposals"][0];
        assert_eq!(proposal["extractionStatus"], expected_state);
        assert_eq!(proposal["sourceClaims"].as_array().unwrap().len(), 3);
        if expected_code == 2 {
            assert_eq!(
                proposal["extractionError"],
                "claim value differs from source"
            );
            assert!(proposal["claims"].as_array().unwrap().is_empty());
            assert!(!String::from_utf8_lossy(&output.stderr).contains("fabricated"));
        }
    }
}
