#[path = "../src/gui_launcher.rs"]
mod gui_launcher;

use gui_launcher::{GuiLaunchError, GuiOptions, LaunchSpec};
use std::path::PathBuf;

fn installed_cli() -> PathBuf {
    std::env::temp_dir()
        .join("installation with spaces")
        .join("llmup")
}

#[test]
fn default_contract_uses_only_installed_sibling_and_port() {
    let executable = installed_cli();
    let spec = LaunchSpec::new(&executable, GuiOptions::new(None, false).unwrap()).unwrap();
    assert_eq!(
        spec.executable(),
        executable.with_file_name(if cfg!(windows) {
            "rigspark-gui.exe"
        } else {
            "rigspark-gui"
        })
    );
    assert_eq!(spec.args(), ["--port", "4000", "--startup-json"]);
    assert_eq!(spec.origin(), "http://127.0.0.1:4000");
    assert!(spec.open_browser());
}

#[test]
fn json_suppresses_browser_and_forwards_harness_for_child_validation() {
    let options = GuiOptions::new(Some("4321"), false)
        .unwrap()
        .with_harness(Some(" openai "))
        .unwrap()
        .with_json(true);
    let spec = LaunchSpec::new(&installed_cli(), options).unwrap();
    assert_eq!(
        spec.args(),
        ["--port", "4321", "--startup-json", "--harness", "openai"]
    );
    assert!(!spec.open_browser());
}

#[test]
fn invalid_harness_never_echoes_untrusted_input() {
    for harness in ["", "  ", "unknown", "secret\n\u{1b}[31m", "--json"] {
        let error = GuiOptions::new(None, false)
            .unwrap()
            .with_harness(Some(harness))
            .unwrap_err();
        assert_eq!(error, GuiLaunchError::InvalidHarness);
        assert!(!error.to_string().contains("secret"));
    }
}

#[test]
fn no_open_is_owned_by_launcher_not_forwarded_to_native_gui() {
    for port in ["1", "65535", "004321"] {
        let spec =
            LaunchSpec::new(&installed_cli(), GuiOptions::new(Some(port), true).unwrap()).unwrap();
        assert_eq!(
            spec.args(),
            [
                "--port",
                &port.parse::<u16>().unwrap().to_string(),
                "--startup-json"
            ]
        );
        assert!(!spec.open_browser());
    }
}

#[test]
fn invalid_ports_never_echo_untrusted_input() {
    for port in [
        "",
        "0",
        "65536",
        "-1",
        "+1",
        "4.5",
        " 4000",
        "4000 ",
        "4e3",
        "--help",
        "secret\n\u{1b}[31m",
    ] {
        let error = GuiOptions::new(Some(port), false).unwrap_err();
        assert_eq!(error, GuiLaunchError::InvalidPort);
        assert_eq!(
            error.to_string(),
            "gui: invalid --port (expected an integer in 1..65535)"
        );
    }
}

#[test]
fn relative_executable_cannot_enable_path_search() {
    assert_eq!(
        LaunchSpec::new(
            std::path::Path::new("llmup"),
            GuiOptions::new(None, false).unwrap()
        )
        .unwrap_err(),
        GuiLaunchError::ExecutableLocation
    );
}

#[test]
fn cancellation_codes_are_stable_on_all_platforms() {
    assert_eq!(gui_launcher::GuiSignal::Interrupt.exit_code(), 130);
    assert_eq!(gui_launcher::GuiSignal::Terminate.exit_code(), 143);
    assert_eq!(gui_launcher::GuiSignal::Hangup.exit_code(), 129);
}

#[test]
fn command_error_messages_preserve_direct_api_context_without_duplicate_prefix() {
    for error in [
        GuiLaunchError::InvalidPort,
        GuiLaunchError::InvalidHarness,
        GuiLaunchError::ExecutableLocation,
        GuiLaunchError::MissingExecutable,
        GuiLaunchError::Spawn,
        GuiLaunchError::Wait,
        GuiLaunchError::Shutdown,
        GuiLaunchError::Signals,
        GuiLaunchError::StartupTimeout,
        GuiLaunchError::ExitedBeforeReady,
        GuiLaunchError::InvalidReadiness,
        GuiLaunchError::Presentation,
    ] {
        let message = error.message();
        assert!(!message.is_empty());
        assert!(!message.contains("gui:"));
        assert!(!message.chars().any(char::is_control));
        assert_eq!(format!("gui: {message}"), error.to_string());
    }
    assert!(
        GuiLaunchError::MissingExecutable
            .message()
            .contains("reinstall")
    );
}

