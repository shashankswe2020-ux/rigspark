//! Managed engines with the network and install commands faked: nothing is downloaded and
//! no real ComfyUI, `tar` or `uv` runs.
use async_trait::async_trait;
use rigspark_runtime::engines::{
    COMFYUI_FOLDER, Engine, EngineState, Engines, Fetch, PACKAGES_AS_OF, Pinned, Run, Source,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct FakeFetch {
    urls: Mutex<Vec<String>>,
    refuse: bool,
}

#[async_trait]
impl Fetch for FakeFetch {
    async fn fetch(
        &self,
        pinned: &Pinned,
        dest: &Path,
        _: &CancellationToken,
    ) -> Result<(), String> {
        self.urls.lock().unwrap().push(pinned.url.to_owned());
        if self.refuse {
            return Err("download integrity mismatch; refusing to install".into());
        }
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(dest, b"archive").unwrap();
        Ok(())
    }
}

/// Plays `tar` and `uv`: creates what each would, and records every call.
type Call = (String, Vec<String>, BTreeMap<String, String>);

#[derive(Default)]
struct FakeRun {
    calls: Mutex<Vec<Call>>,
    fail_on: Option<&'static str>,
}

#[async_trait]
impl Run for FakeRun {
    async fn run(
        &self,
        program: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        _: &CancellationToken,
    ) -> Result<(), String> {
        let name = program.file_name().unwrap().to_string_lossy().into_owned();
        self.calls
            .lock()
            .unwrap()
            .push((name.clone(), args.to_vec(), env.clone()));
        if self
            .fail_on
            .is_some_and(|step| args.first().is_some_and(|first| first == step))
        {
            return Err(format!("{name} failed: resolver error"));
        }
        match (name.as_str(), args.first().map(String::as_str)) {
            ("tar", _) => {
                let into = PathBuf::from(&args[3]);
                if args[1].contains("uv-") {
                    let dir = into.join("uv-aarch64-apple-darwin");
                    std::fs::create_dir_all(&dir).unwrap();
                    std::fs::write(dir.join("uv"), b"").unwrap();
                } else {
                    let dir = into.join(COMFYUI_FOLDER);
                    std::fs::create_dir_all(&dir).unwrap();
                    std::fs::write(dir.join("main.py"), b"").unwrap();
                    std::fs::write(dir.join("requirements.txt"), b"torch\n").unwrap();
                }
            }
            ("uv", Some("venv")) => {
                let venv = PathBuf::from(args.last().unwrap());
                std::fs::create_dir_all(venv.join("bin")).unwrap();
                std::fs::write(venv.join("bin/python"), b"").unwrap();
            }
            ("uv", Some("pip")) if args.iter().any(|arg| arg.starts_with("mlx-audio==")) => {
                let at = args.iter().position(|arg| arg == "--python").unwrap();
                let bin = PathBuf::from(&args[at + 1]).parent().unwrap().to_path_buf();
                std::fs::write(bin.join("mlx_audio.server"), b"").unwrap();
            }
            _ => {}
        }
        Ok(())
    }
}

fn engines(
    root: &Path,
    fetch: Arc<FakeFetch>,
    run: Arc<FakeRun>,
    existing: Option<PathBuf>,
) -> Arc<Engines> {
    Arc::new(Engines::with(
        root.to_path_buf(),
        fetch,
        run,
        ("macos", "aarch64"),
        existing,
        1,
    ))
}

#[tokio::test]
async fn comfyui_installs_in_order_into_rigsparks_folder_then_marks_itself_ready() {
    let home = tempfile::tempdir().unwrap();
    let fetch = Arc::new(FakeFetch::default());
    let run = Arc::new(FakeRun::default());
    let engines = engines(home.path(), fetch.clone(), run.clone(), None);
    assert_eq!(
        engines.status(Engine::Comfyui, false).state,
        EngineState::Missing
    );

    engines
        .install_now(Engine::Comfyui, &CancellationToken::new())
        .await
        .unwrap();

    let urls = fetch.urls.lock().unwrap().clone();
    assert!(urls[0].contains("astral-sh/uv/releases/download/0.13.7/uv-aarch64-apple-darwin"));
    assert!(urls[1].contains("comfyanonymous/ComfyUI/tar.gz/refs/tags/v0.38.2"));
    let calls = run.calls.lock().unwrap().clone();
    let steps: Vec<_> = calls
        .iter()
        .map(|(program, args, _)| format!("{program} {}", args[0]))
        .collect();
    assert_eq!(steps, ["tar -xzf", "tar -xzf", "uv venv", "uv pip"]);
    let (_, pip, env) = &calls[3];
    assert!(
        pip.windows(2)
            .any(|pair| pair == ["--exclude-newer", PACKAGES_AS_OF])
    );
    assert!(pip.last().unwrap().ends_with("requirements.txt"));
    assert_eq!(env["UV_NO_CONFIG"], "1");
    assert!(env["UV_CACHE_DIR"].starts_with(&home.path().display().to_string()));
    assert!(env["UV_PYTHON_INSTALL_DIR"].starts_with(&home.path().display().to_string()));

    let view = engines.status(Engine::Comfyui, false);
    assert_eq!(view.state, EngineState::Ready);
    assert_eq!(view.source, Some(Source::Managed));
    let install = engines.comfyui_install().unwrap();
    assert_eq!(
        install.dir,
        home.path().join("comfyui").join(COMFYUI_FOLDER)
    );
    assert!(install.python.ends_with(".venv/bin/python"));
}

#[tokio::test]
async fn a_failed_step_leaves_nothing_marked_installed() {
    let home = tempfile::tempdir().unwrap();
    let run = Arc::new(FakeRun {
        fail_on: Some("pip"),
        ..FakeRun::default()
    });
    let engines = engines(home.path(), Arc::new(FakeFetch::default()), run, None);
    let error = engines
        .install_now(Engine::Comfyui, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.contains("resolver error"));
    assert!(!home.path().join("comfyui/installed.json").exists());
    assert!(engines.comfyui_install().is_none());
}

#[tokio::test]
async fn a_digest_mismatch_stops_before_anything_runs() {
    let home = tempfile::tempdir().unwrap();
    let fetch = Arc::new(FakeFetch {
        refuse: true,
        ..FakeFetch::default()
    });
    let run = Arc::new(FakeRun::default());
    let engines = engines(home.path(), fetch, run.clone(), None);
    let error = engines
        .install_now(Engine::MlxAudio, &CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.contains("integrity mismatch"));
    assert!(run.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn background_install_reports_progress_refuses_a_second_and_records_failure() {
    let home = tempfile::tempdir().unwrap();
    let run = Arc::new(FakeRun {
        fail_on: Some("venv"),
        ..FakeRun::default()
    });
    let engines = engines(home.path(), Arc::new(FakeFetch::default()), run, None);
    engines
        .install(Engine::MlxAudio, CancellationToken::new())
        .unwrap();
    let second = engines.install(Engine::MlxAudio, CancellationToken::new());
    let finished = async {
        loop {
            if engines.status(Engine::MlxAudio, false).state != EngineState::Installing {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), finished)
        .await
        .unwrap();
    assert!(second.is_err_and(|error| error.contains("already being installed")));
    let view = engines.status(Engine::MlxAudio, false);
    assert_eq!(view.state, EngineState::Failed);
    assert!(view.detail.contains("resolver error"));
}

#[tokio::test]
async fn mlx_audio_installs_its_server_and_only_on_apple_silicon() {
    let home = tempfile::tempdir().unwrap();
    let engines = engines(
        home.path(),
        Arc::new(FakeFetch::default()),
        Arc::new(FakeRun::default()),
        None,
    );
    engines
        .install_now(Engine::MlxAudio, &CancellationToken::new())
        .await
        .unwrap();
    assert!(
        engines
            .managed_mlx_audio_server()
            .unwrap()
            .ends_with(".venv/bin/mlx_audio.server")
    );
    assert_eq!(
        engines.status(Engine::MlxAudio, false).state,
        EngineState::Ready
    );

    let linux = Arc::new(Engines::with(
        home.path().join("other"),
        Arc::new(FakeFetch::default()),
        Arc::new(FakeRun::default()),
        ("linux", "x86_64"),
        None,
        1,
    ));
    assert_eq!(
        linux.status(Engine::MlxAudio, false).state,
        EngineState::Unsupported
    );
    assert!(
        linux
            .install(Engine::MlxAudio, CancellationToken::new())
            .is_err()
    );
    // An mlx_audio.server the user installed themselves still counts.
    assert_eq!(
        linux.status(Engine::MlxAudio, true).source,
        Some(Source::Existing)
    );
}

#[tokio::test]
async fn an_existing_comfyui_with_its_own_python_is_used_without_installing() {
    let home = tempfile::tempdir().unwrap();
    let existing = home.path().join("ComfyUI");
    std::fs::create_dir_all(existing.join(".venv/bin")).unwrap();
    std::fs::write(existing.join(".venv/bin/python"), b"").unwrap();
    let engines = engines(
        home.path(),
        Arc::new(FakeFetch::default()),
        Arc::new(FakeRun::default()),
        Some(existing.clone()),
    );
    let view = engines.status(Engine::Comfyui, false);
    assert_eq!(
        (view.state, view.source),
        (EngineState::Ready, Some(Source::Existing))
    );
    assert_eq!(engines.comfyui_install().unwrap().dir, existing);

    // Without a Python of its own RigSpark can't start it, so setup is offered.
    std::fs::remove_file(existing.join(".venv/bin/python")).unwrap();
    assert_eq!(
        engines.status(Engine::Comfyui, false).state,
        EngineState::Missing
    );
    assert_eq!(engines.comfyui_dir(), Some(existing));
}

/// A stand-in for ComfyUI's Python: serves /system_stats on the --port it is given.
#[cfg(unix)]
fn fake_comfyui(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir.join(".venv/bin")).unwrap();
    std::fs::write(dir.join("main.py"), b"").unwrap();
    let python = dir.join(".venv/bin/python");
    std::fs::write(
        &python,
        r#"#!/bin/sh
exec python3 -c '
import sys, http.server
port = int(sys.argv[sys.argv.index("--port") + 1])
assert sys.argv[sys.argv.index("--listen") + 1] == "127.0.0.1"
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if self.path == "/system_stats" else 404)
        self.end_headers()
        self.wfile.write(b"{}")
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", port), H).serve_forever()
' "$@"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn comfyui_is_started_on_loopback_reused_and_stopped() {
    let home = tempfile::tempdir().unwrap();
    let existing = home.path().join("ComfyUI");
    fake_comfyui(&existing);
    // Port 1 never answers, so nothing "already running" is picked up.
    let engines = engines(
        home.path(),
        Arc::new(FakeFetch::default()),
        Arc::new(FakeRun::default()),
        Some(existing.clone()),
    );
    let cancel = CancellationToken::new();
    let first = engines.ensure_comfyui(&cancel).await.unwrap();
    assert!(first.owned);
    assert_eq!(first.dir, existing);
    let stats = reqwest::get(format!("http://127.0.0.1:{}/system_stats", first.port))
        .await
        .unwrap();
    assert!(stats.status().is_success());
    let view = engines.status(Engine::Comfyui, false);
    assert_eq!(
        view.endpoint,
        Some(format!("http://127.0.0.1:{}", first.port))
    );

    let again = engines.ensure_comfyui(&cancel).await.unwrap();
    assert_eq!(again.port, first.port, "the running ComfyUI is reused");

    engines.shutdown().await;
    assert!(
        reqwest::get(format!("http://127.0.0.1:{}/system_stats", first.port))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn nothing_to_start_says_how_to_set_it_up() {
    let home = tempfile::tempdir().unwrap();
    let engines = engines(
        home.path(),
        Arc::new(FakeFetch::default()),
        Arc::new(FakeRun::default()),
        None,
    );
    let error = engines
        .ensure_comfyui(&CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.contains("Set up"));
}
