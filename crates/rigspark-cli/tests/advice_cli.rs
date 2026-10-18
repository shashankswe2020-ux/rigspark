use std::process::Command;

const HARDWARE: &str = r#"{"arch":"x64","platform":"linux","totalRamBytes":68719476736,"freeRamBytes":60000000000,"freeDiskBytes":500000000000,"gpu":[{"vendor":"nvidia","vramBytes":25769803776}]}"#;

#[test]
fn catalog_refresh_is_offline_read_only_and_native() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("unused-home");
    let output = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
        .args(["catalog", "--refresh", "--all", "--hardware-json", HARDWARE])
        .env("PATH", "")
        .env("RIGSPARK_HOME", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("Refresh (dry-run):\n  added: 0\n  updated: 0\n  removed: 0\n"));
    assert!(text.contains("No catalog file was written.\n\nCatalog (Filter: all"));
    assert!(!home.exists());
}

#[test]
fn catalog_refresh_matches_frozen_legacy_text_and_never_writes_input() {
    use rigspark_core::{
        catalog::Catalog,
        enrich::{Mode, enrich, format_diff, parse_candidates},
        reports::catalog_text,
    };
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("catalog-refresh-goldens.json")).unwrap();
    for case in cases {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("catalog.json");
        let before = case["existing"].to_string();
        let existing = Catalog::parse(&before).unwrap();
        let hardware = serde_json::from_value(case["hardware"].clone()).unwrap();
        let candidates = parse_candidates(rigspark_core::REGISTRY_SNAPSHOT_JSON).unwrap();
        let format = |candidates: &[_]| {
            let result = enrich(
                &existing,
                candidates,
                Mode::Incremental,
                "2026-09-30T00:00:00Z",
                None,
            )
            .unwrap();
            format!(
                "{}{}",
                format_diff(&result.diff),
                catalog_text(&result.catalog, &hardware, case["all"] == true).unwrap()
            )
        };
        let expected = format(&candidates);
        let historical: Vec<_> = candidates
            .into_iter()
            .filter(|model| {
                !matches!(
                    model.id.as_str(),
                    "qwen3.6:35b" | "qwen3.5:4b" | "bonsai:8b" | "bitnet-b1.58:2b"
                )
            })
            .collect();
        assert_eq!(format(&historical), case["expected"].as_str().unwrap());
        std::fs::write(&path, &before).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_llmup-native"));
        command
            .args(["catalog", "--refresh", "--catalog-path"])
            .arg(&path)
            .args(["--hardware-json", &case["hardware"].to_string()])
            .env("PATH", "");
        if case["all"] == true {
            command.arg("--all");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        assert_eq!(std::fs::read_to_string(path).unwrap(), before);
    }
}

