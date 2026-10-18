use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const HARDWARE: &str = r#"{"arch":"x64","platform":"linux","totalRamBytes":68719476736,"freeRamBytes":64424509440,"freeDiskBytes":536870912000,"gpu":[{"vendor":"nvidia","vramBytes":25769803776}]}"#;
const COMMANDS: [&str; 13] = [
    "recommend",
    "can-run",
    "plan",
    "up",
    "chat",
    "gui",
    "down",
    "switch",
    "migrate",
    "ls",
    "catalog",
    "doctor",
    "generate",
];

fn invoke(args: &[&str]) -> Output {
    let directory = tempfile::tempdir().unwrap();
    invoke_at(args, directory.path())
}

fn invoke_at(args: &[&str], directory: &std::path::Path) -> Output {
    let home = directory.join("home");
    let output = invoke_with_home(args, &home);
    assert!(!home.exists(), "unexpected state mutation: {args:?}");
    output
}

fn invoke_with_home(args: &[&str], home: &std::path::Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_llmup-native"))
        .args(args)
        .env("RIGSPARK_HOME", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("PATH", "")
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .env_remove("RIGSPARK_TUI")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert!(!output.stdout.contains(&0x1b));
}

#[test]
fn advisory_catalog_activation_fails_without_creating_runtime_state() {
    let directory = tempfile::tempdir().unwrap();
    let mut catalog: Value = serde_json::from_str(rigspark_core::MODELS_JSON).unwrap();
    let model = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model.get("provenance").is_none())
        .unwrap()
        .clone();
    catalog["schemaVersion"] = json!(4);
    catalog["models"] = json!([model]);
    catalog["models"][0]["id"] = json!("advisory:example");
    catalog["models"][0]["availability"] =
        json!({"status":"advisory-only","reason":"backend-format-unsupported"});
    catalog["models"][0]["source"] = json!({"weights":{
        "repo":"publisher/model","revision":"a".repeat(40),
        "files":[{"file":"model.safetensors","bytes":1000,"sha256":"b".repeat(64)}]
    }});
    catalog["models"][0]["quantizations"] =
        json!([{"name":"BF16","diskBytes":1000,"minRamBytes":1150,"minVramBytes":1150}]);
    let path = directory.path().join("catalog.json");
    std::fs::write(&path, catalog.to_string()).unwrap();
    for command in ["up", "switch"] {
        for extra in [vec![], vec!["--bypass"], vec!["--bypass", "--installed"]] {
            let mut args = vec![
                command,
                "advisory:example",
                "--catalog-path",
                path.to_str().unwrap(),
            ];
            args.extend(extra);
            let output = invoke_at(&args, directory.path());
            assert!(!output.status.success());
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("not yet installable"),
                "{command}: {stderr}"
            );
        }
    }
}

fn assert_json(actual: &Value, expected: &Value) {
    match (actual, expected) {
        (Value::Number(actual), Value::Number(expected)) => {
            let actual = actual.as_f64().unwrap();
            let expected = expected.as_f64().unwrap();
            assert!(
                (actual - expected).abs() <= 1e-12 * expected.abs().max(1.0),
                "{actual} != {expected}"
            );
        }
        (Value::Object(actual), Value::Object(expected)) => {
            assert_eq!(actual.len(), expected.len());
            for (key, expected) in expected {
                assert_json(actual.get(key).unwrap(), expected);
            }
        }
        (Value::Array(actual), Value::Array(expected)) => {
            assert_eq!(actual.len(), expected.len());
            for (actual, expected) in actual.iter().zip(expected) {
                assert_json(actual, expected);
            }
        }
        _ => assert_eq!(actual, expected),
    }
}

