use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use std::{
    io::{Read, Write},
    sync::mpsc,
    time::{Duration, Instant},
};

/// A frozen catalog, so terminal goldens do not move when the shipped catalog grows.
const BASELINE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../rigspark-core/fixtures/catalog-baseline.json"
);

const HARDWARE: &str = r#"{"arch":"x64","platform":"linux","totalRamBytes":68719476736,"freeRamBytes":60000000000,"freeDiskBytes":500000000000,"gpu":[{"vendor":"nvidia","vramBytes":25769803776}]}"#;
fn final_output(output: &str) -> &str {
    let marker = if cfg!(windows) {
        "\x1b[?2004l"
    } else {
        "\x1b[?1049l"
    };
    let boundary = output.rfind(marker).expect("terminal was not restored") + marker.len();
    &output[boundary..]
}

fn plain_recommendation(json: bool) -> String {
    let home = tempfile::tempdir().unwrap();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_llmup-native"));
    command.args(["recommend", "--no-tui", "--hardware-json", HARDWARE]);
    command.args(["--catalog-path", BASELINE]);
    if json {
        command.arg("--json");
    }
    let output = command
        .env("RIGSPARK_HOME", home.path().join("unused"))
        .env("PATH", "")
        .env("TERM", "xterm-256color")
        .env_remove("RIGSPARK_TUI")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(!home.path().join("unused").exists());
    String::from_utf8(output.stdout).unwrap()
}

fn entered(output: &str) -> bool {
    output.contains(if cfg!(windows) {
        "\x1b[?2004h"
    } else {
        "\x1b[?1049h"
    })
}

fn restored(output: &str) -> bool {
    output.contains("\x1b[?2004l")
        && (cfg!(windows) || output.contains("\x1b[?1049l"))
        && output
            .rfind("\x1b[?25h")
            .zip(output.rfind("\x1b[?25l"))
            .is_some_and(|(shown, hidden)| shown > hidden)
}