#[tokio::test]
async fn missing_sibling_fails_closed_without_echoing_install_path() {
    let _entrypoint = gui_launcher::run_gui;
    let directory = tempfile::tempdir().unwrap();
    let spec = LaunchSpec::new(
        &directory.path().join("secret-installation/llmup"),
        GuiOptions::new(None, true).unwrap(),
    )
    .unwrap();
    let error = gui_launcher::launch_with(&spec, std::future::pending(), |_, _| async {
        panic!("not ready")
    })
    .await
    .unwrap_err();
    assert_eq!(error, GuiLaunchError::MissingExecutable);
    assert!(!error.to_string().contains("secret-installation"));
}

#[cfg(unix)]
mod processes {
    use super::*;
    use gui_launcher::{GuiSignal, launch_with};
    use std::{
        future::pending, io::Write, os::unix::fs::PermissionsExt, path::Path, sync::OnceLock,
        time::Duration,
    };

    const DISPATCHER: &str = "#!/bin/sh\n. \"$0.body\"\n";

    // macOS assesses every newly created executable on first exec, serialized
    // system-wide; a fresh script per fixture made spawn latency (not the
    // launcher) consume the bounded test budgets under parallel load. Fixtures
    // therefore hard-link one pre-warmed dispatcher that sources `$0.body`.
    fn dispatcher() -> &'static Path {
        static DISPATCHER_PATH: OnceLock<PathBuf> = OnceLock::new();
        DISPATCHER_PATH.get_or_init(|| {
            let root = Path::new(env!("CARGO_TARGET_TMPDIR"));
            let path = root.join("gui-launcher-dispatcher.sh");
            let current = std::fs::read(&path).is_ok_and(|bytes| bytes == DISPATCHER.as_bytes())
                && std::fs::metadata(&path)
                    .is_ok_and(|metadata| metadata.permissions().mode() & 0o777 == 0o700);
            if !current {
                let mut file = tempfile::NamedTempFile::new_in(root).unwrap();
                file.write_all(DISPATCHER.as_bytes()).unwrap();
                file.as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o700))
                    .unwrap();
                file.persist(&path).unwrap();
            }
            let warm = tempfile::tempdir_in(root).unwrap();
            let link = warm.path().join("warm");
            std::fs::hard_link(&path, &link).unwrap();
            std::fs::write(warm.path().join("warm.body"), "exit 0\n").unwrap();
            assert!(
                std::process::Command::new(&link)
                    .status()
                    .unwrap()
                    .success()
            );
            path
        })
    }

    fn fixture(body: &str, no_open: bool) -> (tempfile::TempDir, LaunchSpec) {
        fixture_with_options(body, GuiOptions::new(None, no_open).unwrap())
    }

    fn fixture_with_options(body: &str, options: GuiOptions) -> (tempfile::TempDir, LaunchSpec) {
        let directory = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let spec = LaunchSpec::new(&directory.path().join("llmup"), options).unwrap();
        std::fs::hard_link(dispatcher(), spec.executable()).unwrap();
        let mut script = spec.executable().as_os_str().to_owned();
        script.push(".body");
        std::fs::write(script, format!("{body}\n")).unwrap();
        (directory, spec)
    }

    #[tokio::test]
    async fn accepts_structured_readiness_from_native_host() {
        let (_directory, spec) = fixture(
            "printf '%s\\n' '{\"url\":\"http://127.0.0.1:4000\",\"harness\":\"claude\",\"port\":4000}'\nexec /bin/sleep 30",
            true,
        );
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let code = launch_with(
            &spec,
            async {
                receiver.await.unwrap();
                Ok(GuiSignal::Interrupt)
            },
            |ready, open| {
                assert_eq!(ready.url(), "http://127.0.0.1:4000");
                let mut output = Vec::new();
                ready.write_startup(&mut output, true).unwrap();
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&output).unwrap(),
                    serde_json::json!({
                        "url": "http://127.0.0.1:4000", "harness": "claude", "port": 4000
                    })
                );
                assert_eq!(output.last(), Some(&b'\n'));
                output.clear();
                ready.write_startup(&mut output, false).unwrap();
                assert_eq!(output, b"rigspark GUI listening at http://127.0.0.1:4000\n");
                assert!(!open);
                async move {
                    sender.send(()).unwrap();
                    Ok(())
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(code, 130);
    }

    #[tokio::test]
    async fn json_mode_forwards_custom_port_and_harness_without_browser() {
        let options = GuiOptions::new(Some("5432"), false)
            .unwrap()
            .with_harness(Some(" openai "))
            .unwrap()
            .with_json(true);
        let (_directory, spec) = fixture_with_options(
            "[ \"$#\" = 5 ] && [ \"$1\" = --port ] && [ \"$2\" = 5432 ] && [ \"$3\" = --startup-json ] && [ \"$4\" = --harness ] && [ \"$5\" = openai ] || exit 99\nprintf '%s\\n' '{\"url\":\"http://127.0.0.1:5432\",\"harness\":\"openai\",\"port\":5432}'\nexec /bin/sleep 30",
            options,
        );
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let code = launch_with(
            &spec,
            async {
                receiver.await.unwrap();
                Ok(GuiSignal::Interrupt)
            },
            |ready, open| {
                assert!(!open);
                let mut output = Vec::new();
                ready.write_startup(&mut output, true).unwrap();
                assert_eq!(
                    output,
                    b"{\"url\":\"http://127.0.0.1:5432\",\"harness\":\"openai\",\"port\":5432}\n"
                );
                async move {
                    sender.send(()).unwrap();
                    Ok(())
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(code, 130);
    }

    #[tokio::test]
    async fn rejects_malformed_mismatched_and_secret_bearing_records() {
        for record in [
            r#"{"url":"http://127.0.0.1:4000","harness":"unknown","port":4000}"#,
            r#"{"url":"http://127.0.0.1:4000","harness":"claude","port":4000}"#,
            r#"{"url":"http://127.0.0.1:4000","harness":"local","port":4001}"#,
            r#"{"url":"http://127.0.0.1:4001","harness":"local","port":4000}"#,
            r#"{"url":"http://127.0.0.1:4000/?token=secret","harness":"local","port":4000}"#,
            r#"{"url":"http://127.0.0.1:4000#secret","harness":"local","port":4000}"#,
            r#"{"url":"http://127.0.0.1:4000","harness":"local","port":4000,"token":"secret"}"#,
            r#"{"url":"http://127.0.0.1:4000","harness":"local","port":4000,"port":4000}"#,
            r#"{"url":"http://127.0.0.1:4000","harness":"local","port":"4000"}"#,
            r#"{"url":"http://127.0.0.1:4000","port":4000}"#,
            r#"{"url":"http://127.0.0.1:4000","harness":"local","port":4000}{}"#,
            r#"{"url":"http://127.0.0.1:4000","harness":"local","port":0}"#,
            r#"{"url":"http://127.0.0.1:4000","harness":"local","port":65536}"#,
        ] {
            let options = GuiOptions::new(None, false)
                .unwrap()
                .with_harness(Some("local"))
                .unwrap();
            let (_directory, spec) = fixture_with_options(
                &format!("printf '%s\\n' '{record}'\nexec /bin/sleep 30"),
                options,
            );
            let error = launch_with(&spec, pending(), |_, _| async { panic!("not ready") })
                .await
                .unwrap_err();
            assert_eq!(error, GuiLaunchError::InvalidReadiness);
            assert!(!error.to_string().contains("secret"));
        }
    }

    #[tokio::test]
    async fn partial_and_non_utf8_readiness_fail_closed() {
        for body in [
            "printf '{\"url\":'; exec 1>&-; exec /bin/sleep 30",
            "printf '\\377\\n'; exec /bin/sleep 30",
        ] {
            let (_directory, spec) = fixture(body, true);
            let error = launch_with(&spec, pending(), |_, _| async { panic!("not ready") })
                .await
                .unwrap_err();
            assert_eq!(error, GuiLaunchError::InvalidReadiness);
        }
    }

    #[tokio::test]
    async fn forwards_exact_arguments_and_preserves_nonzero_exit() {
        let (_directory, spec) = fixture(
            "[ \"$#\" = 3 ] && [ \"$1\" = --port ] && [ \"$2\" = 4000 ] && [ \"$3\" = --startup-json ] || exit 99\nexit 23",
            true,
        );
        assert_eq!(
            launch_with(&spec, pending(), |_, _| async { panic!("not ready") })
                .await
                .unwrap(),
            23
        );
    }

    #[tokio::test]
    async fn successful_exit_without_readiness_is_not_a_running_gui() {
        let (_directory, spec) = fixture("exit 0", true);
        assert_eq!(
            launch_with(&spec, pending(), |_, _| async { panic!("not ready") })
                .await
                .unwrap_err(),
            GuiLaunchError::ExitedBeforeReady
        );
    }

    #[tokio::test]
    async fn native_signal_exit_is_mapped_without_losing_signal() {
        let (_directory, spec) = fixture("kill -TERM $$", true);
        assert_eq!(
            launch_with(&spec, pending(), |_, _| async { panic!("not ready") })
                .await
                .unwrap(),
            143
        );
    }

    #[tokio::test]
    async fn readiness_controls_open_and_shutdown_reaps_child() {
        for no_open in [false, true] {
            let (directory, spec) = fixture(
                "trap 'printf stopped > \"$0.stopped\"; exit 0' INT\nprintf '%s\\n' '{\"url\":\"http://127.0.0.1:4000\",\"harness\":\"local\",\"port\":4000}'\nwhile :; do /bin/sleep 0.02; done",
                no_open,
            );
            let (sender, receiver) = tokio::sync::oneshot::channel();
            let result = tokio::time::timeout(
                Duration::from_secs(6),
                launch_with(
                    &spec,
                    async {
                        receiver.await.unwrap();
                        Ok(GuiSignal::Terminate)
                    },
                    |ready, open| {
                        assert_eq!(ready.url(), "http://127.0.0.1:4000");
                        assert_eq!(open, !no_open);
                        async move {
                            sender.send(()).unwrap();
                            Ok(())
                        }
                    },
                ),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(result, 143);
            assert_eq!(
                std::fs::read(directory.path().join("rigspark-gui.stopped")).unwrap(),
                b"stopped"
            );
        }
    }

    #[tokio::test]
    async fn rejects_untrusted_readiness_without_logging_it() {
        for output in [
            "https://evil.example/secret",
            "http://127.0.0.1:4001",
            "http://127.0.0.1:4000/?token=secret",
            "http://0.0.0.0:4000",
            "\\033[31msecret",
        ] {
            let (_directory, spec) =
                fixture(&format!("printf '{output}\\n'\nexec /bin/sleep 30"), false);
            let error = launch_with(&spec, pending(), |_, _| async { panic!("not ready") })
                .await
                .unwrap_err();
            assert_eq!(error, GuiLaunchError::InvalidReadiness);
            assert!(!error.to_string().contains("secret"));
        }
    }

    #[tokio::test]
    async fn bounds_readiness_even_without_newline() {
        let (_directory, spec) = fixture(
            &format!("printf '{}'\nexec /bin/sleep 30", "s".repeat(1024)),
            true,
        );
        assert_eq!(
            launch_with(&spec, pending(), |_, _| async { panic!("not ready") })
                .await
                .unwrap_err(),
            GuiLaunchError::InvalidReadiness
        );
    }

    #[tokio::test]
    async fn pre_cancelled_launch_never_spawns() {
        let (directory, spec) =
            fixture("printf started > \"$0.started\"\nexec /bin/sleep 30", true);
        assert_eq!(
            launch_with(&spec, async { Ok(GuiSignal::Interrupt) }, |_, _| async {
                panic!("not ready")
            })
            .await
            .unwrap(),
            130
        );
        assert!(!directory.path().join("rigspark-gui.started").exists());
    }

    #[tokio::test]
    async fn presentation_failure_cleans_up_and_redacts_error() {
        let (_directory, spec) = fixture(
            "printf '%s\\n' '{\"url\":\"http://127.0.0.1:4000\",\"harness\":\"local\",\"port\":4000}'\nexec /bin/sleep 30",
            false,
        );
        let error = launch_with(&spec, pending(), |_, _| async {
            Err(std::io::Error::other("secret\n\u{1b}[31m"))
        })
        .await
        .unwrap_err();
        assert_eq!(error, GuiLaunchError::Presentation);
        assert!(!error.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn non_executable_files_directories_and_symlinks_fail_closed() {
        let (directory, spec) = fixture("exit 0", true);
        // Replace the shared hard link so the dispatcher keeps its mode.
        std::fs::remove_file(spec.executable()).unwrap();
        std::fs::write(spec.executable(), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(spec.executable(), std::fs::Permissions::from_mode(0o600))
            .unwrap();
        assert_eq!(
            launch_with(&spec, pending(), |_, _| async { panic!("not ready") })
                .await
                .unwrap_err(),
            GuiLaunchError::Spawn
        );
        std::fs::remove_file(spec.executable()).unwrap();
        std::fs::create_dir(spec.executable()).unwrap();
        assert_eq!(
            launch_with(&spec, pending(), |_, _| async { panic!("not ready") })
                .await
                .unwrap_err(),
            GuiLaunchError::Spawn
        );
        std::fs::remove_dir(spec.executable()).unwrap();
        std::os::unix::fs::symlink(directory.path().join("elsewhere"), spec.executable()).unwrap();
        assert_eq!(
            launch_with(&spec, pending(), |_, _| async { panic!("not ready") })
                .await
                .unwrap_err(),
            GuiLaunchError::Spawn
        );
    }

    #[tokio::test]
    async fn startup_without_readiness_is_bounded() {
        let (_directory, spec) = fixture("exec /bin/sleep 30", true);
        let result = tokio::time::timeout(
            Duration::from_secs(16),
            launch_with(&spec, pending(), |_, _| async { panic!("not ready") }),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), GuiLaunchError::StartupTimeout);
    }

    #[tokio::test]
    async fn cancellation_before_readiness_stops_the_spawned_process() {
        let (_directory, spec) = fixture("exec /bin/sleep 30", true);
        let result = tokio::time::timeout(
            Duration::from_secs(6),
            launch_with(
                &spec,
                async {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok(GuiSignal::Interrupt)
                },
                |_, _| async { panic!("not ready") },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, 130);
    }

    #[tokio::test]
    async fn ignored_interrupt_is_forced_and_waited_even_during_presentation() {
        use rigspark_runtime::process_control::{NativeProcessControl, ProcessControl};
        let (directory, spec) = fixture(
            "trap '' INT\nprintf '%s' \"$$\" > \"$0.pid\"\nprintf '%s\\n' '{\"url\":\"http://127.0.0.1:4000\",\"harness\":\"local\",\"port\":4000}'\nexec /bin/sleep 30",
            false,
        );
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let result = tokio::time::timeout(
            Duration::from_secs(8),
            launch_with(
                &spec,
                async {
                    receiver.await.unwrap();
                    Ok(GuiSignal::Interrupt)
                },
                |_, _| async move {
                    sender.send(()).unwrap();
                    pending().await
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result, 130);
        let pid = std::fs::read_to_string(directory.path().join("rigspark-gui.pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert!(!NativeProcessControl.alive(pid).await.unwrap());
    }

    #[tokio::test]
    async fn shutdown_monitor_failure_is_redacted_and_cleans_up() {
        let (_directory, spec) = fixture(
            "printf '%s\\n' '{\"url\":\"http://127.0.0.1:4000\",\"harness\":\"local\",\"port\":4000}'\nexec /bin/sleep 30",
            true,
        );
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let error = launch_with(
            &spec,
            async {
                receiver.await.unwrap();
                Err(std::io::Error::other("private shutdown details"))
            },
            |_, _| async move {
                sender.send(()).unwrap();
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error, GuiLaunchError::Signals);
        assert!(!error.to_string().contains("private"));
    }

    #[tokio::test]
    async fn presentation_timeout_is_bounded() {
        let (_directory, spec) = fixture(
            "printf '%s\\n' '{\"url\":\"http://127.0.0.1:4000\",\"harness\":\"local\",\"port\":4000}'\nexec /bin/sleep 30",
            false,
        );
        let error = tokio::time::timeout(
            Duration::from_secs(10),
            launch_with(&spec, pending(), |_, _| pending()),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error, GuiLaunchError::Presentation);
    }

    #[tokio::test]
    async fn drains_child_output_without_exposing_it_or_blocking_exit() {
        let (_directory, spec) = fixture(
            "printf '%s\\r\\n' '{\"url\":\"http://127.0.0.1:4000\",\"harness\":\"local\",\"port\":4000}'\n/bin/dd if=/dev/zero bs=4096 count=128 2>/dev/null\n/bin/dd if=/dev/zero bs=4096 count=128 1>&2 2>/dev/null\nexit 19",
            true,
        );
        let mut calls = 0;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            launch_with(&spec, pending(), |ready, open| {
                assert_eq!(ready.url(), "http://127.0.0.1:4000");
                assert!(!open);
                calls += 1;
                async { Ok(()) }
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(result, 19);
    }
}