#[test]
fn executable_consumes_argv_and_prints_public_version_once() {
    for flag in ["--version", "-v", "-V"] {
        let output = invoke(&[flag]);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            output.stdout,
            format!("rigspark {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
        );
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn help_has_unique_described_commands_and_command_scoped_options() {
    let output = invoke(&["--help"]);
    assert_success(&output);
    let help = String::from_utf8(output.stdout).unwrap();
    let commands = help
        .split("Commands:\n")
        .nth(1)
        .unwrap()
        .split("\nOptions:")
        .next()
        .unwrap();
    let mut actual: Vec<_> = commands
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    let mut expected = COMMANDS.to_vec();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    for name in COMMANDS {
        let rows: Vec<_> = commands
            .lines()
            .filter(|line| line.split_whitespace().next() == Some(name))
            .collect();
        assert_eq!(rows.len(), 1, "{name}: {commands}");
        assert!(rows[0].split_whitespace().count() > 1);
        if ["recommend", "up", "chat", "catalog"].contains(&name) {
            assert!(include_str!("../../../README.md").contains(name));
        }
        let output = invoke(&[name, "--help"]);
        assert_success(&output);
        let command_help = String::from_utf8(output.stdout).unwrap();
        assert!(command_help.contains(&format!("rigspark {name}")));
        assert!(!command_help.contains("--parity"));
        assert!(!command_help.contains("--hardware-json"));
        if name == "down" {
            assert!(command_help.contains("detach and forget"));
            assert!(command_help.contains("without stopping it"));
        }
    }
    let output = invoke(&["catalog", "--help"]);
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--all"));
    assert!(help.contains("--refresh"));
    assert!(!help.contains("--context"));
}

#[test]
fn native_fixture_manifest_matches_public_commands_and_json_support() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("fixtures/noninteractive/manifest.json")).unwrap(),
    )
    .unwrap();
    let output = invoke(&["--help"]);
    assert_success(&output);
    let help = String::from_utf8(output.stdout).unwrap();
    let mut commands: Vec<_> = help
        .split("Commands:\n")
        .nth(1)
        .unwrap()
        .split("\nOptions:")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    commands.sort_unstable();
    let mut json_commands = Vec::new();
    for command in &commands {
        let output = invoke(&[command, "--help"]);
        assert_success(&output);
        if String::from_utf8(output.stdout).unwrap().contains("--json") {
            json_commands.push(*command);
        }
    }
    assert_eq!(commands.len(), 13);
    assert_eq!(json_commands.len(), 12);
    assert!(!json_commands.contains(&"catalog"));
    for (mode, expected) in [("plain", commands), ("json", json_commands)] {
        let entries = manifest[mode].as_object().unwrap();
        let mut actual: Vec<_> = entries.keys().map(String::as_str).collect();
        actual.sort_unstable();
        assert_eq!(actual, expected, "{mode}");
        for (command, path) in entries {
            let fixture = std::fs::read_to_string(root.join(path.as_str().unwrap())).unwrap();
            assert!(!fixture.trim().is_empty(), "{mode}: {command}");
            if mode == "json" {
                serde_json::from_str::<Value>(&fixture).unwrap();
            }
        }
    }
}

#[test]
fn plan_matches_reviewed_goldens_without_detection_or_mutation() {
    let plain = invoke(&["plan", "llama3.1:8b", "--hardware-json", HARDWARE]);
    assert_success(&plain);
    assert_eq!(
        String::from_utf8(plain.stdout).unwrap(),
        include_str!("../../../tests/fixtures/noninteractive/plan-plain.txt")
    );
    let json = invoke(&["plan", "llama3.1:8b", "--json", "--hardware-json", HARDWARE]);
    assert_success(&json);
    assert_json(
        &serde_json::from_slice(&json.stdout).unwrap(),
        &serde_json::from_str(include_str!(
            "../../../tests/fixtures/noninteractive/plan-json.json"
        ))
        .unwrap(),
    );
}

#[test]
fn global_help_matches_reviewed_native_golden() {
    let output = invoke(&["--help"]);
    assert_success(&output);
    let expected: String = serde_json::from_str(
        &std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/noninteractive/help-plain.encoded.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
}

#[test]
fn empty_state_json_matches_native_goldens_without_runtime_or_mutation() {
    for (command, fixture) in [
        (
            "ls",
            include_str!("../fixtures/noninteractive/ls-json.json"),
        ),
        (
            "down",
            include_str!("../fixtures/noninteractive/down-json.json"),
        ),
    ] {
        let output = invoke(&[command, "--json"]);
        assert_success(&output);
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            serde_json::from_str::<Value>(fixture).unwrap(),
        );
    }
}

