use crate::{
    acquire::{Acquisition, HfTransport},
    adapters::{BackendAdapter, BackendError, BackendKind, RuntimeAdapter, ServeRequest},
    command::{CommandRunner, NativeCommandRunner},
    http::{HttpError, NativeTransport, Request, Response, Transport},
    identity::{NativeProcessProbe, ProcessIdentity},
    lifecycle::{Activation, Lifecycle, Registry},
    ollama_installed::{InstalledModels, LocalVerifier, verify_catalog_manifest},
    process_control::{
        NativeProcessControl, ProcessControl, listener, resolve_binary, same_process,
    },
    pull::{PullRequest, PullService},
    special_adapters::{LmStudioAdapter, MlxAdapter},
    state::{Config, ServerState, StateStore},
};
use rigspark_core::{
    catalog::{Catalog, CatalogModel, resolve},
    reports::strip_control,
    sizing::{Hardware, SizingRequest, evaluate},
};
use serde_json::{Value, json};
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;

pub mod events;
use events::{
    LifecycleObserver, LifecycleScope, LifecycleStage, LifecycleWarning, ObservedAdapter,
    download_progress, observe, warning,
};

pub struct LifecycleOptions {
    pub command: String,
    pub model: Option<String>,
    pub backend: Option<String>,
    pub port: Option<u16>,
    pub context: Option<u32>,
    pub installed: bool,
    pub bypass: bool,
    pub cache: crate::cache::CacheFlags,
}
impl LifecycleOptions {
    pub fn validate(&self) -> Result<(), BackendError> {
        if !self.cache.is_empty() {
            if !["up", "switch"].contains(&self.command.as_str()) {
                return Err(BackendError(format!(
                    "{} does not accept cache options",
                    self.command
                )));
            }
            if self.installed {
                return Err(BackendError(
                    "cache options apply only to a runtime rigspark starts; --installed attaches to a running Ollama".into(),
                ));
            }
            if let Some(profile) = self.cache.over(None) {
                profile
                    .validate()
                    .map_err(|error| BackendError(error.to_string()))?;
            }
        }
        if !["up", "switch", "down", "doctor"].contains(&self.command.as_str())
            || self.port == Some(0)
            || self
                .context
                .is_some_and(|value| !(1..=10000000).contains(&value))
            || self
                .backend
                .as_ref()
                .is_some_and(|value| !rigspark_core::catalog::BACKENDS.contains(&value.as_str()))
        {
            return Err(BackendError("invalid lifecycle options".into()));
        }
        if self.command == "down" || self.command == "doctor" {
            if (self.command == "doctor" && self.model.is_some())
                || self.backend.is_some()
                || self.port.is_some()
                || self.context.is_some()
                || self.installed
                || self.bypass
            {
                return Err(BackendError(format!(
                    "{} does not accept selection options",
                    self.command
                )));
            }
        } else if self.model.is_none() {
            return Err(BackendError("model is required".into()));
        }
        if let Some(query) = self.model.as_deref() {
            if query.trim().is_empty() || query.len() > 8192 || query.chars().any(char::is_control)
            {
                return Err(BackendError("invalid model reference".into()));
            }
            if self.installed {
                crate::adapters::model_id(query)?;
            }
        }
        if self.installed
            && (!self.bypass
                || self
                    .backend
                    .as_ref()
                    .is_some_and(|backend| backend != "ollama"))
        {
            return Err(BackendError(
                "installed models require --bypass and Ollama".into(),
            ));
        }
        Ok(())
    }
}
/// Rejects advisory selections before callers probe hardware, confirm activation, or access state.
pub fn check_selection_availability(
    options: &LifecycleOptions,
    catalog: &Catalog,
) -> Result<(), BackendError> {
    options.validate()?;
    if !["up", "switch"].contains(&options.command.as_str()) {
        return Ok(());
    }
    let Some(query) = options.model.as_deref() else {
        return Ok(());
    };
    if let Ok(resolved) = resolve(catalog, query) {
        resolved
            .model
            .ensure_runnable()
            .map_err(|error| BackendError(error.to_string()))?;
    }
    // Installed/bypass selection can use the Ollama reference instead of the catalog ID.
    let normalized = query.trim().to_ascii_lowercase();
    for model in &catalog.models {
        if let Some(reference) = &model.source.ollama
            && rigspark_core::registry_collector::parse_reference(&reference.to_ascii_lowercase())
                == rigspark_core::registry_collector::parse_reference(&normalized)
        {
            model
                .ensure_runnable()
                .map_err(|error| BackendError(error.to_string()))?;
        }
    }
    Ok(())
}

