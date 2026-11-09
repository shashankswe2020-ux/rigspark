//! Managed engines: ComfyUI (image and video) and mlx-audio (voice on Apple Silicon).
//!
//! After one consent click RigSpark installs them into its own folder (`<home>/engines`) from
//! pinned sources, then starts, health-checks, reuses and stops them itself. Installs the user
//! already has win: a ComfyUI listening on 127.0.0.1:8188, an existing ComfyUI folder with its
//! own Python, or an `mlx_audio.server` on PATH. See docs/specs/managed-engines.md.
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// The installer that fetches Python and packages.
pub const UV_VERSION: &str = "0.13.7";
pub const PYTHON_VERSION: &str = "3.12";
/// Packages resolve as they were on this date, so every install gets the same versions.
pub const PACKAGES_AS_OF: &str = "2026-11-01T00:00:00Z";
pub const COMFYUI_VERSION: &str = "0.38.2";
pub const MLX_AUDIO_REQUIREMENT: &str = "mlx-audio==0.6.4";
/// Port a ComfyUI started by hand listens on.
pub const COMFYUI_DEFAULT_PORT: u16 = 8188;
/// The first start imports PyTorch, which can take minutes on a cold disk.
const COMFYUI_READY_TIMEOUT: Duration = Duration::from_secs(300);
const REDIRECT_LIMIT: usize = 5;

/// A file fetched over HTTPS and checked against a pinned SHA-256 before use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pinned {
    pub url: &'static str,
    pub sha256: &'static str,
    /// Larger downloads are refused.
    pub max_bytes: u64,
}

/// `uv`'s official release tarball for this platform and the folder inside it.
pub fn uv_pin(os: &str, arch: &str) -> Option<(Pinned, &'static str)> {
    const MAX: u64 = 64 * 1024 * 1024;
    let (folder, sha256) = match (os, arch) {
        ("macos", "aarch64") => (
            "uv-aarch64-apple-darwin",
            "81b811b57e4624df7ed3aac148e0359100d9962f3d9dce00aec533dc2e086731",
        ),
        ("macos", "x86_64") => (
            "uv-x86_64-apple-darwin",
            "20c47e4c1f630b8abaff8c291fa9b9aa300fef92c4fbff95adf198b55287cda7",
        ),
        ("linux", "x86_64") => (
            "uv-x86_64-unknown-linux-gnu",
            "c5221ff41b7892bfc223543883600125f6a47fe845093e0f5bdff4f359196d83",
        ),
        ("linux", "aarch64") => (
            "uv-aarch64-unknown-linux-gnu",
            "71279f8d3c67241e807b9d8db872d1bf541579fb9f08b7d1eb1682196d1e57dc",
        ),
        _ => return None,
    };
    let url = match folder {
        "uv-aarch64-apple-darwin" => {
            "https://github.com/astral-sh/uv/releases/download/0.13.7/uv-aarch64-apple-darwin.tar.gz"
        }
        "uv-x86_64-apple-darwin" => {
            "https://github.com/astral-sh/uv/releases/download/0.13.7/uv-x86_64-apple-darwin.tar.gz"
        }
        "uv-x86_64-unknown-linux-gnu" => {
            "https://github.com/astral-sh/uv/releases/download/0.13.7/uv-x86_64-unknown-linux-gnu.tar.gz"
        }
        _ => {
            "https://github.com/astral-sh/uv/releases/download/0.13.7/uv-aarch64-unknown-linux-gnu.tar.gz"
        }
    };
    Some((
        Pinned {
            url,
            sha256,
            max_bytes: MAX,
        },
        folder,
    ))
}

/// ComfyUI's upstream release source (GPL-3.0, unmodified) and the folder it unpacks to.
pub const COMFYUI_SOURCE: Pinned = Pinned {
    url: "https://codeload.github.com/comfyanonymous/ComfyUI/tar.gz/refs/tags/v0.38.2",
    sha256: "124fe87ed469ae582a08ee0bbc05761f0b1239c38e4a095bb564479e66ed671e",
    max_bytes: 64 * 1024 * 1024,
};
pub const COMFYUI_FOLDER: &str = "ComfyUI-0.38.2";

/// Hosts RigSpark itself downloads engine files from; every redirect is checked again.
const ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "codeload.github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

