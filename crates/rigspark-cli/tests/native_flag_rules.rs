//! Flag-combination rules enforced by `execute`, beyond the per-command scoping in
//! `native_args`. `--parity` bypasses that scoping, so it reaches the rules that only
//! `execute` applies. Every invocation runs with an unusable home and an empty PATH, so
//! nothing is created and no runtime is reachable.

use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

fn invoke(args: &[&str], input: Option<&[u8]>) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let blocker = directory.path().join("not-a-directory");
    std::fs::write(&blocker, "unchanged").unwrap();
    invoke_in(&blocker.join("home"), args, input)
}

fn invoke_in(home: &std::path::Path, args: &[&str], input: Option<&[u8]>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
        .args(args)
        .env("RIGSPARK_HOME", home)
        .env("PATH", "")
        .env("TERM", "dumb")
        .env_remove("RIGSPARK_TUI")
        .env_remove("NO_COLOR")
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
        let mut stdin = child.stdin.take().unwrap();
        // The CLI may reject oversized input before reading all of it.
        let _ = stdin.write_all(input);
    }
    child.wait_with_output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn rejects(args: &[&str], message: &str) {
    let output = invoke(args, None);
    assert_eq!(
        output.status.code(),
        Some(1),
        "{args:?}: {}",
        stderr(&output)
    );
    assert!(
        stderr(&output).contains(message),
        "{args:?}: expected {message:?}, got {}",
        stderr(&output)
    );
}

fn passes_rule(args: &[&str], message: &str) {
    let output = invoke(args, None);
    assert!(
        !stderr(&output).contains(message),
        "{args:?} should pass the {message:?} rule: {}",
        stderr(&output)
    );
}

fn models_json() -> String {
    concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../rigspark-core/data/models.json"
    )
    .into()
}

#[test]
fn generation_catalog_only_combines_with_hardware() {
    let models = models_json();
    for extra in [
        vec!["--refresh"],
        vec!["--all"],
        vec!["--tui"],
        vec!["--accessible"],
        vec!["--catalog-path", &models],
        vec!["--perf-path", &models],
        vec!["--parity"],
    ] {
        let mut args = vec!["catalog", "--generation"];
        args.extend(extra);
        rejects(&args, "--generation only combines with --hardware");
    }
}

#[test]
fn catalog_status_and_update_stand_alone() {
    let models = models_json();
    for extra in [
        vec!["--refresh"],
        vec!["--all"],
        vec!["--catalog-path", &models],
        vec!["--perf-path", &models],
        vec!["--hardware", &models],
        vec!["--hardware-json", "{}"],
        vec!["--tui"],
        vec!["--accessible"],
        vec!["--generation"],
    ] {
        for flag in ["--status", "--update"] {
            let mut args = vec!["catalog", flag];
            args.extend(extra.iter().copied());
            rejects(&args, "--update and --status require catalog");
        }
    }
}

#[test]
fn accessible_output_excludes_plain_json_and_one_shot_modes() {
    let conflict = "--accessible conflicts with --no-tui, --json, and --message";
    rejects(&["recommend", "--accessible", "--no-tui"], conflict);
    rejects(&["recommend", "--accessible", "--json"], conflict);
    rejects(&["chat", "--accessible", "--message", "hi"], conflict);
}

#[test]
fn references_and_messages_must_be_printable_text() {
    rejects(&["can-run", "   "], "invalid model reference");
    rejects(&["can-run", &"m".repeat(8193)], "invalid model reference");
    rejects(&["can-run", "qwen\u{1}3"], "invalid model reference");
    rejects(&["chat", "--message", "   "], "--message must contain text");
    passes_rule(
        &["chat", "--message", &"x".repeat(3000)],
        "--message must contain text",
    );
    passes_rule(&["chat", "--message", "hi"], "--message must contain text");
}

#[test]
fn installed_runtimes_need_bypass_and_ollama() {
    let rule = "installed models require --bypass and Ollama";
    rejects(&["up", "qwen3:8b", "--installed"], rule);
    rejects(
        &[
            "up",
            "qwen3:8b",
            "--installed",
            "--bypass",
            "--backend",
            "llamacpp",
        ],
        rule,
    );
    passes_rule(&["up", "qwen3:8b", "--installed", "--bypass"], rule);
}