struct Cleanup(Box<dyn portable_pty::Child + Send + Sync>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn run(extra: &[&str], columns: u16, rows: u16, keys: Option<&[u8]>) -> (u32, String) {
    run_command("recommend", extra, columns, rows, keys)
}

fn run_command(
    name: &str,
    extra: &[&str],
    columns: u16,
    rows: u16,
    keys: Option<&[u8]>,
) -> (u32, String) {
    run_scripted(name, extra, columns, rows, keys, &[])
}

fn run_scripted(
    name: &str,
    extra: &[&str],
    columns: u16,
    rows: u16,
    keys: Option<&[u8]>,
    answers: &[(&str, &[u8])],
) -> (u32, String) {
    run_scripted_with_fragment(name, extra, columns, rows, keys, answers, None)
}

fn run_scripted_with_fragment(
    name: &str,
    extra: &[&str],
    columns: u16,
    rows: u16,
    keys: Option<&[u8]>,
    answers: &[(&str, &[u8])],
    fragment: Option<&[u8]>,
) -> (u32, String) {
    run_session(name, extra, (columns, rows), keys, answers, fragment, None)
}

fn run_session(
    name: &str,
    extra: &[&str],
    size: (u16, u16),
    keys: Option<&[u8]>,
    answers: &[(&str, &[u8])],
    fragment: Option<&[u8]>,
    signal: Option<(&str, &str)>,
) -> (u32, String) {
    let (columns, rows) = size;
    let home = tempfile::tempdir().unwrap();
    let pair = native_pty_system()
        .openpty(PtySize {
            rows,
            cols: columns,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    #[cfg(unix)]
    let original_terminal = pair.master.get_termios().unwrap();
    #[cfg(unix)]
    let plain_bypass = extra.contains(&"--accessible")
        || name == "recommend"
            && extra
                .iter()
                .any(|flag| ["--no-tui", "--json"].contains(flag));
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_llmup-native"));
    command.arg(name);
    if ["recommend", "can-run", "catalog", "doctor", "up", "switch"].contains(&name) {
        command.args(["--hardware-json", HARDWARE]);
        command.args(["--catalog-path", BASELINE]);
    }
    command.args(extra);
    command.env("TERM", "xterm-256color");
    command.env_remove("NO_COLOR");
    command.env_remove("FORCE_COLOR");
    command.env_remove("RIGSPARK_TUI");
    command.env("PATH", "");
    command.env("RIGSPARK_HOME", home.path().join("unused"));
    for name in [
        "CI",
        "GITHUB_ACTIONS",
        "GITLAB_CI",
        "TF_BUILD",
        "BUILDKITE",
        "JENKINS_URL",
    ] {
        command.env_remove(name);
    }
    let mut child = Cleanup(pair.slave.spawn_command(command).unwrap());
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (sender, receiver) = mpsc::sync_channel(16);
    // Keep draining after the receiver is gone: ConPTY cannot close while its output is unread.
    let reader_thread = std::thread::spawn(move || {
        let mut buffer = [0; 4096];
        while let Ok(count @ 1..) = reader.read(&mut buffer) {
            let _ = sender.send(buffer[..count].to_vec());
        }
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut output = Vec::new();
    let mut sent = false;
    let mut fragment_deadline = None;
    let mut answered_cursor_queries = 0;
    let mut answered_prompts = 0;
    let mut signal_sent = false;
    let exit = loop {
        #[cfg(unix)]
        if plain_bypass {
            assert_eq!(
                pair.master.get_termios().unwrap(),
                original_terminal,
                "plain bypass changed terminal mode"
            );
        }
        #[cfg(unix)]
        if Instant::now() >= deadline
            && let Some(pid) = child.0.process_id()
        {
            let _ = std::process::Command::new("/bin/kill")
                .args(["-KILL", &pid.to_string()])
                .status();
        }
        assert!(
            Instant::now() < deadline,
            "PTY deadline: {}",
            String::from_utf8_lossy(&output)
        );
        match receiver.recv_timeout(Duration::from_millis(5)) {
            Ok(bytes) => output.extend_from_slice(&bytes),
            Err(mpsc::RecvTimeoutError::Disconnected) => break child.0.wait().unwrap(),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(exit) = child.0.try_wait().unwrap() {
                    break exit;
                }
            }
        }
        assert!(output.len() < 1024 * 1024);
        if !signal_sent && let Some((prompt, signal)) = signal {
            let text = String::from_utf8_lossy(&output);
            if text.split_once(prompt).is_some_and(|(_, frame)| {
                extra.contains(&"--accessible") || frame.contains("\x1b[?25l")
            }) {
                assert!(
                    std::process::Command::new("/bin/kill")
                        .args([signal, &child.0.process_id().unwrap().to_string()])
                        .status()
                        .unwrap()
                        .success()
                );
                signal_sent = true;
            }
        }
        if fragment_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            writer.write_all(fragment.unwrap()).unwrap();
            writer.flush().unwrap();
            fragment_deadline = None;
        }
        let cursor_queries = output
            .windows(4)
            .filter(|bytes| *bytes == b"\x1b[6n")
            .count();
        while answered_cursor_queries < cursor_queries {
            writer.write_all(b"\x1b[1;1R").unwrap();
            writer.flush().unwrap();
            answered_cursor_queries += 1;
        }
        if let Some((prompt, answer)) = answers.get(answered_prompts)
            && String::from_utf8_lossy(&output).contains(prompt)
            && (fragment.is_none()
                || String::from_utf8_lossy(&output)
                    .split(prompt)
                    .nth(1)
                    .is_some_and(|frame| frame.contains("\x1b[?25l")))
        {
            writer.write_all(answer).unwrap();
            writer.flush().unwrap();
            answered_prompts += 1;
        }
        if !sent && entered(&String::from_utf8_lossy(&output)) {
            if let Some(keys) = keys {
                writer.write_all(keys).unwrap();
                writer.flush().unwrap();
                if fragment.is_some() {
                    fragment_deadline = Some(Instant::now() + Duration::from_millis(20));
                }
            }
            sent = true;
        }
    };
    #[cfg(unix)]
    assert_eq!(
        pair.master.get_termios().unwrap(),
        original_terminal,
        "terminal attributes were not restored"
    );
    drop(writer);
    // ConPTY only reaches EOF once the child handle and master are released.
    drop(child);
    drop(pair.master);
    let drain_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(bytes) => output.extend_from_slice(&bytes),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                reader_thread.join().unwrap();
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= drain_deadline => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    assert!(!home.path().join("unused").exists());
    (
        exit.exit_code(),
        String::from_utf8_lossy(&output).into_owned(),
    )
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY re-renders the output stream, so byte-exact restore sequences cannot hold"
)]
fn visual_report_accepts_search_and_restores_terminal_on_exit() {
    let (exit, output) = run(&["--tui"], 80, 24, Some(b"/qwen\rn\x1b[Bq"));
    assert_eq!(exit, 0, "{output}");
    assert!(entered(&output));
    assert!(restored(&output));
    assert!(output.contains("Recommend"), "{output}");
    let report = plain_recommendation(false);
    let normalized = output.replace("\r\n", "\n");
    let final_text = final_output(&normalized);
    assert!(final_text.ends_with(&report), "{output}");
    assert_eq!(normalized.matches(&report).count(), 1, "{output}");
    assert_eq!(
        normalized.matches("Ranked local LLMs for").count(),
        1,
        "{output}"
    );
    assert!(final_text.contains("Ranked local LLMs for"), "{output}");
}

#[cfg(unix)]
#[test]
fn hangup_restores_visual_terminal_without_state_changes() {
    for (command, mode, prompt) in [
        ("catalog", "--tui", "Catalog"),
        ("up", "--tui", "up / choose model"),
        ("doctor", "--tui", "doctor"),
        ("chat", "--tui", "rigspark / chat"),
        ("recommend", "--accessible", "p finish and print result"),
        ("chat", "--accessible", "Chatting using local."),
        (
            "up",
            "--accessible",
            "Enter a model number, or q to cancel.",
        ),
        ("down", "--accessible", "Choose 1 or 2, then press Enter:"),
        ("ls", "--accessible", "rigspark up <model>"),
    ] {
        for (signal, expected) in [("-HUP", 129), ("-TERM", 143), ("-INT", 130)] {
            let (exit, output) = run_session(
                command,
                &[mode],
                (100, 30),
                None,
                &[],
                None,
                Some((prompt, signal)),
            );
            assert_eq!(exit, expected, "{command} {signal}: {output}");
            if mode == "--tui" {
                assert!(restored(&output), "{output}");
                assert!(output.rfind("\x1b[?25h").unwrap() > output.rfind("\x1b[?25l").unwrap());
            } else {
                assert!(!entered(&output), "{output}");
                assert!(!output.contains('\x1b'), "{output}");
            }
        }
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY rewrites fragmented escape input and re-renders the output stream"
)]
fn visual_picker_fragmented_end_selects_last_model_without_cancelling() {
    for (initial, fragment, expected) in [
        (b"\x1b".as_slice(), b"[F\r".as_slice(), "mistral:7b"),
        (b"\x1b[B\x1b".as_slice(), b"[H\r".as_slice(), "qwen3.6:35b"),
        (b"\x1b[B".as_slice(), b"\r".as_slice(), "bonsai:8b"),
    ] {
        let (exit, output) = run_scripted_with_fragment(
            "up",
            &["--tui"],
            120,
            30,
            Some(initial),
            &[("up / confirm", b"q")],
            Some(fragment),
        );
        assert_eq!(exit, 130, "{output}");
        let confirmation = output
            .split("up / confirm")
            .nth(1)
            .expect("confirmation missing");
        assert!(confirmation.contains(expected), "{output}");
        assert!(restored(&output), "{output}");
    }
    for initial in [b"\x1b".as_slice(), b"\x03", b"q"] {
        let (exit, output) = run_command("up", &["--tui"], 120, 30, Some(initial));
        assert_eq!(exit, 130, "{output}");
        assert!(!output.contains("up / confirm"), "{output}");
        assert!(restored(&output), "{output}");
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY re-renders the output stream, so byte-exact reports cannot hold"
)]
fn eligible_plain_and_json_overrides_never_enter_ui_and_emit_one_report() {
    for (flag, json) in [("--no-tui", false), ("--json", true)] {
        let (exit, output) = run(&[flag], 100, 30, None);
        assert_eq!(exit, 0, "{output}");
        assert!(!entered(&output), "{output}");
        assert!(!output.contains('\x1b'), "{output}");
        assert!(!output.contains("Accessible"), "{output}");
        assert!(!output.contains("Controls"), "{output}");
        assert!(!output.contains("press Enter"), "{output}");
        let normalized = output.replace("\r\n", "\n");
        if json {
            let report: serde_json::Value = serde_json::from_str(&normalized).unwrap();
            assert!(!report["ranked"].as_array().unwrap().is_empty());
            assert_eq!(
                report,
                serde_json::from_str::<serde_json::Value>(&plain_recommendation(true)).unwrap()
            );
        } else {
            assert_eq!(normalized, plain_recommendation(false));
            assert_eq!(normalized.matches("Ranked local LLMs for").count(), 1);
        }
    }
}

#[test]
fn catalog_fragmented_end_keeps_the_visual_session_open() {
    let (exit, output) = run_scripted_with_fragment(
        "catalog",
        &["--tui"],
        120,
        30,
        Some(b"\x1b"),
        &[],
        Some(b"[F\rq"),
    );
    assert_eq!(exit, 0, "{output}");
    let visual = output.split("\x1b[?2004l").next().unwrap();
    assert!(visual.contains("mistral:7b"), "{output}");
    assert!(restored(&output), "{output}");
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY rewrites terminal input payloads and re-renders the output stream"
)]
fn terminal_payloads_cannot_dispatch_catalog_shortcuts() {
    for payload in [
        b"\x1b[200~q c\x03\x1b[201~".as_slice(),
        b"\x1b]0;q c\x07",
        b"\x1bPq c\x07q\x1b\\",
        b"\x1bXq c\x03\x1b\\",
        b"\x1b^q c\x1b\\",
        b"\x1b_q c\x1b\\",
        "\u{9d}q c\u{9c}".as_bytes(),
    ] {
        let mut keys = payload.to_vec();
        keys.push(b'?');
        let (exit, output) = run_scripted(
            "catalog",
            &["--tui"],
            100,
            30,
            Some(&keys),
            &[("Keyboard help", b"q")],
        );
        assert_eq!(exit, 0, "{output:?}");
        assert!(
            output.contains("Keyboard help"),
            "payload dispatched a shortcut: {output:?}"
        );
        assert!(restored(&output), "{output:?}");
    }
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY rewrites bracketed-paste input and re-renders the output stream"
)]
fn fragmented_paste_markers_do_not_dispatch_payload_shortcuts() {
    let (exit, output) = run_scripted_with_fragment(
        "catalog",
        &["--tui"],
        100,
        30,
        Some(b"\x1b"),
        &[("Keyboard help", b"q")],
        Some(b"[200~q c\x03\x1b[201~?"),
    );
    assert_eq!(exit, 0, "{output:?}");
    assert!(output.contains("Keyboard help"), "{output:?}");
    assert!(restored(&output), "{output:?}");
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY cannot deliver invalid UTF-8 input and re-renders the output stream"
)]
fn accessible_render_failure_preserves_one_authoritative_report() {
    let (exit, output) = run_scripted(
        "recommend",
        &["--accessible"],
        100,
        30,
        None,
        &[("p finish and print result", b"\xff\r")],
    );
    assert_eq!(exit, 0, "{output}");
    // macOS 14 PTYs can duplicate a carriage return at a 4 KiB output boundary.
    let normalized = output.replace('\r', "");
    let report = plain_recommendation(false);
    assert!(normalized.ends_with(&report), "{output}");
    assert_eq!(normalized.matches(&report).count(), 1, "{output}");
    assert_eq!(
        normalized
            .matches("interactive UI failed (renderer_runtime)")
            .count(),
        1
    );
    assert!(!entered(&output));
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY re-renders the output stream, so byte-exact restore sequences cannot hold"
)]
fn recommend_tui_no_color_preserves_visual_session_without_colored_sgr() {
    let color = regex::Regex::new(r"\x1b\[([0-9:;]*)m").unwrap();
    for no_color in [false, true] {
        let args = if no_color {
            vec!["--tui", "--no-color"]
        } else {
            vec!["--tui"]
        };
        let (exit, output) = run(&args, 100, 30, Some(b"q"));
        assert_eq!(exit, 0, "{output}");
        assert!(entered(&output), "{output}");
        assert!(restored(&output), "{output}");
        assert!(output.contains("Recommend"), "{output}");
        let has_color = color.captures_iter(&output).any(|capture| {
            capture[1].split([';', ':']).any(|parameter| {
                parameter.parse::<u16>().is_ok_and(|value| {
                    (30..=38).contains(&value)
                        || (40..=48).contains(&value)
                        || value == 58
                        || (90..=97).contains(&value)
                        || (100..=107).contains(&value)
                })
            })
        });
        assert_eq!(has_color, !no_color, "{output}");
    }
}

