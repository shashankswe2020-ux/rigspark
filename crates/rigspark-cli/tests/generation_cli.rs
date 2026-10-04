use std::process::{Command, Output, Stdio};

const APPLE_16: &str = r#"{"arch":"arm64","platform":"darwin","totalRamBytes":17179869184,"freeRamBytes":8589934592,"freeDiskBytes":536870912000,"gpu":[{"vendor":"apple","vramBytes":0}]}"#;

fn invoke(args: &[&str]) -> Output {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let output = Command::new(env!("CARGO_BIN_EXE_llmup-native"))
        .args(args)
        .current_dir(directory.path())
        .env("RIGSPARK_HOME", &home)
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .env("PATH", "")
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .env_remove("RIGSPARK_TUI")
        .env_remove("RIGSPARK_COMFYUI_DIR")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!home.exists(), "unexpected state mutation: {args:?}");
    let leftovers: Vec<_> = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(leftovers.is_empty(), "unexpected files {leftovers:?}");
    output
}
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn catalog_lists_generation_models_with_memory_only_verdicts() {
    let output = invoke(&["catalog", "--generation", "--hardware-json", APPLE_16]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert!(lines[0].starts_with("Generation catalog (runtime: ComfyUI, local only"));
    let row = |id: &str| *lines.iter().find(|line| line.starts_with(id)).unwrap();
    let flux = row("flux1-schnell:fp8");
    assert!(
        flux.contains("image") && flux.contains(" no ") && flux.contains("unknown"),
        "{flux}"
    );
    let wan = row("wan2.1-t2v:1.3b");
    assert!(
        wan.contains("video") && wan.contains(" yes ") && wan.contains("unknown"),
        "{wan}"
    );
    assert!(text.contains("generation speed is unknown"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn generate_help_documents_the_local_contract() {
    let output = invoke(&["generate", "--help"]);
    assert_eq!(output.status.code(), Some(0));
    let text = String::from_utf8(output.stdout).unwrap();
    for needle in [
        "Generate an image or video locally through ComfyUI",
        "--prompt",
        "--output",
        "--seed",
        "--comfyui-dir",
        "Loopback ComfyUI port (default: 8188)",
        "image, video, or a model id",
    ] {
        assert!(text.contains(needle), "missing {needle}");
    }
    let root = String::from_utf8(invoke(&["--help"]).stdout).unwrap();
    assert!(root.contains("generate"));
}

#[test]
fn invalid_generation_invocations_fail_before_network_or_state() {
    let missing_models = tempfile::tempdir().unwrap();
    let missing = missing_models.path().to_str().unwrap();
    for (args, needle) in [
        (vec!["generate", "--prompt", "fox"], "image, video"),
        (vec!["generate", "image"], "--prompt is required"),
        (vec!["generate", "image", "--prompt", " "], "prompt must be"),
        (
            vec!["generate", "image", "--prompt", "fox"],
            "RIGSPARK_COMFYUI_DIR",
        ),
        (
            vec![
                "generate",
                "audio",
                "--prompt",
                "fox",
                "--comfyui-dir",
                missing,
            ],
            "unknown generation model",
        ),
        (
            vec![
                "generate",
                "image",
                "--prompt",
                "fox",
                "--seed",
                "9007199254740992",
            ],
            "invalid --seed",
        ),
        (
            vec![
                "generate",
                "image",
                "--prompt",
                "fox",
                "--comfyui-dir",
                missing,
            ],
            "models/ folder",
        ),
        (
            vec!["generate", "image", "--prompt", "fox", "--port", "70000"],
            "invalid --port",
        ),
        (
            vec!["generate", "image", "--prompt", "fox", "--context", "1"],
            "not supported",
        ),
        (vec!["catalog", "--generation", "--json"], "not supported"),
        (
            vec!["catalog", "--generation", "--all"],
            "only combines with --hardware",
        ),
        (
            vec!["catalog", "--generation", "--status"],
            "--update and --status",
        ),
        (vec!["recommend", "--generation"], "not supported"),
        (vec!["ls", "--prompt", "fox"], "not supported"),
        (
            vec!["up", "qwen3:8b", "--comfyui-dir", missing],
            "not supported",
        ),
    ] {
        let output = invoke(&args);
        assert_ne!(output.status.code(), Some(0), "{args:?}");
        assert!(
            stderr(&output).contains(needle),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn unreachable_comfyui_downloads_nothing() {
    let comfy = tempfile::tempdir().unwrap();
    std::fs::create_dir(comfy.path().join("models")).unwrap();
    let output = invoke(&[
        "generate",
        "video",
        "--prompt",
        "a fox",
        "--port",
        "1",
        "--bypass",
        "--comfyui-dir",
        comfy.path().to_str().unwrap(),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("ComfyUI is not reachable at http://127.0.0.1:1"));
    assert_eq!(
        std::fs::read_dir(comfy.path().join("models"))
            .unwrap()
            .count(),
        0
    );
}