pub fn allowed_url(raw: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(raw).map_err(|_| "invalid download URL".to_owned())?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !ALLOWED_HOSTS.contains(&host)
    {
        return Err(format!("download host is not allowed: {host}"));
    }
    Ok(url)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    Comfyui,
    MlxAudio,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Engine::Comfyui, Engine::MlxAudio];
    pub fn name(self) -> &'static str {
        match self {
            Engine::Comfyui => "comfyui",
            Engine::MlxAudio => "mlx-audio",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|engine| engine.name() == value)
    }
    pub fn label(self) -> &'static str {
        match self {
            Engine::Comfyui => "Image and video (ComfyUI)",
            Engine::MlxAudio => "Voice (mlx-audio)",
        }
    }
    /// What the consent card says will be installed.
    pub fn installs(self) -> &'static str {
        match self {
            Engine::Comfyui => {
                "Python 3.12, PyTorch and ComfyUI 0.38.2, into RigSpark's own folder. Downloads several GB."
            }
            Engine::MlxAudio => {
                "Python 3.12 and mlx-audio 0.6.4, into RigSpark's own folder. Downloads about a GB."
            }
        }
    }
    fn steps(self) -> u32 {
        match self {
            Engine::Comfyui => 5,
            Engine::MlxAudio => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineState {
    Missing,
    Installing,
    Ready,
    Failed,
    Unsupported,
}

/// Where a ready engine comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Installed by RigSpark under `<home>/engines`.
    Managed,
    /// The user's own install.
    Existing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineView {
    pub engine: Engine,
    pub label: &'static str,
    pub installs: &'static str,
    pub state: EngineState,
    pub source: Option<Source>,
    pub detail: String,
    /// `(step, of)` while installing.
    pub step: Option<(u32, u32)>,
    /// Where a running ComfyUI answers.
    pub endpoint: Option<String>,
}

/// Network boundary: fetch `pinned` to `dest` only if its SHA-256 matches.
#[async_trait]
pub trait Fetch: Send + Sync {
    async fn fetch(
        &self,
        pinned: &Pinned,
        dest: &Path,
        cancel: &CancellationToken,
    ) -> Result<(), String>;
}

/// Process boundary for install steps (`tar`, `uv`): run to completion.
#[async_trait]
pub trait Run: Send + Sync {
    async fn run(
        &self,
        program: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        cancel: &CancellationToken,
    ) -> Result<(), String>;
}

pub struct HttpsFetch {
    client: reqwest::Client,
}

impl HttpsFetch {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .user_agent(concat!("rigspark/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(|error| error.to_string())?,
        })
    }
}

#[async_trait]
impl Fetch for HttpsFetch {
    async fn fetch(
        &self,
        pinned: &Pinned,
        dest: &Path,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let mut url = allowed_url(pinned.url)?;
        let mut response = None;
        for _ in 0..=REDIRECT_LIMIT {
            let reply = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err("cancelled".into()),
                reply = self.client.get(url.clone()).send() => reply.map_err(|error| format!("download failed: {error}"))?,
            };
            if reply.status().is_redirection() {
                let location = reply
                    .headers()
                    .get("location")
                    .and_then(|value| value.to_str().ok())
                    .ok_or("redirect without a location")?;
                url = allowed_url(
                    url.join(location)
                        .map_err(|_| "invalid redirect".to_owned())?
                        .as_str(),
                )?;
                continue;
            }
            response = Some(reply);
            break;
        }
        let response = response.ok_or("too many download redirects")?;
        if !response.status().is_success() {
            return Err(format!("download failed: HTTP {}", response.status()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > pinned.max_bytes)
        {
            return Err("download is larger than allowed".into());
        }
        let parent = dest.parent().ok_or("invalid download destination")?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| error.to_string())?;
        let partial = dest.with_extension("part");
        let mut file = tokio::fs::File::create(&partial)
            .await
            .map_err(|error| error.to_string())?;
        let mut hash = Sha256::new();
        let mut total = 0u64;
        let mut stream = response.bytes_stream();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = cancel.cancelled() => None,
                chunk = stream.next() => Some(chunk),
            };
            let Some(chunk) = chunk else {
                let _ = tokio::fs::remove_file(&partial).await;
                return Err("cancelled".into());
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(|error| format!("download interrupted: {error}"))?;
            total += chunk.len() as u64;
            if total > pinned.max_bytes {
                let _ = tokio::fs::remove_file(&partial).await;
                return Err("download is larger than allowed".into());
            }
            hash.update(&chunk);
            file.write_all(&chunk)
                .await
                .map_err(|error| error.to_string())?;
        }
        file.sync_all().await.map_err(|error| error.to_string())?;
        drop(file);
        verify_digest(&format!("{:x}", hash.finalize()), pinned.sha256).inspect_err(|_| {
            let _ = std::fs::remove_file(&partial);
        })?;
        tokio::fs::rename(&partial, dest)
            .await
            .map_err(|error| error.to_string())
    }
}