#[test]
fn visual_doctor_unhealthy_diagnostics_restore_terminal_and_exit_one() {
    let (exit, output) = run_command("doctor", &["--tui"], 100, 30, Some(b"q"));
    assert_eq!(exit, 1, "{output}");
    assert!(entered(&output), "{output}");
    assert!(restored(&output), "{output}");
    assert!(output.contains("doctor"), "{output}");
    let final_text = final_output(&output);
    assert!(final_text.contains("not installed"), "{output}");
    assert_eq!(
        final_text.matches("AI Hardware Score:").count(),
        1,
        "{output}"
    );
}

#[test]
fn visual_catalog_filters_opens_details_and_preserves_final_output() {
    let (exit, output) = run_command(
        "catalog",
        &["--all", "--tui"],
        100,
        30,
        Some(b"/llama3.1:8b\r\r\tq"),
    );
    assert_eq!(exit, 0, "{output}");
    assert!(entered(&output));
    assert!(restored(&output));
    assert!(output.contains("Catalog"), "{output}");
    assert!(output.contains("Catalog (Filter: all"), "{output}");
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY re-renders the output stream, so byte-exact restore sequences cannot hold"
)]
fn visual_shutdown_runs_and_restores_each_presentation_without_state() {
    let (exit, output) = run_scripted(
        "down",
        &["--tui"],
        100,
        30,
        None,
        &[("down / confirm", b"\x1b[B\r"), ("down / result", b"q")],
    );
    assert_eq!(exit, 0, "{output}");
    assert!(entered(&output));
    assert!(restored(&output));
    assert!(output.contains("down / recorded servers"), "{output}");
    assert!(output.contains("Result: no-active"), "{output}");
    assert!(output.contains("No active server to stop."), "{output}");
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY translates Ctrl-C and re-renders the output stream"
)]
fn visual_shutdown_result_interrupt_suppresses_final_success_output() {
    let (exit, output) = run_scripted(
        "down",
        &["--tui", "--yes"],
        100,
        30,
        None,
        &[("down / result", &[3])],
    );
    assert_eq!(exit, 130, "{output}");
    assert!(restored(&output));
    assert!(output.contains("Result: no-active"), "{output}");
    assert_eq!(
        output.matches("No active server to stop.").count(),
        2,
        "{output}"
    );
}