#[test]
fn invalid_values_fail_without_stdout_or_state_access() {
    for args in [
        vec!["recommend", "--context", "0"],
        vec!["recommend", "--context", "abc"],
        vec!["recommend", "--context", "1.5"],
        vec!["recommend", "--context", "8192", "--max-context"],
        vec!["recommend", "--backend", "bogus"],
        vec!["migrate", "--from", "source", "--to", "target", "--yes"],
    ] {
        let output = invoke(&args);
        assert!(!output.status.success(), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert!(!output.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn invalid_advice_fails_exactly_before_loading_any_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("must-not-read.json");
    for (flags, diagnostic) in [
        (vec!["--context", "0"], "invalid context"),
        (
            vec!["--context", "abc"],
            "error: invalid value 'abc' for '--context <CONTEXT>': invalid float literal",
        ),
        (vec!["--context", "1.5"], "invalid context"),
        (
            vec!["--context", "8192", "--max-context"],
            "context, contextPercent, and maxContext are mutually exclusive",
        ),
        (vec!["--backend", "bogus"], "invalid backend"),
    ] {
        for command in [vec![], vec!["recommend"]] {
            let mut args = command;
            args.extend(flags.iter().copied());
            args.extend([
                "--catalog-path",
                missing.to_str().unwrap(),
                "--hardware-json",
                "invalid",
            ]);
            let output = invoke_at(&args, directory.path());
            assert_eq!(output.status.code(), Some(1), "{args:?}");
            assert!(output.stdout.is_empty(), "{args:?}");
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                format!("recommend: {diagnostic}\n"),
                "{args:?}"
            );
        }
    }
}

#[test]
fn recommendation_reports_requested_context_8192() {
    let output = invoke(&[
        "recommend",
        "--context",
        "8192",
        "--json",
        "--hardware-json",
        HARDWARE,
    ]);
    assert_success(&output);
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let ranked = report["ranked"].as_array().unwrap();
    assert!(!ranked.is_empty());
    assert!(
        ranked
            .iter()
            .all(|entry| entry["context"].as_f64() == Some(8192.0))
    );
}

#[test]
fn invalid_up_ports_preserve_exact_diagnostics_before_state_access() {
    for port in ["0", "65536"] {
        let output = invoke(&["up", "llama3.1:8b", "--port", port]);
        assert_eq!(output.status.code(), Some(1), "{port}");
        assert!(output.stdout.is_empty(), "{port}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!("up: invalid --port {port} (expected an integer in 1..65535)\n")
        );
    }
}

#[test]
fn boundary_ports_are_accepted_before_catalog_loading() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.json");
    for port in ["1", "65535"] {
        let output = invoke_at(
            &[
                "up",
                "llama3.1:8b",
                "--port",
                port,
                "--catalog-path",
                missing.to_str().unwrap(),
            ],
            directory.path(),
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.starts_with("up: "), "{error}");
        assert!(!error.contains("port"), "{error}");
        assert!(error.contains("os error"), "{error}");
    }
}

#[test]
fn command_failures_are_one_prefixed_stderr_line_and_exit_one() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.json");
    for (name, args) in [
        ("recommend", vec![]),
        ("recommend", vec!["recommend"]),
        ("can-run", vec!["can-run", "llama3.1:8b"]),
        ("up", vec!["up", "llama3.1:8b"]),
        ("switch", vec!["switch", "qwen2.5:7b"]),
        ("down", vec!["down"]),
        ("catalog", vec!["catalog"]),
        ("doctor", vec!["doctor"]),
        (
            "migrate",
            vec!["migrate", "--from", "source", "--to", "target"],
        ),
    ] {
        let mut args = args;
        args.extend(["--catalog-path", missing.to_str().unwrap()]);
        if name == "doctor" {
            args.extend(["--hardware-json", HARDWARE]);
        }
        let output = invoke_at(&args, directory.path());
        assert_eq!(output.status.code(), Some(1), "{name}");
        if name == "doctor" {
            assert!(output.stderr.is_empty());
            let report = String::from_utf8(output.stdout).unwrap();
            assert!(report.contains("catalog is unusable:"), "{report}");
            assert!(report.contains("hardware"), "{report}");
            assert!(report.contains("Backends"), "{report}");
            assert!(report.contains("Problems found"), "{report}");
            continue;
        }
        assert!(output.stdout.is_empty(), "{name}");
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.starts_with(&format!("{name}: ")), "{error}");
        assert_eq!(error.lines().count(), 1, "{error}");
        assert!(!error.contains('\u{1b}'));
    }
}

#[test]
fn default_dispatch_and_plain_overrides_preserve_output_without_node() {
    let default = invoke(&["--hardware-json", HARDWARE]);
    assert_success(&default);
    for extra in [
        vec![],
        vec!["--no-tui"],
        vec!["--no-color"],
        vec!["--no-tui", "--no-color"],
    ] {
        let mut args = vec!["recommend", "--hardware-json", HARDWARE];
        args.extend(extra);
        let output = invoke(&args);
        assert_success(&output);
        assert_eq!(output.stdout, default.stdout);
    }
    let default = invoke(&["--json", "--hardware-json", HARDWARE]);
    let named = invoke(&["recommend", "--json", "--hardware-json", HARDWARE]);
    assert_success(&default);
    assert_success(&named);
    assert_eq!(default.stdout, named.stdout);
    serde_json::from_slice::<Value>(&default.stdout).unwrap();
}