/// Fail closed: anything but the pinned digest is refused.
pub fn verify_digest(actual: &str, pinned: &str) -> Result<(), String> {
    if pinned.len() == 64 && actual.eq_ignore_ascii_case(pinned) {
        Ok(())
    } else {
        Err("download integrity mismatch; refusing to install".into())
    }
}

pub struct SystemRun;

#[async_trait]
impl Run for SystemRun {
    async fn run(
        &self,
        program: &Path,
        args: &[String],
        env: &BTreeMap<String, String>,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let mut command = tokio::process::Command::new(program);
        command
            .args(args)
            .envs(env)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let child = command
            .spawn()
            .map_err(|_| format!("could not run {}", display_name(program)))?;
        let output = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err("cancelled".into()),
            output = child.wait_with_output() => output.map_err(|error| error.to_string())?,
        };
        if output.status.success() {
            return Ok(());
        }
        // The last line of stderr is usually the reason (a resolver or network error).
        let reason = String::from_utf8_lossy(&output.stderr)
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .map(|line| rigspark_core::reports::strip_control(line.trim()))
            .unwrap_or_default();
        let reason: String = reason.chars().take(300).collect();
        Err(format!(
            "{} failed{}",
            display_name(program),
            if reason.is_empty() {
                String::new()
            } else {
                format!(": {reason}")
            }
        ))
    }
}

fn display_name(program: &Path) -> String {
    program
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "installer".into())
}

/// A ComfyUI install RigSpark can start: its folder and its own Python.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComfyInstall {
    pub dir: PathBuf,
    pub python: PathBuf,
    pub source: Source,
}

/// Where ComfyUI answers for a generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComfyEndpoint {
    pub dir: PathBuf,
    pub port: u16,
    /// RigSpark started it and stops it on shutdown.
    pub owned: bool,
}

/// The Python inside an existing ComfyUI folder, if it has its own environment.
pub fn comfy_python(dir: &Path) -> Option<PathBuf> {
    [
        ".venv/bin/python",
        "venv/bin/python",
        ".venv/Scripts/python.exe",
        "venv/Scripts/python.exe",
    ]
    .iter()
    .map(|candidate| dir.join(candidate))
    .find(|candidate| candidate.is_file())
}

#[derive(Debug, Clone, Default)]
struct Progress {
    installing: bool,
    failed: Option<String>,
    step: Option<(u32, u32)>,
    detail: String,
}

struct Owned {
    child: tokio::process::Child,
    endpoint: ComfyEndpoint,
}

pub struct Engines {
    root: PathBuf,
    fetch: Arc<dyn Fetch>,
    run: Arc<dyn Run>,
    platform: (&'static str, &'static str),
    /// The user's own ComfyUI folder (`RIGSPARK_COMFYUI_DIR` or `~/ComfyUI`).
    existing_comfyui: Option<PathBuf>,
    progress: Mutex<BTreeMap<Engine, Progress>>,
    comfy: tokio::sync::Mutex<Option<Owned>>,
    client: reqwest::Client,
    default_port: u16,
}

