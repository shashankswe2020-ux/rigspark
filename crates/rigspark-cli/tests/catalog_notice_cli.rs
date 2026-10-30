use std::{fs, process::Command};

#[test]
fn native_report_producers_feed_native_workflow_decisions_without_node() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("crates/rigspark-core/data")).unwrap();
    fs::write(
        root.path().join("crates/rigspark-core/data/models.json"),
        include_str!("../../rigspark-core/data/models.json"),
    )
    .unwrap();
    fs::write(
        root.path().join("inventory.go"),
        "var libraryModels = []string{\n \"qwen3\",\n}\n",
    )
    .unwrap();
    let fresh = Command::new(env!("CARGO_BIN_EXE_llmup-catalog-freshness"))
        // Far past any catalog stamp: admission rewrites `generatedAt` to the run date
        // before this suite runs, so a near date would make the catalog look fresh.
        .args(["--now", "2099-01-01T00:00:00Z"])
        .current_dir(root.path())
        .env("PATH", "")
        .env_remove("GITHUB_STEP_SUMMARY")
        .output()
        .unwrap();
    assert!(
        fresh.status.success(),
        "{}",
        String::from_utf8_lossy(&fresh.stderr)
    );
    let coverage = Command::new(env!("CARGO_BIN_EXE_llmup-catalog-coverage"))
        .args(["--inventory-path", "inventory.go"])
        .current_dir(root.path())
        .env("PATH", "")
        .env_remove("GITHUB_STEP_SUMMARY")
        .output()
        .unwrap();
    assert!(
        coverage.status.success(),
        "{}",
        String::from_utf8_lossy(&coverage.stderr)
    );
    for (kind, path, expected) in [
        ("needs-attention", "catalog-freshness.json", "true"),
        ("missing-count", "catalog-coverage.json", "0"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_llmup-catalog-notice"))
            .args([kind, "--input", path])
            .current_dir(root.path())
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
    }
    assert_eq!(
        fs::read_to_string(root.path().join("crates/rigspark-core/data/models.json")).unwrap(),
        include_str!("../../rigspark-core/data/models.json")
    );
}

#[test]
fn notice_reads_native_reports_offline_without_modifying_them() {
    let root = tempfile::tempdir().unwrap();
    let report = rigspark_core::freshness::evaluate(
        "2026-09-01T00:00:00Z",
        &Default::default(),
        "2026-09-19T00:00:00Z",
        7,
    )
    .unwrap();
    let bytes = serde_json::to_vec(&report).unwrap();
    let path = root.path().join("report.json");
    fs::write(&path, &bytes).unwrap();
    for kind in ["needs-attention", "freshness-issue", "refresh-pr"] {
        let output = Command::new(env!("CARGO_BIN_EXE_llmup-catalog-notice"))
            .args([kind, "--input"])
            .arg(&path)
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            rigspark_core::catalog_notice::render(kind, std::str::from_utf8(&bytes).unwrap())
                .unwrap()
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn invalid_or_oversized_report_never_prints_a_partial_notice() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("report.json");
    for content in ["{}".to_string(), "x".repeat(1024 * 1024 + 1)] {
        fs::write(&path, &content).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_llmup-catalog-notice"))
            .args(["refresh-pr", "--input"])
            .arg(&path)
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
    }
}

#[cfg(unix)]
#[test]
fn symlink_report_is_rejected() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("report.json");
    fs::write(&path, "{}").unwrap();
    let link = root.path().join("link.json");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_llmup-catalog-notice"))
        .args(["refresh-pr", "--input"])
        .arg(link)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}