#[test]
fn explicit_interactive_modes_reject_non_tty_without_partial_output() {
    for args in [
        vec!["recommend"],
        vec!["can-run", "llama3.1:8b"],
        vec!["doctor"],
        vec!["catalog"],
        vec!["ls"],
    ] {
        for mode in ["--tui", "--accessible"] {
            let mut selected = args.clone();
            selected.push(mode);
            let output = invoke(&selected);
            assert_eq!(output.status.code(), Some(1), "{selected:?}");
            assert!(output.stdout.is_empty());
            let error = String::from_utf8(output.stderr).unwrap();
            assert!(error.contains("incompatible"), "{selected:?}: {error}");
        }
    }
    let missing = invoke(&["can-run"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
    assert_eq!(missing.stderr, b"can-run: model is required\n");
}

fn can_run_catalog() -> Value {
    json!({
        "schemaVersion": 2,
        "generatedAt": "2026-01-01T00:00:00.000Z",
        "models": [{
            "id": "llama3.1:8b", "family": "llama3.1", "params": "7B",
            "architecture": "dense", "license": "apache-2.0", "openWeight": true,
            "contextLength": 4096, "capabilities": ["chat"], "releaseDate": "2024-01-01",
            "source": {"ollama": "llama3.1:8b"},
            "quantizations": [{"name": "Q4_K_M", "diskBytes": 4_400_000_000_u64,
                "minRamBytes": 4_400_000_000_u64, "minVramBytes": 4_400_000_000_u64}]
        }]
    })
}

#[test]
fn classic_can_run_and_empty_ls_match_retained_typescript_goldens() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    std::fs::write(&path, can_run_catalog().to_string()).unwrap();
    let args = [
        "can-run",
        "llama3.1",
        "--catalog-path",
        path.to_str().unwrap(),
        "--hardware-json",
        HARDWARE,
    ];
    let output = invoke_at(&args, directory.path());
    assert_success(&output);
    assert_eq!(
        output.stdout,
        include_bytes!("../../../tests/fixtures/noninteractive/can-run-plain.txt")
    );
    let mut args = args.to_vec();
    args.push("--json");
    let output = invoke_at(&args, directory.path());
    assert_success(&output);
    let actual: Value = serde_json::from_slice(&output.stdout).unwrap();
    let expected: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/noninteractive/can-run-json.json"
    ))
    .unwrap();
    assert_json(&actual, &expected);
    let output = invoke(&["ls"]);
    assert_success(&output);
    assert_eq!(
        output.stdout,
        include_bytes!("../../../tests/fixtures/noninteractive/ls-plain.txt")
    );
}

#[test]
fn can_run_yes_slow_and_no_verdicts_control_exit_not_json_mode() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    std::fs::write(&path, can_run_catalog().to_string()).unwrap();
    let mut slow: Value = serde_json::from_str(HARDWARE).unwrap();
    slow["gpu"] = json!([]);
    let mut insufficient = slow.clone();
    insufficient["totalRamBytes"] = json!(1_073_741_824);
    insufficient["freeRamBytes"] = json!(1_073_741_824);
    for (hardware, verdict, exit) in [
        (HARDWARE.to_owned(), "yes", 0),
        (slow.to_string(), "slow", 0),
        (insufficient.to_string(), "no", 1),
    ] {
        for json_mode in [false, true] {
            let mut args = vec![
                "can-run",
                "llama3.1",
                "--catalog-path",
                path.to_str().unwrap(),
                "--hardware-json",
                &hardware,
            ];
            if json_mode {
                args.push("--json");
            }
            let output = invoke_at(&args, directory.path());
            assert_eq!(
                output.status.code(),
                Some(exit),
                "{verdict}: {:?}",
                output.stderr
            );
            assert!(output.stderr.is_empty());
            assert!(!output.stdout.contains(&0x1b));
            if json_mode {
                let report: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["verdict"], verdict);
            }
        }
    }
}