#[test]
fn visual_comparison_and_help_do_not_execute_model_actions() {
    let (exit, output) = run(&["--tui"], 100, 30, Some(b" j c??q"));
    assert_eq!(exit, 0, "{output}");
    assert!(entered(&output));
    assert!(restored(&output));
    assert!(output.contains("rigspark up"), "{output}");
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY translates Ctrl-C and re-renders the output stream"
)]
fn visual_can_run_preserves_verdict_exit_and_interrupt_contracts() {
    for (model, expected) in [("llama3.1:8b", 0), ("deepseek-r1:671b", 1)] {
        let (exit, output) = run_command("can-run", &[model, "--tui"], 100, 30, Some(b"q"));
        assert_eq!(exit, expected, "{output}");
        assert!(entered(&output));
        assert!(restored(&output));
        assert!(output.contains("Can Run"), "{output}");
        assert!(output.contains(&format!("{model}: ")), "{output}");
    }
    let (exit, output) = run_command("can-run", &["llama3.1:8b", "--tui"], 100, 30, Some(&[3]));
    assert_eq!(exit, 130, "{output}");
    assert!(restored(&output));
    assert!(!output.contains("llama3.1:8b: "), "{output}");
}

#[test]
fn small_terminal_falls_back_but_explicit_request_fails_before_rendering() {
    let (exit, output) = run(&[], 40, 10, None);
    assert_eq!(exit, 0, "{output}");
    assert!(!entered(&output));
    let (exit, output) = run(&["--tui"], 40, 10, None);
    assert_ne!(exit, 0);
    assert!(output.contains("terminal_width"));
    assert!(!entered(&output));
}