struct CheckedHttp<'runtime> {
    inner: &'runtime dyn Transport,
    probe: &'runtime NativeProcessProbe,
    expected: ProcessIdentity,
}
#[async_trait::async_trait]
impl Transport for CheckedHttp<'_> {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        let observed = listener(self.probe, request.url.as_str(), &CancellationToken::new())
            .await
            .map_err(|_| HttpError::Invalid)?;
        if !same_process(&observed.identity, &self.expected) {
            return Err(HttpError::Invalid);
        }
        self.inner.send(request).await
    }
}
struct ContextActivation<'runtime> {
    source: String,
    context: Option<u32>,
    installed: bool,
    expected_manifest: Option<String>,
    expected_sha: Option<String>,
    expected_bytes: Option<u64>,
    projectors: Vec<rigspark_core::sizing::ProjectorArtifact>,
    root: PathBuf,
    http: &'runtime NativeTransport,
    probe: &'runtime NativeProcessProbe,
    observer: Option<&'runtime LifecycleObserver>,
}
#[async_trait::async_trait]
impl Activation for ContextActivation<'_> {
    async fn finalize(
        &self,
        handle: &ServerState,
        cancel: &CancellationToken,
    ) -> Result<ServerState, BackendError> {
        let observed = listener(self.probe, &handle.endpoint, cancel).await?;
        if handle.pid != Some(observed.identity.pid)
            || handle.process_executable.as_deref() != Some(&observed.identity.executable)
            || handle.process_started_at.as_deref() != Some(&observed.identity.started)
        {
            return Err(BackendError(
                "Ollama listener changed before activation".into(),
            ));
        }
        let http = CheckedHttp {
            inner: self.http,
            probe: self.probe,
            expected: observed.identity,
        };
        let verifier = LocalVerifier {
            root: self.root.clone(),
        };
        let support = InstalledModels::new(&http, &verifier);
        let model = support
            .inspect(&handle.endpoint, &self.source, cancel)
            .await
            .map_err(|error| BackendError(error.to_string()))?;
        if self
            .expected_manifest
            .as_ref()
            .is_some_and(|digest| digest != &model.digest)
        {
            return Err(BackendError("installed source changed; retry".into()));
        }
        observe(
            self.observer,
            LifecycleScope::Runtime,
            LifecycleStage::Verification,
            async {
                verify_catalog_manifest(
                    &self.root,
                    &model.id,
                    &model.digest,
                    self.expected_sha.as_deref(),
                    self.expected_bytes,
                    &self.projectors,
                    cancel,
                )
                .await
                .map_err(|error| BackendError(error.to_string()))
            },
        )
        .await?;
        let runtime = observe(
            self.observer,
            LifecycleScope::Runtime,
            LifecycleStage::Activation,
            async {
                support
                    .activate(&handle.endpoint, &model, self.context, cancel)
                    .await
                    .map_err(|error| BackendError(error.to_string()))
            },
        )
        .await?;
        let final_model = support
            .inspect(&handle.endpoint, &self.source, cancel)
            .await
            .map_err(|error| BackendError(error.to_string()))?;
        if final_model.digest != model.digest {
            return Err(BackendError(
                "installed source changed before commit".into(),
            ));
        }
        let mut active = handle.clone();
        active.runtime_model_id = Some(runtime);
        active.context = self.context;
        if self.installed {
            active.integrity = Some("local-manifest".into());
            active.local_manifest_digest = Some(model.digest);
        }
        Ok(active)
    }
}
fn compatible(model: &CatalogModel, backend: &str) -> bool {
    if model.is_advisory_only() {
        return false;
    }
    match backend {
        "ollama" => model.source.ollama.is_some(),
        "llamacpp" => model.source.gguf.is_some(),
        "mlx" => model.source.mlx.is_some(),
        "lmstudio" => model.source.gguf.is_some() || model.source.mlx.is_some(),
        _ => false,
    }
}
fn backend_known(backend: &str) -> bool {
    rigspark_core::catalog::BACKENDS.contains(&backend)
}
/// Index into the resolved runtime binaries: ollama, llama-server, python3, lms.
fn backend_index(backend: &str) -> usize {
    match backend {
        "llamacpp" => 1,
        "mlx" => 2,
        "lmstudio" => 3,
        _ => 0,
    }
}
/// Attach-intent commands follow the active backend; a conflicting override must fail, not be ignored.
pub fn check_attach_override(
    command: &str,
    active: &str,
    flag: Option<&str>,
    env: Option<&str>,
) -> Result<(), BackendError> {
    if let Some(backend) = flag
        && backend != active
    {
        return Err(BackendError(format!(
            "{command} cannot change the active backend from {active} to {backend}; use up --backend {backend}"
        )));
    }
    if let Some(backend) = env.map(str::trim).filter(|value| !value.is_empty())
        && backend != active
    {
        return Err(BackendError(format!(
            "RIGSPARK_BACKEND={} conflicts with the active {active} server; unset it or use up",
            strip_control(backend)
        )));
    }
    Ok(())
}
/// Flag, then non-blank `RIGSPARK_BACKEND`, then the user config default.
pub fn preferred_backend(
    flag: Option<&str>,
    env: Option<&str>,
    user: Option<&str>,
) -> Option<String> {
    flag.or(env.filter(|value| !value.trim().is_empty()))
        .or(user)
        .map(str::to_owned)
}
/// Backends compatible with the model, in auto-selection order; MLX leads only on Apple Silicon.
/// Attach-only LM Studio is never auto-selected (spec Q1).
pub fn auto_backend_order(model: &CatalogModel, hardware: &Hardware) -> Vec<&'static str> {
    let apple = hardware.platform == rigspark_core::sizing::Platform::Darwin
        && hardware.arch == rigspark_core::sizing::CpuArch::Arm64;
    let order: &[&str] = if apple {
        &["mlx", "ollama", "llamacpp"]
    } else {
        &["ollama", "llamacpp"]
    };
    order
        .iter()
        .copied()
        .filter(|name| compatible(model, name))
        .collect()
}
pub fn check_backend(
    model: &CatalogModel,
    backend: &str,
    hardware: &Hardware,
    installed: bool,
) -> Result<(), BackendError> {
    model
        .ensure_runnable()
        .map_err(|error| BackendError(error.to_string()))?;
    if !rigspark_core::catalog::BACKENDS.contains(&backend) {
        return Err(BackendError("invalid backend selection".into()));
    }
    validate_backend_platform(backend, hardware.platform.clone(), hardware.arch.clone())?;
    if backend == "lmstudio" && model.source.gguf.is_none() && model.source.mlx.is_some() {
        validate_backend_platform("mlx", hardware.platform.clone(), hardware.arch.clone())?;
    }
    if !installed || !compatible(model, backend) {
        return Err(BackendError(
            "backend is unavailable or model source is unsupported".into(),
        ));
    }
    Ok(())
}
#[derive(Debug, Clone, Copy)]
pub struct PlanOptions {
    pub simple_switch: bool,
    pub bypass: bool,
    pub context: Option<u32>,
    /// Size the KV cache at this type; `None` keeps the conservative f16 estimate.
    pub kv_cache: Option<rigspark_core::sizing::KvCacheType>,
}
#[derive(Debug)]
pub struct QuantPlan {
    pub quant: rigspark_core::sizing::Quantization,
    pub estimated_fit: bool,
}
/// Picks the quantization to pull and enforces fit, bypass and disk preflight before any side effect.
pub fn plan_quantization(
    model: &CatalogModel,
    explicit: Option<&rigspark_core::sizing::Quantization>,
    backend: &str,
    hardware: &Hardware,
    options: PlanOptions,
) -> Result<QuantPlan, BackendError> {
    model
        .ensure_runnable()
        .map_err(|error| BackendError(error.to_string()))?;
    if options.simple_switch && backend != "ollama" {
        return Err(BackendError(
            "single-model and delegated runtimes require up to replace models".into(),
        ));
    }
    if options.context.is_some() && backend != "ollama" {
        return Err(BackendError(
            "explicit runtime context currently requires Ollama".into(),
        ));
    }
    let mut sizing = match options.kv_cache {
        Some(kind) => model
            .with_kv_cache(kind)
            .map_err(|error| BackendError(error.to_string()))?
            .sizing(),
        None => model.sizing(),
    };
    if let Some(quant) = explicit {
        sizing.quantizations = vec![quant.clone()];
    }
    let fit = evaluate(&SizingRequest {
        model: sizing,
        hardware: hardware.clone(),
        context: options.context.map(f64::from),
    })
    .map_err(|error| BackendError(error.to_string()))?;
    if !options.simple_switch
        && explicit.is_none()
        && backend != "lmstudio"
        && fit.fit.reason == Some("disk-bound")
    {
        return Err(BackendError("insufficient disk space".into()));
    }
    let quant = if options.simple_switch {
        explicit.or_else(|| model.quantizations.first()).cloned()
    } else {
        explicit.cloned().or(fit.fit.quant.clone()).or_else(|| {
            if options.bypass {
                model
                    .quantizations
                    .iter()
                    .min_by(|left, right| left.disk_bytes.total_cmp(&right.disk_bytes))
                    .cloned()
            } else {
                None
            }
        })
    }
    .ok_or_else(|| {
        BackendError("model does not fit; use --bypass to override estimated fit".into())
    })?;
    if !options.simple_switch
        && backend != "lmstudio"
        && quant.disk_bytes > hardware.free_disk_bytes
    {
        return Err(BackendError("insufficient disk space".into()));
    }
    if !options.simple_switch
        && !fit.fit.fits
        && !options.bypass
        && (options.context.is_some() || explicit.is_none())
    {
        return Err(BackendError(
            "model does not fit requested context; use --bypass".into(),
        ));
    }
    Ok(QuantPlan {
        quant,
        estimated_fit: !options.simple_switch && !fit.fit.fits,
    })
}
fn catalog_pull_request(
    model: &CatalogModel,
    quant: &rigspark_core::sizing::Quantization,
    backend: &str,
) -> Result<PullRequest, BackendError> {
    Ok(PullRequest {
        backend: backend.into(),
        model_id: if backend == "ollama" {
            model
                .source
                .ollama
                .clone()
                .ok_or_else(|| BackendError("missing Ollama source".into()))?
        } else {
            model.id.clone()
        },
        expected_bytes: quant.disk_bytes as u64,
        expected_sha256: quant.sha256.clone(),
        projectors: quant.projectors.clone(),
        gguf: model.source.gguf.clone(),
        mlx: model.source.mlx.clone(),
    })
}
pub fn validate_backend_platform(
    backend: &str,
    platform: rigspark_core::sizing::Platform,
    arch: rigspark_core::sizing::CpuArch,
) -> Result<(), BackendError> {
    if backend == "mlx"
        && !(platform == rigspark_core::sizing::Platform::Darwin
            && arch == rigspark_core::sizing::CpuArch::Arm64)
    {
        return Err(BackendError(
            "MLX requires Apple Silicon (darwin/arm64)".into(),
        ));
    }
    Ok(())
}
pub(crate) fn studio_trust(home: &std::path::Path) -> Vec<PathBuf> {
    let paths = if cfg!(target_os = "macos") {
        vec![
            PathBuf::from("/Applications/LM Studio.app/Contents/MacOS/LM Studio"),
            home.join("Applications/LM Studio.app/Contents/MacOS/LM Studio"),
        ]
    } else if cfg!(windows) {
        let mut paths = vec![
            PathBuf::from("C:\\Program Files\\LM Studio\\LM Studio.exe"),
            PathBuf::from("C:\\Program Files\\LM Studio\\llmster.exe"),
        ];
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            paths.extend([
                PathBuf::from(&local).join("Programs/LM Studio/LM Studio.exe"),
                PathBuf::from(local).join("Programs/LM Studio/llmster.exe"),
            ]);
        }
        paths
    } else {
        vec![
            PathBuf::from("/usr/bin/llmster"),
            PathBuf::from("/usr/local/bin/llmster"),
            PathBuf::from("/opt/lm-studio/bin/llmster"),
        ]
    };
    paths
        .into_iter()
        .map(|path| path.canonicalize().unwrap_or(path))
        .collect()
}
pub async fn run_native(
    options: &LifecycleOptions,
    catalog: &Catalog,
    hardware: Option<&Hardware>,
    cancel: &CancellationToken,
) -> Result<(Value, String), BackendError> {
    run_native_observed(options, catalog, hardware, cancel, None).await
}
pub async fn run_native_observed(
    options: &LifecycleOptions,
    catalog: &Catalog,
    hardware: Option<&Hardware>,
    cancel: &CancellationToken,
    observer: Option<&LifecycleObserver>,
) -> Result<(Value, String), BackendError> {
    options.validate()?;
    check_selection_availability(options, catalog)?;
    let config = Config::load().map_err(|error| BackendError(error.to_string()))?;
    run_native_with_config_observed(options, catalog, hardware, cancel, config, observer).await
}
/// `down --forget`: clears a stale attached pointer without probing or signalling any process.
pub async fn forget_attached_with_config(
    config: Config,
    cancel: &CancellationToken,
) -> Result<(Value, String), BackendError> {
    let store = StateStore::new(config);
    let registry = Registry::new(Vec::new());
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &NativeProcessProbe,
    };
    Ok(match lifecycle.forget_attached(cancel).await? {
        None => (
            json!({"type":"no-active"}),
            "No active server to forget.\n".into(),
        ),
        Some(active) => (
            json!({"type":"forgotten","modelId":active.model_id,"endpoint":active.endpoint}),
            format!(
                "Forgot {} ({}); no process was signalled.\n",
                strip_control(&active.model_id),
                active.endpoint
            ),
        ),
    })
}
pub async fn run_native_with_config(
    options: &LifecycleOptions,
    catalog: &Catalog,
    hardware: Option<&Hardware>,
    cancel: &CancellationToken,
    config: Config,
) -> Result<(Value, String), BackendError> {
    run_native_with_config_observed(options, catalog, hardware, cancel, config, None).await
}
pub async fn run_native_with_config_observed(
    options: &LifecycleOptions,
    catalog: &Catalog,
    hardware: Option<&Hardware>,
    cancel: &CancellationToken,
    config: Config,
    observer: Option<&LifecycleObserver>,
) -> Result<(Value, String), BackendError> {
    options.validate()?;
    check_selection_availability(options, catalog)?;
    let observer = observer.filter(|_| options.command != "doctor");
    let store = StateStore::new(config.clone());
    let prior = store
        .read()
        .map_err(|error| BackendError(error.to_string()))?;
    if options.command == "switch" {
        let active = prior
            .active
            .as_ref()
            .ok_or_else(|| BackendError("no active server to switch; run up first".into()))?;
        check_attach_override(
            "switch",
            &active.backend,
            options.backend.as_deref(),
            std::env::var("RIGSPARK_BACKEND").ok().as_deref(),
        )?;
        if options.context.is_none()
            && !options.bypass
            && let Some(port) = options.port
            && port != active.port
        {
            return Err(BackendError(format!(
                "switch without --context or --bypass cannot change the active port; use up --port {port}"
            )));
        }
    }
    if options.command == "down"
        && let Some(active) = &prior.active
    {
        check_attach_override(
            "down",
            &active.backend,
            None,
            std::env::var("RIGSPARK_BACKEND").ok().as_deref(),
        )?;
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| BackendError("home unavailable".into()))?;
    let paths = ["ollama", "llama-server", "python3", "lms"].map(|name| resolve_binary(name).ok());
    let binary = |index: usize| {
        paths[index]
            .clone()
            .unwrap_or_else(|| config.home.join(".unavailable").join(index.to_string()))
    };
    let http = NativeTransport::new().map_err(|error| BackendError(error.to_string()))?;
    let probe = NativeProcessProbe;
    let control = NativeProcessControl;
    let commands = NativeCommandRunner;
    let ollama_root = std::env::var_os("OLLAMA_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".ollama/models"));
    let ollama = RuntimeAdapter::new(BackendKind::Ollama, binary(0), &http, &probe, &control)
        .with_ollama_models(ollama_root.clone())?;
    let llama = RuntimeAdapter::new(BackendKind::LlamaCpp, binary(1), &http, &probe, &control);
    let mlx = MlxAdapter {
        binary: binary(2),
        http: &http,
        probe: &probe,
        control: &control,
        commands: &commands,
        token: None,
    };
    let studio = LmStudioAdapter {
        binary: binary(3),
        trusted_executables: studio_trust(&home),
        http: &http,
        probe: &probe,
        commands: &commands,
        token: std::env::var("LM_API_TOKEN").ok(),
    };
    let acquisition_daemon =
        ObservedAdapter::new(&ollama, observer, LifecycleScope::AcquisitionDaemon);
    let ollama = ObservedAdapter::new(&ollama, observer, LifecycleScope::Runtime);
    let llama = ObservedAdapter::new(&llama, observer, LifecycleScope::Runtime);
    let mlx = ObservedAdapter::new(&mlx, observer, LifecycleScope::Runtime);
    let studio = ObservedAdapter::new(&studio, observer, LifecycleScope::Runtime);
    let registry = Registry::new(vec![&ollama, &llama, &mlx, &studio]);
    let lifecycle = Lifecycle {
        store: &store,
        registry: &registry,
        probe: &probe,
    };
    if options.command == "doctor" {
        let Some(active) = prior.active else {
            return Ok((
                json!({"name":"state","status":"ok","detail":"no active server recorded"}),
                String::new(),
            ));
        };
        let model = catalog
            .models
            .iter()
            .find(|model| model.id == active.model_id);
        let path = if active.backend == "mlx" {
            let source = model
                .and_then(|model| model.source.mlx.as_ref())
                .ok_or_else(|| BackendError("active MLX source unavailable".into()))?;
            let (owner, name) = source
                .repo
                .split_once('/')
                .ok_or_else(|| BackendError("invalid source".into()))?;
            Some(
                config
                    .home
                    .join("cache/mlx")
                    .join(owner)
                    .join(format!("{name}@{}", source.revision)),
            )
        } else if active.backend == "llamacpp" {
            let source = model
                .and_then(|model| model.source.gguf.as_ref())
                .ok_or_else(|| BackendError("active GGUF source unavailable".into()))?;
            let (owner, name) = source
                .repo
                .split_once('/')
                .ok_or_else(|| BackendError("invalid source".into()))?;
            Some(
                config
                    .home
                    .join("cache/llamacpp")
                    .join(owner)
                    .join(format!("{name}@{}", source.revision))
                    .join(&source.file),
            )
        } else {
            active.model_path.as_ref().map(PathBuf::from)
        };
        let request = ServeRequest {
            model_id: active.model_id.clone(),
            endpoint: active.endpoint.clone(),
            model_path: path,
            context: None,
            cache: None,
        };
        lifecycle.health(&active, &request, cancel).await?;
        return Ok((
            json!({"name":"state","status":"ok","detail":format!("serving {} at {}",strip_control(&active.model_id),active.endpoint)}),
            String::new(),
        ));
    }
    if options.command == "down" {
        let stopped = lifecycle
            .down_with_target(cancel, || {
                options
                    .model
                    .as_deref()
                    .map(|query| {
                        resolve(catalog, query)
                            .map(|resolved| resolved.model.id.clone())
                            .map_err(|error| BackendError(error.message))
                    })
                    .transpose()
            })
            .await?;
        let Some(active) = stopped else {
            return Ok((
                json!({"type":"no-active"}),
                "No active server to stop.\n".into(),
            ));
        };
        return Ok((
            json!({"type":if active.owned_by_us{"stopped"}else{"detached"},"modelId":active.model_id,"endpoint":active.endpoint}),
            if active.owned_by_us {
                format!(
                    "Stopped {} ({}).\n",
                    strip_control(&active.model_id),
                    active.endpoint
                )
            } else {
                format!(
                    "Detached from {} ({}); it was not started by rigspark and is still running.\n",
                    strip_control(&active.model_id),
                    active.endpoint
                )
            },
        ));
    }
    let query = options
        .model
        .as_deref()
        .ok_or_else(|| BackendError("model required".into()))?;
    let installed_fallback = options.bypass
        && resolve(catalog, query).is_err_and(|error| error.code == "MODEL_RESOLUTION_ERROR");
    if options.installed || installed_fallback {
        warning(observer, LifecycleWarning::InstalledBypass);
        if options
            .backend
            .as_ref()
            .is_some_and(|backend| backend != "ollama")
        {
            return Err(BackendError(
                "installed model bypass requires Ollama".into(),
            ));
        }
        let port = options
            .port
            .or_else(|| {
                prior
                    .active
                    .as_ref()
                    .filter(|active| active.backend == "ollama")
                    .map(|active| active.port)
            })
            .unwrap_or(11434);
        let endpoint = format!("http://127.0.0.1:{port}");
        let reviewed = lifecycle.review(query, cancel).await?;
        if !control.occupied(&endpoint).await? {
            return Err(BackendError(
                "installed activation requires an existing Ollama daemon".into(),
            ));
        }
        let observed = listener(&probe, &endpoint, cancel).await?;
        if !ollama.trusts(&observed.identity) {
            return Err(BackendError("untrusted Ollama daemon".into()));
        }
        let checked = CheckedHttp {
            inner: &http,
            probe: &probe,
            expected: observed.identity,
        };
        let verifier = LocalVerifier {
            root: ollama_root.clone(),
        };
        let support = InstalledModels::new(&checked, &verifier);
        let model = support
            .inspect(&endpoint, query, cancel)
            .await
            .map_err(|error| BackendError(error.to_string()))?;
        let catalog_quant = catalog
            .models
            .iter()
            .find(|entry| entry.source.ollama.as_deref() == Some(model.id.as_str()))
            .and_then(|entry| {
                entry
                    .quantizations
                    .iter()
                    .find(|quant| Some(&quant.name) == model.quant.as_ref())
                    .or_else(|| entry.quantizations.first())
            });
        let activation = ContextActivation {
            source: model.id.clone(),
            context: options.context,
            installed: true,
            expected_manifest: Some(model.digest),
            expected_sha: catalog_quant.and_then(|quant| quant.sha256.clone()),
            expected_bytes: catalog_quant.map(|quant| quant.disk_bytes as u64),
            projectors: catalog_quant
                .map(|quant| quant.projectors.clone())
                .unwrap_or_default(),
            root: ollama_root,
            http: &http,
            probe: &probe,
            observer,
        };
        let request = ServeRequest {
            model_id: model.id,
            endpoint,
            model_path: None,
            context: options.context,
            cache: options.cache.over(None),
        };
        let active = lifecycle
            .attach_installed(&request, &reviewed, cancel, &activation)
            .await?;
        return Ok(up_output(&active, "local-manifest"));
    }
    let resolved = resolve(catalog, query).map_err(|error| BackendError(error.message))?;
    let model = resolved.model;
    // A switch keeps the running profile unless a flag overrides part of it.
    let prior_cache = prior.active.as_ref().and_then(|active| active.cache);
    let cache_kept = options.command == "switch"
        && crate::cache::requested(options.cache, true, prior_cache, None) == prior_cache;
    if options.command == "switch"
        && prior
            .active
            .as_ref()
            .is_some_and(|active| active.model_id == model.id)
        && options.context.is_none()
        && !options.bypass
        && cache_kept
    {
        let active = prior
            .active
            .as_ref()
            .ok_or_else(|| BackendError("no active model".into()))?;
        return Ok((
            json!({"type":"already-active","modelId":model.id,"endpoint":active.endpoint}),
            format!("{} is already active.\n", model.id),
        ));
    }
    let hardware =
        hardware.ok_or_else(|| BackendError("hardware required for model selection".into()))?;
    let simple_switch =
        options.command == "switch" && options.context.is_none() && !options.bypass && cache_kept;
    let env_backend = std::env::var("RIGSPARK_BACKEND").ok();
    let user = config
        .user_config()
        .map_err(|error| BackendError(error.to_string()))?;
    let requested = crate::cache::requested(
        options.cache,
        options.command == "switch",
        prior_cache,
        user.cache,
    );
    let user_backend = user.default_backend;
    let configured = preferred_backend(
        options.backend.as_deref(),
        env_backend.as_deref(),
        user_backend.as_deref(),
    );
    let backend = if options.command == "switch" {
        prior
            .active
            .as_ref()
            .ok_or_else(|| BackendError("no active model".into()))?
            .backend
            .clone()
    } else if let Some(backend) = configured {
        backend
    } else {
        let mut selected = None;
        for name in auto_backend_order(model, hardware) {
            let index = backend_index(name);
            if paths[index].is_some() {
                let args = if name == "mlx" {
                    vec![
                        "-I".into(),
                        "-c".into(),
                        "import importlib.metadata; print(importlib.metadata.version('mlx-lm'))"
                            .into(),
                    ]
                } else {
                    vec!["--version".into()]
                };
                if let Ok(version) = commands
                    .run(&binary(index), &args, cancel, Duration::from_secs(5))
                    .await
                    && (name != "mlx" || version.trim() == "0.31.3")
                {
                    selected = Some(name.to_owned());
                    break;
                }
            }
        }
        selected.ok_or_else(|| BackendError("no installed backend supports this model".into()))?
    };
    check_backend(
        model,
        &backend,
        hardware,
        backend_known(&backend) && paths[backend_index(&backend)].is_some(),
    )?;
    let index = backend_index(&backend);
    crate::cache::launch_delta(requested.as_ref(), &backend, true)
        .map_err(|error| BackendError(error.to_string()))?;
    let plan = plan_quantization(
        model,
        resolved.quant,
        &backend,
        hardware,
        PlanOptions {
            simple_switch,
            bypass: options.bypass,
            context: options.context,
            kv_cache: requested
                .filter(|profile| profile.kv_k == profile.kv_v)
                .map(|profile| profile.kv_k),
        },
    )?;
    let quant = plan.quant;
    let reviewed = lifecycle.review(&model.id, cancel).await?;
    let pull_request = catalog_pull_request(model, &quant, &backend)?;
    if plan.estimated_fit {
        warning(observer, LifecycleWarning::EstimatedFit);
    }
    let diagnostics = observer.and_then(|observer| observer.diagnostics.clone());
    let acquisition = Acquisition::new(config.home.join("cache"))?.with_progress(
        move |completed, total, file| {
            download_progress(diagnostics.as_ref(), completed, total, file)
        },
    );
    let download = HfTransport::new()?;
    let studio_root = home.join(".lmstudio/models");
    let pulls = PullService {
        acquisition: &acquisition,
        download: &download,
        commands: &commands,
        ollama_models: &ollama_root,
        studio_models: &studio_root,
    };
    let port = options
        .port
        .or_else(|| {
            if options.command == "switch" {
                prior.active.as_ref().map(|active| active.port)
            } else {
                None
            }
        })
        .unwrap_or(match backend.as_str() {
            "ollama" => 11434,
            "lmstudio" => 1234,
            _ => 8080,
        });
    let endpoint = format!("http://127.0.0.1:{port}");
    let pull_binary = binary(index);
    let pull = pulls.pull_at_observed(&pull_request, &pull_binary, &endpoint, cancel, observer);
    let prepared = if backend == "ollama" {
        crate::pull::with_ollama_daemon(&acquisition_daemon, &endpoint, cancel, pull).await?
    } else {
        pull.await?
    };
    if !prepared.digest_verified {
        warning(observer, LifecycleWarning::SizeOnly);
    }
    if simple_switch {
        let active = lifecycle
            .switch_pointer(&model.id, &reviewed, cancel)
            .await?;
        return Ok((
            json!({"type":"switched","modelId":active.model_id,"endpoint":active.endpoint}),
            format!("Switched to {} ({}).\n", active.model_id, active.endpoint),
        ));
    }
    let request = ServeRequest {
        model_id: model.id.clone(),
        endpoint,
        model_path: prepared.model_path,
        context: options.context,
        cache: requested,
    };
    let active = if options.context.is_some() {
        let activation = ContextActivation {
            source: pull_request.model_id,
            context: options.context,
            installed: false,
            expected_manifest: prepared.local_manifest_digest,
            expected_sha: quant.sha256.clone(),
            expected_bytes: Some(quant.disk_bytes as u64),
            projectors: quant.projectors.clone(),
            root: ollama_root,
            http: &http,
            probe: &probe,
            observer,
        };
        lifecycle
            .replace_with(&backend, &request, &reviewed, cancel, &activation)
            .await?
    } else {
        lifecycle
            .replace(&backend, &request, &reviewed, cancel)
            .await?
    };
    Ok(up_output(
        &active,
        if prepared.digest_verified {
            "verified"
        } else {
            "size-only"
        },
    ))
}
fn up_output(active: &ServerState, integrity: &str) -> (Value, String) {
    let mut report = json!({"modelId":active.model_id,"backend":active.backend,"endpoint":active.endpoint,"ownership":if active.owned_by_us{"owned"}else{"attached"},"integrity":integrity});
    let mut text = format!(
        "{} ready at {}\n",
        strip_control(&active.model_id),
        active.endpoint
    );
    if let Some(runtime) = &active.runtime_model_id {
        text.push_str(&format!("Runtime model: {}", strip_control(runtime)));
        if let Some(context) = active.context {
            text.push_str(&format!(" (context {context})"));
        }
        text.push('\n');
    }
    if let Some(cache) = &active.cache {
        report["cache"] = json!(cache);
        text.push_str(&format!("Cache: {}\n", cache.summary()));
    }
    (report, text)
}

pub async fn installed_inventory(
    hardware: &Hardware,
    model: Option<&str>,
    port: u16,
    context: Option<u32>,
    fits_only: bool,
    cancel: &CancellationToken,
) -> Result<(Value, String, u8), BackendError> {
    if port == 0 || context.is_some_and(|context| !(1..=10000000).contains(&context)) {
        return Err(BackendError("invalid installed comparison options".into()));
    }
    let binary = resolve_binary("ollama")?;
    let endpoint = format!("http://127.0.0.1:{port}");
    let http = NativeTransport::new().map_err(|error| BackendError(error.to_string()))?;
    let probe = NativeProcessProbe;
    let before = listener(&probe, &endpoint, cancel).await?;
    if binary.to_str() != Some(before.identity.executable.as_str()) {
        return Err(BackendError(
            "untrusted installed inventory listener".into(),
        ));
    }
    let checked = CheckedHttp {
        inner: &http,
        probe: &probe,
        expected: before.identity.clone(),
    };
    let root = std::env::var_os("OLLAMA_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_default()
                .join(".ollama/models")
        });
    let verifier = LocalVerifier { root };
    let support = InstalledModels::new(&checked, &verifier);
    let models = if let Some(model) = model {
        vec![
            support
                .inspect(&endpoint, model, cancel)
                .await
                .map_err(|error| BackendError(error.to_string()))?,
        ]
    } else {
        let mut models = Vec::new();
        for entry in support
            .list(&endpoint, cancel)
            .await
            .map_err(|error| BackendError(error.to_string()))?
        {
            models.push(
                support
                    .inspect(&endpoint, &entry.id, cancel)
                    .await
                    .map_err(|error| BackendError(error.to_string()))?,
            );
        }
        models
    };
    let mut results = Vec::new();
    let mut lines = Vec::new();
    for entry in models {
        let sized = crate::ollama_installed::size_installed(&entry, hardware, context)
            .map_err(|error| BackendError(error.to_string()))?;
        if fits_only && sized["fit"] != "yes" {
            continue;
        }
        lines.push(format!(
            "{}: context {}; estimated {} fit {}; weights {:.2} GiB ({}); throughput unknown",
            strip_control(&entry.id),
            context
                .map(|value| value.to_string())
                .unwrap_or_else(|| "default".into()),
            sized["memoryKind"].as_str().unwrap_or("ram"),
            sized["fit"].as_str().unwrap_or("unknown"),
            entry.size_bytes as f64 / 1073741824.0,
            if sized["weightsFit"] == true {
                "fit"
            } else {
                "over budget"
            }
        ));
        results.push(sized);
    }
    let after = listener(&probe, &endpoint, cancel).await?;
    if !same_process(&before.identity, &after.identity) {
        return Err(BackendError("inventory listener changed".into()));
    }
    let exit =
        u8::from(model.is_some() && results.first().is_some_and(|result| result["fit"] == "no"));
    Ok((
        json!({"source":"local-runtime-metadata","models":results}),
        format!(
            "{}\n",
            if lines.is_empty() {
                "No installed models match.".into()
            } else {
                lines.join("\n")
            }
        ),
        exit,
    ))
}