#[test]
fn public_advice_options_match_frozen_typescript_reports() {
    let oracle: Value = serde_json::from_str(include_str!("advice-parity-oracle.json")).unwrap();
    let values = oracle["values"].as_array().unwrap();
    for case in oracle["cases"].as_array().unwrap().iter().take(13) {
        let hardware = case["input"]["hardware"].to_string();
        let options = case["input"]["options"].as_object().unwrap();
        let mut flags = Vec::new();
        for (name, value) in options {
            flags.push(
                match name.as_str() {
                    "context" => "--context",
                    "maxContext" => "--max-context",
                    "contextPercent" => "--context-percent",
                    "task" => "--task",
                    "backend" => "--backend",
                    _ => panic!("unmapped oracle option: {name}"),
                }
                .to_owned(),
            );
            if name != "maxContext" {
                flags.push(
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                );
            }
        }
        let mut args = vec![
            "recommend",
            "--hardware-json",
            &hardware,
            "--catalog-path",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../rigspark-core/fixtures/advice-catalog-v2.0.0.json"
            ),
        ];
        args.extend(flags.iter().map(String::as_str));
        let output = invoke(&args);
        assert_success(&output);
        let expected = &values[case["expected"]["text"].as_u64().unwrap() as usize];
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("{}\n", expected.as_str().unwrap()),
            "{}",
            case["label"]
        );
        args.push("--json");
        let output = invoke(&args);
        assert_success(&output);
        let expected = &values[case["expected"]["recommendation"].as_u64().unwrap() as usize];
        assert_json(
            &serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            expected,
        );
        if options
            .keys()
            .all(|key| ["context", "backend"].contains(&key.as_str()))
        {
            let queries = case["input"]["queries"].as_array().unwrap();
            let index = queries
                .iter()
                .position(|query| query == "llama3.1:8b")
                .unwrap();
            let reference = &case["expected"]["checks"][index]["report"];
            let expected = &values[reference.as_u64().unwrap() as usize];
            let mut args = vec![
                "can-run",
                "llama3.1:8b",
                "--json",
                "--hardware-json",
                &hardware,
            ];
            args.extend(flags.iter().map(String::as_str));
            let output = invoke(&args);
            assert_eq!(
                output.status.code(),
                Some(i32::from(expected["verdict"] == "no"))
            );
            assert!(output.stderr.is_empty());
            assert_json(
                &serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                expected,
            );
        }
    }
}

#[test]
fn can_run_mlx_backend_eligibility_is_offline_and_unsourced_throughput_stays_unknown() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    let mut catalog = can_run_catalog();
    catalog["models"][0]["quantizations"] = json!([{"name":"6bit", "diskBytes":1300,
        "minRamBytes":1300, "minVramBytes":1300}]);
    catalog["models"][0]["source"] = json!({"mlx": {
        "repo":"mlx-community/fixture", "revision":"a".repeat(40),
        "files":[{"file":"config.json","sha256":"b".repeat(64),"bytes":100},
            {"file":"tokenizer_config.json","sha256":"c".repeat(64),"bytes":200},
            {"file":"model.safetensors","sha256":"d".repeat(64),"bytes":1000}]
    }});
    std::fs::write(&path, catalog.to_string()).unwrap();
    for apple in [false, true] {
        let mut hardware: Value = serde_json::from_str(HARDWARE).unwrap();
        if apple {
            hardware["platform"] = json!("darwin");
            hardware["arch"] = json!("arm64");
            hardware["gpu"] = json!([{"vendor":"apple","vramBytes":0}]);
        }
        let output = invoke_at(
            &[
                "can-run",
                "llama3.1",
                "--catalog-path",
                path.to_str().unwrap(),
                "--hardware-json",
                &hardware.to_string(),
                "--backend",
                "mlx",
                "--json",
            ],
            directory.path(),
        );
        assert_success(&output);
        let report: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["backends"],
            if apple {
                json!(["mlx", "lmstudio"])
            } else {
                json!([])
            }
        );
        assert_eq!(report["throughputBackend"], "mlx");
        assert_eq!(report["throughput"]["known"], false);
        assert_eq!(report["throughput"]["lowTokPerSec"].as_f64(), Some(0.0));
        assert_eq!(report["throughput"]["highTokPerSec"].as_f64(), Some(0.0));
        let plain = invoke_at(
            &[
                "can-run",
                "llama3.1",
                "--catalog-path",
                path.to_str().unwrap(),
                "--hardware-json",
                &hardware.to_string(),
                "--backend",
                "mlx",
            ],
            directory.path(),
        );
        assert_success(&plain);
        assert!(
            String::from_utf8(plain.stdout)
                .unwrap()
                .contains("Estimated throughput: unknown")
        );
    }
}