#[test]
fn raw_control_c_restores_terminal_and_returns_130() {
    let (exit, output) = run(&["--tui"], 80, 24, Some(&[3]));
    assert_eq!(exit, 130, "{output}");
    assert!(restored(&output));
}

#[test]
fn visual_chat_can_exit_without_runtime_or_state_access() {
    let (exit, output) = run_command("chat", &["--tui"], 80, 24, Some(b"\x1b"));
    assert_eq!(exit, 0, "{output}");
    assert!(restored(&output));
    assert!(output.contains("Chat session ended: 0 turns, 0 memory warnings."));
}

#[test]
fn model_picker_and_lifecycle_confirmation_cancel_before_side_effects() {
    for (name, extra) in [
        ("can-run", vec!["--tui"]),
        ("up", vec!["llama3.1:8b", "--tui"]),
        ("switch", vec!["llama3.1:8b", "--tui"]),
        ("down", vec!["--tui"]),
    ] {
        let (exit, output) = run_command(name, &extra, 80, 24, Some(b"\x1b"));
        assert_eq!(exit, 130, "{name}: {output}");
        assert!(restored(&output), "{name}: {output}");
        let final_text = final_output(&output);
        for report in [
            "llama3.1:8b: ",
            " ready at ",
            "Switched",
            "No active server to stop.",
        ] {
            assert!(!final_text.contains(report), "{name}: {output}");
        }
    }
}