#[cfg(test)]
mod acquisition_contract_tests {
    use super::*;

    #[test]
    fn catalog_pull_preserves_quant_digest_size_and_runtime_model_id() {
        let mut catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
        let model = catalog
            .models
            .iter_mut()
            .find(|model| model.source.ollama.is_some())
            .unwrap();
        model.quantizations[0].sha256 = Some("b".repeat(64));
        model.quantizations[0].disk_bytes = 5_000_000_000.0;
        let id = model.id.clone();
        let quant_query = format!("{id}-{}", model.quantizations[0].name);
        for query in [&id, &quant_query] {
            let resolved = resolve(&catalog, query).unwrap();
            let quant = resolved.quant.unwrap_or(&resolved.model.quantizations[0]);
            let request = catalog_pull_request(resolved.model, quant, "ollama").unwrap();
            assert_eq!(request.expected_sha256, Some("b".repeat(64)));
            assert_eq!(request.expected_bytes, 5_000_000_000);
            assert_eq!(Some(request.model_id), resolved.model.source.ollama);
        }
        let model = &mut catalog.models[0];
        model.source.ollama = None;
        assert!(catalog_pull_request(model, &model.quantizations[0], "ollama").is_err());
    }

    #[test]
    fn success_lines_strip_controls_and_report_integrity_and_ownership() {
        let active = ServerState {
            backend: "ollama".into(),
            model_id: "evil\u{1b}[31m\nid".into(),
            endpoint: "http://127.0.0.1:11434".into(),
            port: 11434,
            owned_by_us: true,
            pid: Some(7),
            runtime_model_id: Some("variant\u{7}".into()),
            context: Some(65_536),
            integrity: None,
            local_manifest_digest: None,
            model_path: None,
            process_executable: None,
            process_started_at: None,
            auth_token: None,
            cache: None,
        };
        let (report, text) = up_output(&active, "size-only");
        assert_eq!(
            (report["ownership"].as_str(), report["integrity"].as_str()),
            (Some("owned"), Some("size-only"))
        );
        assert!(
            !text
                .chars()
                .any(|character| character.is_control() && character != '\n'),
            "{text:?}"
        );
        assert_eq!(text.lines().count(), 2, "{text:?}");
        assert!(text.ends_with("(context 65536)\n"));
    }
}