impl Engines {
    /// The real network, `tar` and `uv`, for `home`'s `engines` folder.
    pub fn new(home: &Path) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self::with(
            home.join("engines"),
            Arc::new(HttpsFetch::new()?),
            Arc::new(SystemRun),
            (std::env::consts::OS, std::env::consts::ARCH),
            crate::generation::default_comfyui_dir(),
            COMFYUI_DEFAULT_PORT,
        )))
    }

    /// Everything injected; for tests.
    pub fn with(
        root: PathBuf,
        fetch: Arc<dyn Fetch>,
        run: Arc<dyn Run>,
        platform: (&'static str, &'static str),
        existing_comfyui: Option<PathBuf>,
        default_port: u16,
    ) -> Self {
        Self {
            root,
            fetch,
            run,
            platform,
            existing_comfyui,
            progress: Mutex::new(BTreeMap::new()),
            comfy: tokio::sync::Mutex::new(None),
            client: reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap_or_default(),
            default_port,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn engine_dir(&self, engine: Engine) -> PathBuf {
        self.root.join(engine.name())
    }

    fn marker(&self, engine: Engine) -> PathBuf {
        self.engine_dir(engine).join("installed.json")
    }

    fn uv_binary(&self) -> PathBuf {
        let folder = uv_pin(self.platform.0, self.platform.1)
            .map(|(_, folder)| folder)
            .unwrap_or("uv");
        self.root
            .join("tools")
            .join(format!("uv-{UV_VERSION}"))
            .join(folder)
            .join(if self.platform.0 == "windows" {
                "uv.exe"
            } else {
                "uv"
            })
    }

    fn managed_comfyui_dir(&self) -> PathBuf {
        self.engine_dir(Engine::Comfyui).join(COMFYUI_FOLDER)
    }

    fn venv_python(venv: &Path) -> PathBuf {
        venv.join("bin").join("python")
    }

    /// Whether this machine can run `engine` from a managed install.
    pub fn supported(&self, engine: Engine) -> bool {
        match engine {
            Engine::Comfyui => uv_pin(self.platform.0, self.platform.1).is_some(),
            Engine::MlxAudio => self.platform == ("macos", "aarch64"),
        }
    }

    fn installed(&self, engine: Engine) -> bool {
        self.marker(engine).is_file()
            && match engine {
                Engine::Comfyui => comfy_python(&self.managed_comfyui_dir()).is_some(),
                Engine::MlxAudio => self.managed_mlx_audio_server().is_some(),
            }
    }

    /// RigSpark's own `mlx_audio.server`, once installed.
    pub fn managed_mlx_audio_server(&self) -> Option<PathBuf> {
        managed_mlx_audio_server_in(&self.root)
    }

    /// The ComfyUI to use: RigSpark's own first, then the user's folder if it has a Python.
    pub fn comfyui_install(&self) -> Option<ComfyInstall> {
        if self.installed(Engine::Comfyui) {
            let dir = self.managed_comfyui_dir();
            return comfy_python(&dir).map(|python| ComfyInstall {
                dir,
                python,
                source: Source::Managed,
            });
        }
        let dir = self.existing_comfyui.clone()?;
        comfy_python(&dir).map(|python| ComfyInstall {
            dir,
            python,
            source: Source::Existing,
        })
    }

    /// A ComfyUI folder for weights even when it can't be started by RigSpark (a user who
    /// runs ComfyUI themselves, perhaps with conda).
    pub fn comfyui_dir(&self) -> Option<PathBuf> {
        self.comfyui_install()
            .map(|install| install.dir)
            .or_else(|| self.existing_comfyui.clone())
    }

    pub fn status(&self, engine: Engine, external_mlx_audio: bool) -> EngineView {
        let progress = self
            .progress
            .lock()
            .ok()
            .and_then(|progress| progress.get(&engine).cloned())
            .unwrap_or_default();
        let (state, source) = if progress.installing {
            (EngineState::Installing, None)
        } else {
            match engine {
                Engine::Comfyui => match self.comfyui_install() {
                    Some(install) => (EngineState::Ready, Some(install.source)),
                    None if progress.failed.is_some() => (EngineState::Failed, None),
                    None if !self.supported(engine) => (EngineState::Unsupported, None),
                    None => (EngineState::Missing, None),
                },
                Engine::MlxAudio => {
                    if self.managed_mlx_audio_server().is_some() {
                        (EngineState::Ready, Some(Source::Managed))
                    } else if external_mlx_audio {
                        (EngineState::Ready, Some(Source::Existing))
                    } else if progress.failed.is_some() {
                        (EngineState::Failed, None)
                    } else if !self.supported(engine) {
                        (EngineState::Unsupported, None)
                    } else {
                        (EngineState::Missing, None)
                    }
                }
            }
        };
        let endpoint = match engine {
            Engine::Comfyui => self.comfy.try_lock().ok().and_then(|owned| {
                owned
                    .as_ref()
                    .map(|owned| format!("http://127.0.0.1:{}", owned.endpoint.port))
            }),
            Engine::MlxAudio => None,
        };
        let detail = match state {
            EngineState::Installing => progress.detail,
            EngineState::Failed => progress.failed.unwrap_or_default(),
            EngineState::Unsupported if engine == Engine::MlxAudio => {
                "mlx-audio runs on Apple Silicon only; attach a local speech server instead".into()
            }
            EngineState::Unsupported => {
                "Managed install isn't available on this platform yet; start ComfyUI yourself"
                    .into()
            }
            _ => String::new(),
        };
        EngineView {
            engine,
            label: engine.label(),
            installs: engine.installs(),
            state,
            source,
            detail,
            step: (state == EngineState::Installing)
                .then_some(progress.step)
                .flatten(),
            endpoint,
        }
    }

    fn report(&self, engine: Engine, step: u32, detail: &str) {
        if let Ok(mut progress) = self.progress.lock() {
            let entry = progress.entry(engine).or_default();
            entry.step = Some((step, engine.steps()));
            entry.detail = detail.to_owned();
        }
    }

    /// Starts installing `engine` in the background. Refuses a second install of the same
    /// engine while one runs.
    pub fn install(
        self: &Arc<Self>,
        engine: Engine,
        cancel: CancellationToken,
    ) -> Result<(), String> {
        if !self.supported(engine) {
            return Err(format!(
                "{} can't be installed on this machine",
                engine.label()
            ));
        }
        {
            let mut progress = self
                .progress
                .lock()
                .map_err(|_| "engine state unavailable")?;
            let entry = progress.entry(engine).or_default();
            if entry.installing {
                return Err(format!("{} is already being installed", engine.label()));
            }
            *entry = Progress {
                installing: true,
                detail: "Starting".into(),
                ..Progress::default()
            };
        }
        let engines = self.clone();
        tokio::spawn(async move {
            let outcome = engines.install_now(engine, &cancel).await;
            if let Ok(mut progress) = engines.progress.lock() {
                let entry = progress.entry(engine).or_default();
                entry.installing = false;
                entry.step = None;
                entry.failed = outcome.err();
            }
        });
        Ok(())
    }

    /// Runs every step; the installed marker is written only after all of them succeed.
    pub async fn install_now(
        &self,
        engine: Engine,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let _ = tokio::fs::remove_file(self.marker(engine)).await;
        let uv = self.ensure_uv(engine, cancel).await?;
        let env = self.uv_env();
        let dir = self.engine_dir(engine);
        match engine {
            Engine::Comfyui => {
                self.report(engine, 2, "Downloading ComfyUI 0.38.2");
                let archive = self
                    .root
                    .join("downloads")
                    .join(format!("comfyui-{COMFYUI_VERSION}.tar.gz"));
                self.fetch.fetch(&COMFYUI_SOURCE, &archive, cancel).await?;
                self.report(engine, 3, "Unpacking ComfyUI");
                tokio::fs::create_dir_all(&dir)
                    .await
                    .map_err(|error| error.to_string())?;
                let target = self.managed_comfyui_dir();
                if tokio::fs::try_exists(&target).await.unwrap_or(false) {
                    tokio::fs::remove_dir_all(&target)
                        .await
                        .map_err(|error| error.to_string())?;
                }
                self.run
                    .run(
                        Path::new("tar"),
                        &untar(&archive, &dir),
                        &BTreeMap::new(),
                        cancel,
                    )
                    .await?;
                if !target.join("main.py").is_file() {
                    return Err("ComfyUI archive did not unpack as expected".into());
                }
                self.report(engine, 4, "Installing Python 3.12");
                let venv = target.join(".venv");
                self.run.run(&uv, &venv_args(&venv), &env, cancel).await?;
                self.report(engine, 5, "Installing PyTorch and ComfyUI's packages");
                let mut args = pip_args(&venv);
                args.extend([
                    "-r".into(),
                    target.join("requirements.txt").display().to_string(),
                ]);
                self.run.run(&uv, &args, &env, cancel).await?;
                if !Self::venv_python(&venv).is_file() {
                    return Err("ComfyUI's Python environment is missing".into());
                }
            }
            Engine::MlxAudio => {
                self.report(engine, 2, "Installing Python 3.12");
                let venv = dir.join(".venv");
                self.run.run(&uv, &venv_args(&venv), &env, cancel).await?;
                self.report(engine, 3, "Installing mlx-audio 0.6.4");
                let mut args = pip_args(&venv);
                args.push(MLX_AUDIO_REQUIREMENT.into());
                self.run.run(&uv, &args, &env, cancel).await?;
                if !venv.join("bin/mlx_audio.server").is_file() {
                    return Err("mlx-audio installed without its server".into());
                }
            }
        }
        let marker = serde_json::json!({
            "engine": engine.name(),
            "uv": UV_VERSION,
            "python": PYTHON_VERSION,
            "packagesAsOf": PACKAGES_AS_OF,
            "version": match engine { Engine::Comfyui => COMFYUI_VERSION, Engine::MlxAudio => MLX_AUDIO_REQUIREMENT },
        });
        tokio::fs::write(self.marker(engine), marker.to_string())
            .await
            .map_err(|error| error.to_string())
    }

    async fn ensure_uv(
        &self,
        engine: Engine,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, String> {
        self.report(engine, 1, "Getting the installer (uv 0.13.7)");
        let binary = self.uv_binary();
        if binary.is_file() {
            return Ok(binary);
        }
        let (pin, _) =
            uv_pin(self.platform.0, self.platform.1).ok_or("no installer for this platform")?;
        let archive = self
            .root
            .join("downloads")
            .join(format!("uv-{UV_VERSION}.tar.gz"));
        self.fetch.fetch(&pin, &archive, cancel).await?;
        let tools = self.root.join("tools").join(format!("uv-{UV_VERSION}"));
        tokio::fs::create_dir_all(&tools)
            .await
            .map_err(|error| error.to_string())?;
        self.run
            .run(
                Path::new("tar"),
                &untar(&archive, &tools),
                &BTreeMap::new(),
                cancel,
            )
            .await?;
        if !binary.is_file() {
            return Err("installer archive did not unpack as expected".into());
        }
        Ok(binary)
    }

    /// Keeps `uv`'s cache and Python builds inside RigSpark's folder and ignores any user
    /// configuration that could point it elsewhere.
    fn uv_env(&self) -> BTreeMap<String, String> {
        [
            ("UV_NO_CONFIG", "1".to_owned()),
            (
                "UV_CACHE_DIR",
                self.root.join("cache").display().to_string(),
            ),
            (
                "UV_PYTHON_INSTALL_DIR",
                self.root.join("python").display().to_string(),
            ),
            ("UV_PYTHON_PREFERENCE", "only-managed".to_owned()),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect()
    }

    /// A ComfyUI to generate with, starting one if none answers. Reuses the one RigSpark
    /// started earlier while it is alive.
    pub async fn ensure_comfyui(
        &self,
        cancel: &CancellationToken,
    ) -> Result<ComfyEndpoint, String> {
        let mut owned = self.comfy.lock().await;
        if let Some(current) = owned.as_mut() {
            if matches!(current.child.try_wait(), Ok(None))
                && self.answers(current.endpoint.port).await
            {
                return Ok(current.endpoint.clone());
            }
            *owned = None;
        }
        if self.answers(self.default_port).await {
            let dir = self.comfyui_dir().ok_or(
                "ComfyUI is running, but its folder wasn't found; set RIGSPARK_COMFYUI_DIR",
            )?;
            return Ok(ComfyEndpoint {
                dir,
                port: self.default_port,
                owned: false,
            });
        }
        let install = self
            .comfyui_install()
            .ok_or("ComfyUI isn't set up yet: choose Set up in the composer to install it")?;
        let port = free_port()?;
        let mut command = tokio::process::Command::new(&install.python);
        command
            .arg(install.dir.join("main.py"))
            .args([
                "--listen",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--disable-auto-launch",
            ])
            .current_dir(&install.dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .map_err(|_| "ComfyUI failed to start".to_owned())?;
        let deadline = tokio::time::Instant::now() + COMFYUI_READY_TIMEOUT;
        loop {
            if cancel.is_cancelled() {
                let _ = child.start_kill();
                return Err("cancelled".into());
            }
            if !matches!(child.try_wait(), Ok(None)) {
                return Err(
                    "ComfyUI stopped while starting; open it once from Terminal to see why".into(),
                );
            }
            if self.answers(port).await {
                break;
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = child.start_kill();
                return Err("ComfyUI didn't become ready within 5 minutes".into());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let endpoint = ComfyEndpoint {
            dir: install.dir,
            port,
            owned: true,
        };
        *owned = Some(Owned {
            child,
            endpoint: endpoint.clone(),
        });
        Ok(endpoint)
    }

    async fn answers(&self, port: u16) -> bool {
        self.client
            .get(format!("http://127.0.0.1:{port}/system_stats"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }

    /// Stops the ComfyUI RigSpark started, if any.
    pub async fn shutdown(&self) {
        if let Some(mut owned) = self.comfy.lock().await.take() {
            let _ = owned.child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), owned.child.wait()).await;
        }
    }
}

/// RigSpark's own `mlx_audio.server` under an `engines` folder, once fully installed.
pub fn managed_mlx_audio_server_in(engines_root: &Path) -> Option<PathBuf> {
    let dir = engines_root.join(Engine::MlxAudio.name());
    let path = dir.join(".venv/bin/mlx_audio.server");
    (dir.join("installed.json").is_file() && path.is_file()).then_some(path)
}

fn untar(archive: &Path, into: &Path) -> Vec<String> {
    vec![
        "-xzf".into(),
        archive.display().to_string(),
        "-C".into(),
        into.display().to_string(),
    ]
}

fn venv_args(venv: &Path) -> Vec<String> {
    vec![
        "venv".into(),
        "--python".into(),
        PYTHON_VERSION.into(),
        "--clear".into(),
        venv.display().to_string(),
    ]
}

fn pip_args(venv: &Path) -> Vec<String> {
    vec![
        "pip".into(),
        "install".into(),
        "--python".into(),
        Engines::venv_python(venv).display().to_string(),
        "--exclude-newer".into(),
        PACKAGES_AS_OF.into(),
    ]
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|_| "no free local port".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_supported_platform_has_a_pinned_installer() {
        for (os, arch) in [
            ("macos", "aarch64"),
            ("macos", "x86_64"),
            ("linux", "x86_64"),
            ("linux", "aarch64"),
        ] {
            let (pin, folder) = uv_pin(os, arch).unwrap();
            assert_eq!(pin.sha256.len(), 64);
            assert!(pin.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(pin.url.ends_with(&format!("{folder}.tar.gz")));
            assert!(pin.url.contains(&format!("/download/{UV_VERSION}/")));
            allowed_url(pin.url).unwrap();
        }
        assert!(uv_pin("windows", "x86_64").is_none());
        assert!(uv_pin("freebsd", "x86_64").is_none());
        allowed_url(COMFYUI_SOURCE.url).unwrap();
        assert!(COMFYUI_SOURCE.url.ends_with(&format!("v{COMFYUI_VERSION}")));
        assert!(COMFYUI_FOLDER.ends_with(COMFYUI_VERSION));
    }

    #[test]
    fn only_https_from_github_hosts_is_downloaded() {
        assert!(allowed_url("https://github.com/astral-sh/uv/releases/download/x").is_ok());
        assert!(allowed_url("https://objects.githubusercontent.com/a").is_ok());
        for refused in [
            "http://github.com/a",
            "https://github.com:8443/a",
            "https://user@github.com/a",
            "https://evil.example/a",
            "https://github.com.evil.example/a",
            "file:///etc/passwd",
        ] {
            assert!(allowed_url(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn digests_fail_closed() {
        let pinned = "a".repeat(64);
        assert!(verify_digest(&pinned.to_uppercase(), &pinned).is_ok());
        assert!(verify_digest(&"b".repeat(64), &pinned).is_err());
        assert!(verify_digest("", "").is_err());
    }

    #[test]
    fn packages_resolve_as_of_a_fixed_date_inside_rigsparks_folder() {
        let args = pip_args(Path::new("/r/engines/mlx-audio/.venv"));
        assert_eq!(args[..2], ["pip".to_owned(), "install".to_owned()]);
        let at = args
            .iter()
            .position(|arg| arg == "--exclude-newer")
            .unwrap();
        assert_eq!(args[at + 1], PACKAGES_AS_OF);
        assert!(args.contains(&"/r/engines/mlx-audio/.venv/bin/python".to_owned()));
        assert!(venv_args(Path::new("/v")).contains(&PYTHON_VERSION.to_owned()));
    }
}