#[test]
fn accessible_lifecycle_defaults_to_cancel_without_raw_mode_or_state() {
    for (name, args) in [
        ("up", vec!["llama3.1:8b", "--accessible"]),
        ("switch", vec!["llama3.1:8b", "--accessible"]),
        ("down", vec!["--accessible"]),
    ] {
        let (exit, output) = run_scripted(
            name,
            &args,
            40,
            10,
            None,
            &[("Choose 1 or 2, then press Enter:", b"\r")],
        );
        assert_eq!(exit, 130, "{name}: {output}");
        assert!(output.contains("1. Cancel (default)"));
        assert!(!entered(&output));
        assert!(!output.contains(" ready at "), "{output}");
        assert!(!output.contains("No active server to stop."), "{output}");
    }
}

#[test]
fn accessible_model_choice_and_review_share_one_cooked_input_reader() {
    let (exit, output) = run_scripted(
        "up",
        &["--accessible"],
        40,
        10,
        None,
        &[
            ("Enter a model number, or q to cancel.", b"1\r"),
            ("Choose 1 or 2, then press Enter:", b"1\r"),
        ],
    );
    assert_eq!(exit, 130, "{output}");
    assert!(output.contains("Confirm activation"));
    assert!(!entered(&output));
    assert!(!output.contains(" ready at "), "{output}");
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY translates Ctrl-C and re-renders the output stream"
)]
fn accessible_confirmation_control_c_exits_130_without_state() {
    let (exit, output) = run_scripted(
        "down",
        &["--accessible"],
        40,
        10,
        None,
        &[("Choose 1 or 2, then press Enter:", &[3])],
    );
    assert_eq!(exit, 130, "{output}");
    assert!(!entered(&output));
}

