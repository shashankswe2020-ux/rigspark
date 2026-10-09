use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

const HARDWARE: &str = r#"{"arch":"x64","platform":"linux","totalRamBytes":68719476736,"freeRamBytes":60000000000,"freeDiskBytes":500000000000,"gpu":[{"vendor":"nvidia","vramBytes":25769803776}]}"#;

fn invoke(args: &[&str]) -> Output {
    invoke_with_input(args, None)
}

fn invoke_with_input(args: &[&str], input: Option<&str>) -> Output {
    invoke_fixture(args, input, false)
}

fn invoke_fixture(args: &[&str], input: Option<&str>, blocked_home: bool) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let home = if blocked_home {
        let blocker = directory.path().join("not-a-directory");
        std::fs::write(&blocker, "unchanged").unwrap();
        blocker.join("unused")
    } else {
        directory.path().join("unused")
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
        .args(args)
        .env("RIGSPARK_HOME", &home)
        .env("PATH", "")
        .env("TERM", "dumb")
        .env_remove("RIGSPARK_TUI")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if let Some(input) = input {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(!home.exists(), "unexpected state creation for {args:?}");
    output
}

#[test]
fn public_help_lists_commands_and_scopes_flags() {
    let output = invoke(&["--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("rigspark"));
    assert!(!help.contains("Experimental"));
    for flag in ["--tui", "--no-tui", "--accessible", "--no-color"] {
        assert!(!help.contains(flag), "unexpected UI flag {flag}");
    }
    assert!(help.ends_with(
        "If Sparky helps you, please consider sponsoring the project:\nhttps://buymeacoffee.com/shashanksw9\n"
    ));
    for command in [
        "recommend",
        "can-run",
        "up",
        "switch",
        "down",
        "ls",
        "doctor",
        "catalog",
        "chat",
        "migrate",
        "gui",
    ] {
        assert!(help.contains(command), "missing {command}");
    }
    let output = invoke(&["chat", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("--harness"));
    assert!(help.contains("--model"));
    assert!(!help.contains("--installed"));
    assert!(!help.contains("--parity"));
}

#[test]
fn irrelevant_flags_and_extra_positionals_fail_before_work() {
    for args in [
        vec!["ls", "--perf-path", "/missing"],
        vec!["chat", "--catalog-path", "/missing"],
        vec!["ls", "extra"],
        vec!["can-run", "model", "extra"],
        vec!["down", "--unknown"],
    ] {
        let output = invoke(&args);
        assert!(!output.status.success(), "accepted {args:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn help_and_version_are_offline_and_public() {
    for flag in ["--version", "-v", "-V"] {
        let output = invoke(&[flag]);
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!("rigspark {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
    for (command, flags) in [
        (
            "recommend",
            vec![
                "--task",
                "--context-percent",
                "--available-backends",
                "--fits-only",
            ],
        ),
        (
            "can-run",
            vec!["--context", "--backend", "--installed", "--port"],
        ),
        (
            "up",
            vec![
                "--port",
                "--backend",
                "--bypass",
                "--installed",
                "--context",
            ],
        ),
        ("switch", vec!["--bypass", "--installed", "--context"]),
        ("down", vec!["[MODEL]", "--yes"]),
        ("ls", vec!["--json"]),
        ("doctor", vec!["--json"]),
        ("catalog", vec!["--all", "--refresh"]),
        (
            "chat",
            vec![
                "--model",
                "--harness",
                "--agent",
                "--skill",
                "--no-memory",
                "--message",
            ],
        ),
        (
            "migrate",
            vec!["--from", "--to", "--move", "--dry-run", "--yes"],
        ),
        ("gui", vec!["--port", "--harness", "--no-open", "--json"]),
    ] {
        let output = invoke(&[command, "--help"]);
        assert!(output.status.success(), "{command}: {:?}", output.stderr);
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains(&format!("rigspark {command}")));
        for flag in flags {
            assert!(help.contains(flag), "{command} missing {flag}");
        }
        assert!(!help.contains("--hardware-json"));
        assert!(!help.contains("--parity"));
        for flag in ["--tui", "--no-tui", "--accessible", "--no-color"] {
            assert!(!help.contains(flag), "{command} exposes {flag}");
        }
    }
}

#[test]
fn lifecycle_help_distinguishes_state_inventory_and_process_ownership() {
    let output = invoke(&["ls", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("active server state"));
    assert!(help.contains("not installed-model inventory"));

    let output = invoke(&["down", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(help.contains("owned"));
    assert!(help.contains("detach"));
    assert!(help.contains("without stopping it"));
}

#[test]
fn catalog_json_is_rejected_before_datasets_and_hardware() {
    for extra in [
        vec!["--catalog-path", "/missing/catalog.json"],
        vec!["--perf-path", "/missing/perf.json"],
        vec!["--hardware-json", "invalid"],
        vec!["--help"],
        vec!["--version"],
    ] {
        let mut args = vec!["catalog", "--json"];
        args.extend(extra);
        let output = invoke(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.starts_with("catalog: "), "{stderr}");
        assert!(stderr.contains("--json is not supported"), "{stderr}");
    }
    let output = invoke(&["catalog", "--help"]);
    assert!(output.status.success());
    assert!(!String::from_utf8(output.stdout).unwrap().contains("--json"));
}

#[test]
fn supplied_hardware_is_validated_before_datasets() {
    let output = invoke_fixture(
        &[
            "recommend",
            "--hardware-json",
            "{",
            "--catalog-path",
            "/missing/catalog.json",
            "--perf-path",
            "/missing/perf.json",
        ],
        None,
        true,
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.starts_with("recommend: "), "{stderr}");
    assert!(stderr.contains("EOF while parsing"), "{stderr}");
    assert!(!stderr.contains("os error"), "{stderr}");
}

#[test]
fn command_flag_matrix_rejects_irrelevant_options() {
    for args in [
        vec!["recommend", "--yes"],
        vec!["can-run", "model", "--task", "code"],
        vec!["up", "model", "--max-context"],
        vec!["switch", "model", "--fits-only"],
        vec!["down", "--bypass"],
        vec!["ls", "--installed"],
        vec!["doctor", "--port", "1234"],
        vec!["catalog", "--context", "1234"],
        vec!["chat", "--from", "source"],
        vec!["migrate", "--harness", "local"],
        vec!["gui", "--no-memory"],
        vec!["gui", "extra"],
        vec!["recommend", "--no-open"],
        vec!["provider"],
        vec!["agent"],
    ] {
        let output = invoke(&args);
        assert!(!output.status.success(), "accepted {args:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn cross_command_flags_reject_before_unavailable_inputs() {
    let commands = [
        "recommend",
        "can-run",
        "catalog",
        "doctor",
        "ls",
        "up",
        "switch",
        "down",
        "chat",
        "migrate",
        "gui",
    ];
    for (flag, value, allowed) in [
        ("--task", Some("code"), "recommend"),
        (
            "--context",
            Some("8192"),
            "recommend can-run up switch migrate",
        ),
        ("--max-context", None, "recommend"),
        ("--context-percent", Some("25"), "recommend"),
        ("--backend", Some("ollama"), "recommend can-run up switch"),
        ("--available-backends", None, "recommend"),
        ("--installed", None, "recommend can-run up switch"),
        ("--port", Some("1234"), "recommend can-run up switch gui"),
        ("--fits-only", None, "recommend"),
        ("--bypass", None, "up switch"),
        ("--all", None, "catalog"),
        ("--refresh", None, "catalog"),
        ("--yes", None, "down migrate"),
        ("--model", Some("model"), "chat"),
        ("--harness", Some("local"), "chat gui"),
        ("--message", Some("hello"), "chat"),
        ("--agent", Some("assistant"), "chat"),
        ("--skill", Some("skill"), "chat"),
        ("--no-memory", None, "chat"),
        ("--from", Some("source"), "migrate"),
        ("--to", Some("target"), "migrate"),
        ("--move", None, "migrate"),
        ("--dry-run", None, "migrate"),
        ("--no-open", None, "gui"),
        (
            "--json",
            None,
            "recommend can-run doctor ls up switch down chat migrate gui",
        ),
        (
            "--catalog-path",
            Some("/missing/catalog.json"),
            "recommend can-run catalog doctor up switch down migrate",
        ),
        (
            "--perf-path",
            Some("/missing/perf.json"),
            "recommend can-run catalog doctor up switch down",
        ),
        (
            "--hardware-json",
            Some("invalid"),
            "recommend can-run catalog doctor up switch",
        ),
        (
            "--tui",
            None,
            "recommend can-run catalog doctor ls up switch down chat migrate",
        ),
        (
            "--no-tui",
            None,
            "recommend can-run catalog doctor ls up switch down chat migrate",
        ),
        (
            "--accessible",
            None,
            "recommend can-run catalog doctor ls up switch down chat migrate",
        ),
        (
            "--no-color",
            None,
            "recommend can-run catalog doctor ls up switch down chat migrate",
        ),
    ] {
        for command in commands {
            if allowed.split_whitespace().any(|name| name == command) {
                continue;
            }
            let mut args = vec![command, flag];
            args.extend(value);
            if [
                "recommend",
                "can-run",
                "catalog",
                "doctor",
                "up",
                "switch",
                "down",
                "migrate",
            ]
            .contains(&command)
            {
                args.extend(["--catalog-path", "/missing/catalog.json"]);
            }
            let output = invoke_fixture(&args, None, true);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{args:?}: {:?}",
                output.stderr
            );
            assert!(output.stdout.is_empty(), "{args:?}");
            let stderr = String::from_utf8(output.stderr).unwrap();
            assert!(stderr.starts_with(&format!("{command}: ")), "{stderr}");
            assert_eq!(
                stderr.matches(&format!("{command}: ")).count(),
                1,
                "{stderr}"
            );
            assert!(
                stderr.contains(&format!("{flag} is not supported")),
                "{args:?}: {stderr}"
            );
        }
    }
}

#[test]
fn informational_flags_short_circuit_semantics_but_not_parser_errors() {
    for command in [
        "recommend",
        "can-run",
        "catalog",
        "doctor",
        "ls",
        "up",
        "switch",
        "down",
        "chat",
        "migrate",
        "gui",
    ] {
        for flag in ["--help", "-h", "--version", "-v", "-V"] {
            let mut args = vec![command, flag];
            if [
                "recommend",
                "can-run",
                "catalog",
                "doctor",
                "up",
                "switch",
                "down",
                "migrate",
            ]
            .contains(&command)
            {
                args.extend(["--catalog-path", "/missing/catalog.json"]);
            }
            if ["recommend", "can-run", "up", "switch", "migrate"].contains(&command) {
                args.extend(["--context", "0"]);
            }
            let output = invoke_fixture(&args, None, true);
            assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
            assert!(output.stderr.is_empty());
            let text = String::from_utf8(output.stdout).unwrap();
            if ["--help", "-h"].contains(&flag) {
                assert!(text.contains(&format!("rigspark {command}")), "{text}");
                assert_eq!(text.contains("--json"), command != "catalog", "{text}");
            } else {
                assert_eq!(text, format!("rigspark {}\n", env!("CARGO_PKG_VERSION")));
            }
        }
    }
    for informational in ["--help", "-h", "--version", "-v", "-V"] {
        for mut args in [
            vec!["unknown"],
            vec!["up", "--unknown"],
            vec!["up", "--task", "code"],
            vec!["up", "--context", "nonnumeric"],
            vec!["up", "--port", "0"],
            vec!["up", "--context"],
        ] {
            args.push(informational);
            let output = invoke_fixture(&args, None, true);
            assert_eq!(output.status.code(), Some(1), "{args:?}");
            assert!(output.stdout.is_empty(), "{args:?}");
            assert!(!output.stderr.is_empty(), "{args:?}");
        }
    }
}

#[test]
fn invalid_values_and_dependencies_fail_before_io() {
    for (args, error) in [
        (vec!["recommend", "--context", "invalid"], "context"),
        (vec!["recommend", "--context-percent", "256"], "context"),
        (vec!["up", "model", "--port", "invalid"], "port"),
        (vec!["recommend", "--context", "0"], "context"),
        (vec!["recommend", "--context", "1.5"], "context"),
        (vec!["recommend", "--context", "NaN"], "context"),
        (vec!["recommend", "--context", "10000001"], "context"),
        (vec!["recommend", "--context-percent", "26"], "context"),
        (
            vec!["recommend", "--context", "512", "--max-context"],
            "mutually exclusive",
        ),
        (vec!["recommend", "--task", "bogus"], "task"),
        (vec!["recommend", "--backend", "bogus"], "backend"),
        (vec!["recommend", "--fits-only"], "--installed"),
        (vec!["recommend", "--port", "1234"], "--installed"),
        (
            vec!["recommend", "--installed", "--task", "code"],
            "installed",
        ),
        (vec!["can-run"], "model is required"),
        (vec!["up"], "model is required"),
        (vec!["switch"], "model is required"),
        (
            vec!["up", "model", "--installed", "--catalog-path", "/missing"],
            "--bypass",
        ),
        (
            vec![
                "up",
                "bad tag",
                "--installed",
                "--bypass",
                "--catalog-path",
                "/missing",
            ],
            "invalid runtime model id",
        ),
        (
            vec![
                "can-run",
                "bad tag",
                "--installed",
                "--catalog-path",
                "/missing",
            ],
            "invalid runtime model id",
        ),
        (vec!["up", "model", "--port", "0"], "port"),
        (vec!["gui", "--port", "65536"], "port"),
        (vec!["migrate", "--from", "source"], "--to"),
        (
            vec!["migrate", "--from", "source", "--to", "target", "--yes"],
            "--move",
        ),
        (
            vec!["migrate", "--from", "source", "--to", "target", "--move"],
            "--yes",
        ),
        (
            vec![
                "migrate",
                "--from",
                "source",
                "--to",
                "target",
                "--context",
                "0",
                "--catalog-path",
                "/missing",
            ],
            "context",
        ),
        (
            vec!["chat", "--harness", "bogus", "--message", "hello"],
            "harness",
        ),
        (
            vec!["chat", "--harness", "openai", "--message", "hello"],
            "--model",
        ),
        (vec!["chat", "--model", " "], "model"),
    ] {
        let output = invoke(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.starts_with(&format!("{}: ", args[0])), "{stderr}");
        assert!(
            stderr.contains(error),
            "{args:?}: expected {error}, got {stderr}"
        );
    }
}

#[test]
fn migrate_yes_dependency_precedes_missing_references() {
    for args in [
        vec!["migrate", "--yes"],
        vec!["migrate", "--from", "source", "--yes"],
    ] {
        let output = invoke_fixture(&args, None, true);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            "migrate: --yes requires --move\n",
            "{args:?}"
        );
    }
}

#[test]
fn chat_library_selection_fails_before_state_access() {
    for (args, error) in [
        (
            vec!["chat", "--message", "hello", "--agent", "../private"],
            "memory: invalid library id",
        ),
        (
            vec!["chat", "--message", "hello", "--skill", "bad/skill"],
            "memory: invalid library id",
        ),
    ] {
        let output = invoke_fixture(&args, None, true);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!("chat: {error}\n"),
            "{args:?}"
        );
    }

    let mut args = vec!["chat", "--message", "hello"];
    for _ in 0..51 {
        args.extend(["--skill", "valid-skill"]);
    }
    let output = invoke_fixture(&args, None, true);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "chat: memory: skill selection exceeds limit\n"
    );
}

#[test]
fn parse_error_prefix_respects_positionals_and_sanitizes_command_text() {
    for (args, command) in [
        (vec!["--context", "invalid"], "recommend"),
        (vec!["--port", "invalid", "up", "model"], "up"),
        (vec!["--harness", "down", "gui", "--port", "invalid"], "gui"),
        (vec!["--context-percent=invalid", "recommend"], "recommend"),
        (vec!["bad\u{1b}[31mcommand"], "badcommand"),
    ] {
        let output = invoke(&args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.starts_with(&format!("{command}: ")), "{stderr:?}");
        assert!(!stderr.contains('\u{1b}'), "{stderr:?}");
    }
}

#[test]
fn hidden_ui_flags_are_still_recognized() {
    for flag in ["--tui", "--no-tui", "--accessible", "--no-color"] {
        let output = invoke(&["chat", "--help", flag]);
        assert!(output.status.success(), "{flag}: {:?}", output.stderr);
        assert!(output.stderr.is_empty());
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("--harness")
        );
    }
}

#[test]
fn default_recommend_and_native_context_modes_remain_supported() {
    let default = invoke(&["--json", "--hardware-json", HARDWARE]);
    let named = invoke(&["recommend", "--json", "--hardware-json", HARDWARE]);
    assert!(default.status.success());
    assert!(named.status.success());
    assert_eq!(default.stdout, named.stdout);
    for mode in [
        vec!["--context=8192"],
        vec!["--context-percent", "25"],
        vec!["--context-percent", "50"],
        vec!["--context-percent", "75"],
        vec!["--context-percent", "100"],
        vec!["--max-context"],
    ] {
        let mut args = vec!["recommend", "--json", "--hardware-json", HARDWARE];
        args.extend(mode);
        let output = invoke(&args);
        assert!(output.status.success(), "{args:?}: {:?}", output.stderr);
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    }
}

#[test]
fn hidden_parity_path_still_accepts_stdin() {
    let output = invoke_with_input(&["--parity"], Some("[]"));
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(output.stdout, b"[]\n");
}

#[test]
fn down_yes_and_empty_state_remain_offline() {
    let output = invoke(&["down", "--yes", "--json"]);
    assert!(output.status.success(), "{:?}", output.stderr);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["type"],
        "no-active"
    );
}