#[test]
fn tui_is_offered_to_read_only_commands_and_chat() {
    // Both reach terminal detection, which fails without a TTY, rather than the flag rule.
    for command in ["recommend", "chat"] {
        let output = invoke(&[command, "--tui"], None);
        assert!(
            stderr(&output).contains("interactive UI is incompatible"),
            "{command}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn chat_and_migration_options_are_scoped_to_their_commands() {
    for extra in [
        vec!["-m", "x"],
        vec!["--harness", "claude"],
        vec!["--message", "hi"],
        vec!["--agent", "a"],
        vec!["--skill", "s"],
        vec!["--no-memory"],
    ] {
        let mut args = vec!["recommend", "--parity"];
        args.extend(extra);
        rejects(&args, "chat options require chat");
    }
    for extra in [
        vec!["--from", "a"],
        vec!["--to", "b"],
        vec!["--move"],
        vec!["--dry-run"],
        vec!["--yes"],
    ] {
        let mut args = vec!["recommend", "--parity"];
        args.extend(extra);
        rejects(&args, "migration options require migrate");
    }
    // connect previews with --dry-run, and down confirms with --yes.
    for args in [
        ["connect", "--dry-run", "--parity"],
        ["down", "--yes", "--parity"],
    ] {
        let output = invoke(&args, Some(b"[]"));
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stderr(&output)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "[]");
    }
}

#[test]
fn installed_comparison_keeps_to_ollama_context_options() {
    rejects(
        &["recommend", "--fits-only"],
        "--fits-only requires recommend --installed",
    );
    passes_rule(
        &["recommend", "--installed", "--fits-only"],
        "--fits-only requires recommend --installed",
    );
    let rule = "installed comparison only supports Ollama context options";
    for extra in [
        vec!["--max-context"],
        vec!["--available-backends"],
        vec!["--backend", "llamacpp"],
    ] {
        let mut args = vec!["recommend", "--installed"];
        args.extend(extra);
        rejects(&args, rule);
    }
    passes_rule(&["recommend", "--installed", "--backend", "ollama"], rule);
}

#[test]
fn seeds_are_bounded_by_the_largest_exact_float_integer() {
    rejects(
        &["generate", "--seed", "9007199254740992"],
        "invalid --seed 9007199254740992",
    );
    passes_rule(
        &["generate", "--seed", "9007199254740991"],
        "invalid --seed",
    );
}

#[test]
fn only_generate_takes_an_aspect() {
    passes_rule(
        &["generate", "--aspect", "landscape"],
        "--aspect is not supported by this command",
    );
    rejects(
        &["recommend", "--aspect", "landscape"],
        "--aspect is not supported by this command",
    );
}

#[test]
fn parity_mode_answers_every_command_from_stdin() {
    for command in ["bench", "connect", "add"] {
        let output = invoke(&[command, "--parity"], Some(b"[]"));
        assert_eq!(
            output.status.code(),
            Some(0),
            "{command}: {}",
            stderr(&output)
        );
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "[]");
    }
}

#[test]
fn runtime_commands_fail_without_a_usable_home() {
    for args in [vec!["bench"], vec!["connect", "claude"]] {
        let output = invoke(&args, None);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn add_lists_added_models_outside_parity() {
    // Listing is read-only, so it needs no usable home: Windows reports a path
    // through a file as missing, which reads as an empty list there. Use a real,
    // empty home so the result is the same everywhere.
    let home = tempfile::tempdir().unwrap();
    let output = invoke_in(home.path(), &["add"], None);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("You have not added any models"),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn parity_input_is_capped_at_eight_mib_and_1024_requests() {
    let limit = 8 * 1024 * 1024;
    let mut exact = vec![b' '; limit - 2];
    exact.extend(b"[]");
    let output = invoke(&["recommend", "--parity"], Some(&exact));
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let mut over = vec![b' '; limit - 1];
    over.extend(b"[]");
    let output = invoke(&["recommend", "--parity"], Some(&over));
    assert!(stderr(&output).contains("parity input exceeds 8 MiB"));
    // Requests are counted before any is evaluated: invalid hardware only fails later.
    let request = r#"{"hardware":{"arch":"x64","platform":"linux","totalRamBytes":-1,"freeRamBytes":0,"freeDiskBytes":0,"gpu":[]}}"#;
    let batch = |count: usize| format!("[{}]", vec![request; count].join(","));
    for (count, too_many) in [(1024, false), (1025, true), (1026, true)] {
        let output = invoke(&["recommend", "--parity"], Some(batch(count).as_bytes()));
        assert_eq!(output.status.code(), Some(1), "{count}");
        assert_eq!(
            stderr(&output).contains("too many parity requests"),
            too_many,
            "{count}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn chat_and_migrate_refuse_parity_mode() {
    for command in ["chat", "migrate"] {
        rejects(
            &[command, "--parity"],
            "advice and lifecycle options cannot be used with chat or migrate",
        );
    }
}

#[test]
fn refresh_belongs_to_catalog_outside_parity() {
    let rule = "--refresh is only supported by catalog";
    rejects(&["catalog", "--refresh", "--parity"], rule);
    passes_rule(&["catalog", "--refresh"], rule);
}

#[test]
fn json_chat_reads_at_most_one_mib_from_stdin() {
    let rule = "chat input exceeds 1 MiB";
    let limit = 1024 * 1024;
    let over = invoke(&["chat", "--json"], Some(&vec![b'x'; limit + 1]));
    assert_eq!(over.status.code(), Some(1), "{}", stderr(&over));
    assert!(stderr(&over).contains(rule), "{}", stderr(&over));
    let exact = invoke(&["chat", "--json"], Some(&vec![b'x'; limit]));
    assert!(!stderr(&exact).contains(rule), "{}", stderr(&exact));
}