#[test]
fn accessible_active_server_uses_cooked_help_then_prints_plain_result() {
    let (exit, output) = run_scripted(
        "ls",
        &["--accessible"],
        40,
        10,
        None,
        &[
            ("rigspark up <model>", b"?\r"),
            ("Commands: ? help; q quit", b"q\r"),
        ],
    );
    assert_eq!(exit, 0, "{output}");
    assert!(output.contains("Active Server / Accessible"));
    assert_eq!(output.matches("No active model.").count(), 2, "{output}");
    assert!(!entered(&output));
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY translates Ctrl-C and re-renders the output stream"
)]
fn accessible_active_server_interrupt_does_not_print_final_result() {
    let (exit, output) = run_scripted(
        "ls",
        &["--accessible"],
        40,
        10,
        None,
        &[("rigspark up <model>", &[3])],
    );
    assert_eq!(exit, 130, "{output}");
    assert_eq!(output.matches("No active model.").count(), 1, "{output}");
    assert!(!entered(&output));
}

#[test]
fn accessible_doctor_preserves_failed_diagnostics_and_cooked_navigation() {
    let (exit, output) = run_scripted(
        "doctor",
        &["--accessible"],
        80,
        24,
        None,
        &[
            ("Suggested commands are text only.", b"?\r"),
            ("Commands: ? help; q quit\r\n", b"q\r"),
        ],
    );
    assert_eq!(exit, 1, "{output}");
    assert!(output.contains("Doctor / Accessible"), "{output}");
    assert!(output.contains("not installed"), "{output}");
    assert!(output.contains("AI Hardware Score:"), "{output}");
    assert!(!entered(&output));
}

#[test]
fn accessible_can_run_preserves_evidence_plain_result_and_verdict_exit() {
    for (model, exit_code) in [("llama3.1:8b", 0), ("deepseek-r1:671b", 1)] {
        let (exit, output) = run_scripted(
            "can-run",
            &[model, "--accessible"],
            80,
            24,
            None,
            &[("5. Controls", b"q\r")],
        );
        assert_eq!(exit, exit_code, "{output}");
        assert!(output.contains("Can Run / Accessible"), "{output}");
        assert!(
            output.contains("usable bytes") || output.contains("does not fit:"),
            "{output}"
        );
        assert!(output.contains(&format!("{model}: ")), "{output}");
        if exit_code == 1 {
            assert!(output.contains("Unknown reason: not-evaluated-model-does-not-fit"));
            assert!(!output.contains("Next: rigspark up"));
        }
        assert!(!entered(&output));
    }
}