#[test]
fn rejects_irrelevant_flags_instead_of_ignoring_them() {
    for args in [
        vec!["catalog", "--context", "65536"],
        vec!["doctor", "--task", "code"],
        vec!["can-run", "llama3.1:8b", "--max-context"],
        vec!["recommend", "unwanted-model"],
        vec!["recommend", "--all"],
        vec!["recommend", "--refresh"],
        vec!["chat", "--refresh", "--message", "hello"],
        vec!["recommend", "--tui"],
        vec!["recommend", "--tui", "--json"],
        vec!["recommend", "--tui", "--no-tui"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
            .args(args)
            .args(["--hardware-json", HARDWARE])
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn catalog_filters_orders_and_reports_nonfitting_memory_without_writing() {
    use serde_json::json;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalog.json");
    let home = root.path().join("unused-home");
    let models: Vec<_> = [
        ("zeta:8b", "2026-01-01", "8B"),
        ("alpha:8b", "2026-01-01", "8B"),
        ("giant:70b", "2025-01-01", "70B"),
    ]
    .into_iter()
    .map(|(id, release, params)| {
        json!({
            "id":id,"family":"fixture","params":params,"architecture":"dense",
            "license":"apache-2.0","openWeight":true,"contextLength":8192,
            "capabilities":["chat"],"releaseDate":release,"source":{"ollama":id},
            "quantizations":[{"name":"Q4_K_M","diskBytes":5368709120_u64,
                "minRamBytes":6442450944_u64,"minVramBytes":12884901888_u64}]
        })
    })
    .collect();
    let before =
        json!({"schemaVersion":2,"generatedAt":"2026-01-01T00:00:00.000Z","models":models})
            .to_string();
    std::fs::write(&path, &before).unwrap();
    for all in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_llmup-native"));
        command
            .args(["catalog", "--catalog-path"])
            .arg(&path)
            .args(["--hardware-json", HARDWARE])
            .env("PATH", "")
            .env("RIGSPARK_HOME", &home);
        if all {
            command.arg("--all");
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{:?}", output.stderr);
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.starts_with(if all {
            "Catalog (Filter: all, shown: 3/3)"
        } else {
            "Catalog (Filter: fits, shown: 2/3)"
        }));
        let rows: Vec<_> = text
            .lines()
            .filter(|line| {
                line.starts_with("alpha:")
                    || line.starts_with("zeta:")
                    || line.starts_with("giant:")
            })
            .collect();
        assert!(rows[0].starts_with("alpha:8b"));
        assert!(rows[1].starts_with("zeta:8b"));
        if all {
            let cells: Vec<_> = rows[2].split_whitespace().collect();
            assert_eq!(cells[0], "giant:70b");
            assert_eq!(cells[5], "vram-bound");
            assert!(cells[4].parse::<f64>().unwrap() > 24.0);
        } else {
            assert!(!text.contains("giant:70b"));
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(!home.exists());
    }
}

#[test]
fn plain_override_and_no_color_preserve_noninteractive_output() {
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_llmup-native"))
            .args(["recommend", "--hardware-json", HARDWARE])
            .args(extra)
            .env("PATH", "")
            .output()
            .unwrap()
    };
    let baseline = run(&[]);
    let explicit = run(&["--no-tui", "--no-color"]);
    assert!(
        explicit.status.success(),
        "{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    assert_eq!(explicit.stdout, baseline.stdout);
    assert!(!explicit.stdout.contains(&0x1b));
}

#[test]
fn native_commands_work_without_node_or_runtime_mutations() {
    for args in [
        vec!["recommend", "--json", "--context", "65536"],
        vec!["can-run", "llama3.1:8b", "--json"],
        vec!["catalog", "--all"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
            .args(args)
            .args(["--hardware-json", HARDWARE])
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!result.stdout.is_empty());
    }
}

#[test]
fn native_cli_rejects_unsupported_operations_and_invalid_context() {
    for args in [
        vec!["up", "test"],
        vec!["recommend", "--context", "0"],
        vec!["recommend", "--context", "65536", "--max-context"],
        vec!["can-run", "missing"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
            .args(args)
            .args(["--hardware-json", HARDWARE])
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
}

#[test]
fn doctor_reports_corrupt_catalog_without_hiding_other_checks_or_reading_perf() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("catalog.json");
    std::fs::write(&path, "invalid catalog").unwrap();
    let home = root.path().join("unused-home");
    let output = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
        .args([
            "doctor",
            "--json",
            "--hardware-json",
            HARDWARE,
            "--catalog-path",
        ])
        .arg(&path)
        .arg("--perf-path")
        .arg(root.path().join("missing-perf.json"))
        .env("PATH", "")
        .env("RIGSPARK_HOME", &home)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}; stderr: {:?}", output.stderr));
    let checks = report["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 4);
    assert!(
        checks
            .iter()
            .any(|check| check["name"] == "catalog" && check["status"] == "fail")
    );
    assert!(
        checks
            .iter()
            .any(|check| check["name"] == "hardware" && check["status"] == "ok")
    );
    assert!(!report["hardwareScore"].is_null());
    assert_eq!(report["backends"].as_array().unwrap().len(), 5);
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid catalog");
    assert!(!home.exists());
}

#[test]
fn doctor_reports_corrupt_state_without_mutation_or_lock_creation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.json");
    std::fs::write(&path, "invalid state").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
        .args(["doctor", "--json", "--hardware-json", HARDWARE])
        .env("PATH", "")
        .env("RIGSPARK_HOME", root.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stderr.is_empty());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let checks = report["checks"].as_array().unwrap();
    assert!(
        checks
            .iter()
            .any(|check| check["name"] == "state" && check["status"] == "fail")
    );
    assert!(
        checks
            .iter()
            .any(|check| check["name"] == "catalog" && check["status"] == "ok")
    );
    assert!(
        checks
            .iter()
            .any(|check| check["name"] == "hardware" && check["status"] == "ok")
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid state");
    assert!(!root.path().join("lock").exists());
}

#[cfg(unix)]
#[test]
fn commands_run_within_a_windows_sized_main_thread_stack() {
    // Windows gives the main thread 1 MiB; ulimit -s caps the Unix main thread the same way.
    for args in [
        "catalog --all --no-tui",
        "recommend --no-tui",
        "doctor --no-tui --json",
        "plan llama3.1:8b",
    ] {
        let root = tempfile::tempdir().unwrap();
        let output = Command::new("/bin/sh")
            .args([
                "-c",
                &format!("ulimit -s 1024 && exec \"$0\" {args} --hardware-json \"$1\"",),
                env!("CARGO_BIN_EXE_llmup"),
                HARDWARE,
            ])
            .env("PATH", "/usr/bin:/bin")
            .env("RIGSPARK_HOME", root.path().join("home"))
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("overflowed its stack"), "{args}: {stderr}");
        assert!(
            output.status.code().is_some(),
            "{args}: {:?}",
            output.status
        );
    }
}