#[test]
fn local_chat_and_ls_errors_stay_on_stderr_without_mutating_home() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("not-a-directory");
    std::fs::write(&home, b"unchanged").unwrap();
    for (name, args) in [
        ("ls", vec!["ls"]),
        ("chat", vec!["chat", "--message", "hello"]),
    ] {
        let output = invoke_with_home(&args, &home);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.starts_with(&format!("{name}: ")), "{error}");
        assert_eq!(error.lines().count(), 1);
        assert_eq!(std::fs::read(&home).unwrap(), b"unchanged");
    }
}

#[test]
fn retained_goldens_cover_every_public_command() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/noninteractive");
    for command in COMMANDS {
        assert!(
            directory.join(format!("{command}-plain.txt")).is_file(),
            "{command}"
        );
    }
    for command in ["recommend", "can-run", "gui", "doctor"] {
        let fixture = std::fs::read(directory.join(format!("{command}-json.json"))).unwrap();
        serde_json::from_slice::<Value>(&fixture).unwrap();
    }
}

#[test]
fn already_active_switch_emits_one_report_without_runtime_or_state_changes() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let store = rigspark_runtime::state::StateStore::new(
        rigspark_runtime::state::Config::from_home(&home).unwrap(),
    );
    let state = rigspark_runtime::state::RuntimeState::parse(r#"{"schemaVersion":2,"active":{"backend":"ollama","modelId":"llama3.1:8b","endpoint":"http://127.0.0.1:11435","port":11435,"ownedByUs":false}}"#).unwrap();
    let guard = store.lock(std::time::Duration::from_secs(1)).unwrap();
    store.write(&guard, &state).unwrap();
    guard.release().unwrap();
    let before = std::fs::read(&store.config.state).unwrap();
    for json_mode in [false, true] {
        let mut args = vec![
            "switch",
            "llama3.1:8b",
            "--port",
            "11435",
            "--hardware-json",
            HARDWARE,
        ];
        if json_mode {
            args.push("--json");
        }
        let output = invoke_with_home(&args, &home);
        assert_success(&output);
        if json_mode {
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                serde_json::from_str::<Value>(include_str!(
                    "../fixtures/noninteractive/switch-json.json"
                ))
                .unwrap()
            );
        } else {
            assert_eq!(output.stdout, b"llama3.1:8b is already active.\n");
        }
        assert_eq!(std::fs::read(&store.config.state).unwrap(), before);
        assert!(!store.config.lock.exists());
    }
}

#[tokio::test]
async fn migration_dispatch_preserves_plain_json_and_preview_copy_semantics() {
    use rigspark_runtime::memory::{CaptureOptions, MemoryStore};
    for json_mode in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let source = MemoryStore::open(&home, "llama3.1:8b", "2026-09-20T00:00:00Z").unwrap();
        source
            .capture(
                "hello",
                "reply",
                CaptureOptions {
                    timestamp: "2026-09-20T00:00:00Z",
                    embedder: None,
                    embedding_unsupported: false,
                },
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
        let before = source.load().unwrap();
        for dry_run in [true, false] {
            let mut args = vec![
                "migrate",
                "--from",
                "llama3.1:8b",
                "--to",
                "qwen2.5:14b",
                "--context",
                "8192",
            ];
            if json_mode {
                args.push("--json");
            }
            if dry_run {
                args.push("--dry-run");
            }
            let output = invoke_with_home(&args, &home);
            assert_success(&output);
            if json_mode {
                let mut expected: Value = serde_json::from_str(include_str!(
                    "../fixtures/noninteractive/migrate-json.json"
                ))
                .unwrap();
                expected["dryRun"] = json!(dry_run);
                assert_eq!(
                    serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                    expected
                );
            } else {
                let golden =
                    include_str!("../../../tests/fixtures/noninteractive/migrate-plain.txt");
                let expected = if dry_run {
                    golden.replace("Migrated memory:", "[dry-run] Planned migration:")
                } else {
                    golden.into()
                };
                assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
            }
            assert_eq!(source.load().unwrap(), before);
            assert!(!home.join("lock").exists());
            if dry_run {
                assert!(MemoryStore::existing(&home, "qwen2.5:14b").is_err());
            } else {
                assert_eq!(
                    MemoryStore::existing(&home, "qwen2.5:14b")
                        .unwrap()
                        .load()
                        .unwrap(),
                    before
                );
            }
        }
    }
}