#[test]
fn accessible_can_run_picker_and_report_share_input_and_allow_cancel() {
    let catalog = rigspark_core::catalog::Catalog::parse(include_str!(
        "../../rigspark-core/fixtures/catalog-baseline.json"
    ))
    .unwrap();
    let index = catalog
        .models
        .iter()
        .position(|model| model.id == "llama3.1:8b")
        .unwrap()
        + 1;
    let answer = format!("{index}\r");
    let (exit, output) = run_scripted(
        "can-run",
        &["--accessible"],
        80,
        24,
        None,
        &[
            ("Enter a model number, or q to cancel.", answer.as_bytes()),
            ("5. Controls", b"q\r"),
        ],
    );
    assert_eq!(exit, 0, "{output}");
    assert!(output.contains("llama3.1:8b: "), "{output}");
    assert!(!entered(&output));
    let (exit, output) = run_scripted(
        "can-run",
        &["--accessible"],
        40,
        10,
        None,
        &[("Enter a model number, or q to cancel.", b"q\r")],
    );
    assert_eq!(exit, 130, "{output}");
    assert!(!output.contains("Can Run / Accessible"));
    assert!(!output.contains("llama3.1:8b: "), "{output}");
    assert!(!entered(&output));
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY translates Ctrl-C and re-renders the output stream"
)]
fn accessible_can_run_interrupt_skips_final_plain_output() {
    let (exit, output) = run_scripted(
        "can-run",
        &["llama3.1:8b", "--accessible"],
        80,
        24,
        None,
        &[("5. Controls", &[3])],
    );
    assert_eq!(exit, 130, "{output}");
    assert!(!output.contains("llama3.1:8b: "));
    assert!(!entered(&output));
}

#[test]
fn accessible_can_run_shows_unknown_geometry_before_quitting() {
    let (exit, output) = run_scripted(
        "can-run",
        &["qwen3:8b", "--context", "8192", "--accessible"],
        80,
        24,
        None,
        &[(
            "Requested context fit unknown: attention geometry unavailable",
            b"q\r",
        )],
    );
    assert_eq!(exit, 0, "{output}");
    assert!(
        output.contains("Requested context fit unknown: attention geometry unavailable"),
        "{output}"
    );
    assert!(!entered(&output));
}

#[test]
fn accessible_catalog_search_details_and_refresh_are_read_only() {
    let (exit, output) = run_scripted(
        "catalog",
        &["--all", "--refresh", "--accessible"],
        100,
        30,
        None,
        &[
            ("4. Controls", b"/llama3.1\r"),
            ("Filter: llama3.1", b"1\r"),
            ("Details: llama3.1:", b"q\r"),
        ],
    );
    assert_eq!(exit, 0, "{output}");
    for expected in [
        "Catalog / Accessible",
        "KV bytes/token",
        "Dry-run diff:",
        "Refresh (dry-run):",
        "Catalog (Filter: all",
    ] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
    assert!(!entered(&output));
}

#[test]
fn accessible_recommendation_search_details_and_print_never_execute() {
    let (exit, output) = run_scripted(
        "recommend",
        &["--accessible"],
        100,
        30,
        None,
        &[
            ("4. Controls", b"/llama3.1:8b\r"),
            ("Filter: llama3.1:8b", b"1\r"),
            ("Details: llama3.1:8b", b"p\r"),
        ],
    );
    assert_eq!(exit, 0, "{output}");
    assert!(output.contains("Recommend / Accessible"), "{output}");
    assert!(output.contains("scores quality"), "{output}");
    assert!(output.contains("rigspark up"), "{output}");
    assert!(!entered(&output));
}

#[test]
#[cfg_attr(
    windows,
    ignore = "ConPTY translates Ctrl-C and re-renders the output stream"
)]
fn accessible_model_lists_interrupt_without_final_plain_result() {
    for (name, final_output) in [
        ("recommend", "Ranked local LLMs for"),
        ("catalog", "Catalog (Filter:"),
    ] {
        let (exit, output) = run_scripted(
            name,
            &["--accessible"],
            80,
            24,
            None,
            &[("4. Controls", &[3])],
        );
        assert_eq!(exit, 130, "{name}: {output}");
        assert!(!entered(&output));
        assert!(!output.contains(final_output), "{name}: {output}");
    }
}
