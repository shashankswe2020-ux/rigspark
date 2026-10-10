#[cfg(test)]
mod workflow_tests {
    use super::*;
    #[tokio::test]
    async fn workflow_flags_fail_before_runtime_or_storage_access() {
        for args in [
            vec!["llmup-native", "ls", "--message", "hello"],
            vec!["llmup-native", "chat", "--message", "hello", "--installed"],
            vec![
                "llmup-native",
                "migrate",
                "--from",
                "a",
                "--to",
                "b",
                "--move",
            ],
            vec![
                "llmup-native",
                "migrate",
                "--from",
                "a",
                "--to",
                "b",
                "--yes",
            ],
        ] {
            assert!(execute(Args::try_parse_from(args).unwrap()).await.is_err());
        }
    }
    #[test]
    fn workflow_arguments_parse_without_changing_advice_defaults() {
        let chat = Args::try_parse_from([
            "llmup-native",
            "chat",
            "-m",
            "test",
            "--harness",
            "openai",
            "--message",
            "hello",
        ])
        .unwrap();
        assert_eq!(chat.chat_model.as_deref(), Some("test"));
        assert_eq!(chat.harness.as_deref(), Some("openai"));
        assert_eq!(
            Args::try_parse_from(["llmup-native"]).unwrap().command,
            "recommend"
        );
    }
}
mod native_args;

use clap::Parser;
use rigspark_core::{
    advice::{hardware_score, verdict},
    catalog::{Catalog, PerfDataset, resolve},
    ranking::{AdviceOptions, recommend},
    reports::{can_run, catalog_text, recommendation_text, sanitized, strip_control},
    sizing::Hardware,
};
use rigspark_runtime::{
    diagnostics,
    hardware::{detect, validate_hardware},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    name = "rigspark",
    version,
    about = "Hardware-aware local model advice, runtime management, and chat"
)]
struct Args {
    #[arg(default_value="recommend", value_parser=["recommend","can-run","plan","catalog","doctor","ls","up","switch","down","chat","migrate","gui","generate"])]
    command: String,
    model: Option<String>,
    #[arg(short = 'm', long = "model")]
    chat_model: Option<String>,
    #[arg(long)]
    harness: Option<String>,
    #[arg(long)]
    message: Option<String>,
    #[arg(long)]
    agent: Option<String>,
    #[arg(long = "skill")]
    skills: Vec<String>,
    #[arg(long)]
    no_memory: bool,
    #[arg(long)]
    no_open: bool,
    #[arg(long)]
    from: Option<String>,
    #[arg(long)]
    to: Option<String>,
    #[arg(long = "move")]
    move_memory: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    yes: bool,
    #[arg(long)]
    json: bool,
    #[arg(long)]
    context: Option<f64>,
    #[arg(long)]
    max_context: bool,
    #[arg(long)]
    context_percent: Option<u8>,
    #[arg(long)]
    task: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=3))]
    month: Option<u8>,
    #[arg(long, hide = true)]
    today: Option<String>,
    #[arg(long)]
    backend: Option<String>,
    #[arg(long)]
    port: Option<u32>,
    #[arg(long)]
    bypass: bool,
    #[arg(long)]
    installed: bool,
    #[arg(long)]
    fits_only: bool,
    #[arg(long)]
    available_backends: bool,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    refresh: bool,
    #[arg(long, hide = true)]
    no_tui: bool,
    #[arg(long, hide = true)]
    tui: bool,
    #[arg(long, hide = true)]
    no_color: bool,
    #[arg(long, hide = true)]
    accessible: bool,
    #[arg(long, hide = true)]
    hardware_json: Option<String>,
    #[arg(long)]
    hardware: Option<PathBuf>,
    #[arg(long, hide = true)]
    parity: bool,
    #[arg(long)]
    catalog_path: Option<PathBuf>,
    #[arg(long)]
    perf_path: Option<PathBuf>,
    #[arg(long)]
    forget: bool,
    #[arg(long, value_parser = ["f16", "q8_0", "q4_0"])]
    kv_cache: Option<String>,
    #[arg(long, value_parser = ["auto", "on", "off"])]
    flash_attn: Option<String>,
    #[arg(long, value_parser = ["off", "reuse"])]
    prompt_cache: Option<String>,
    #[arg(long, conflicts_with = "status")]
    update: bool,
    #[arg(long)]
    status: bool,
    #[arg(long)]
    generation: bool,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long)]
    seed: Option<u64>,
    #[arg(long)]
    comfyui_dir: Option<PathBuf>,
}

impl Args {
    fn kv_cache(&self) -> Option<rigspark_core::sizing::KvCacheType> {
        self.kv_cache
            .as_deref()
            .and_then(rigspark_core::sizing::KvCacheType::parse)
    }
    fn cache_flags(&self) -> rigspark_runtime::cache::CacheFlags {
        use rigspark_runtime::cache::{FlashAttention, PromptReuse};
        rigspark_runtime::cache::CacheFlags {
            kv: self.kv_cache(),
            flash_attention: self.flash_attn.as_deref().and_then(FlashAttention::parse),
            prompt_reuse: self.prompt_cache.as_deref().and_then(PromptReuse::parse),
        }
    }
}

enum AdviceRequest {
    Installed {
        model: Option<String>,
        port: u16,
        context: Option<u32>,
        fits_only: bool,
    },
    Catalog(AdviceOptions),
}

impl Args {
    fn advice_request(
        &self,
        backends: Option<Vec<diagnostics::BackendInfo>>,
    ) -> Result<AdviceRequest, Box<dyn std::error::Error>> {
        if self.installed && ["recommend", "can-run"].contains(&self.command.as_str()) {
            return Ok(AdviceRequest::Installed {
                model: self.model.clone(),
                port: self.runtime_port()?.unwrap_or(11434),
                context: self.context.map(|context| context as u32),
                fits_only: self.fits_only,
            });
        }
        let options = AdviceOptions {
            task: self.task.clone(),
            context: self.context,
            context_percent: self.context_percent,
            max_context: self.max_context,
            backend: self.backend.clone(),
            kv_cache: self.kv_cache(),
            available_backends: if self.available_backends {
                Some(
                    backends
                        .ok_or("backend availability was not collected")?
                        .into_iter()
                        .filter(|entry| entry.installed)
                        .map(|entry| entry.name)
                        .collect(),
                )
            } else {
                None
            },
        };
        options.validate()?;
        Ok(AdviceRequest::Catalog(options))
    }

    fn runtime_port(&self) -> Result<Option<u16>, Box<dyn std::error::Error>> {
        match self.port {
            Some(port @ 1..=65535) => Ok(Some(port as u16)),
            Some(port) => {
                Err(format!("invalid --port {port} (expected an integer in 1..65535)").into())
            }
            None => Ok(None),
        }
    }

    fn lifecycle_options(
        &self,
    ) -> Result<rigspark_runtime::application::LifecycleOptions, Box<dyn std::error::Error>> {
        let options = rigspark_runtime::application::LifecycleOptions {
            command: self.command.clone(),
            model: self.model.clone(),
            backend: self.backend.clone(),
            port: self.runtime_port()?,
            context: self.context.map(|value| value as u32),
            installed: self.installed,
            bypass: self.bypass,
            cache: self.cache_flags(),
        };
        options.validate()?;
        Ok(options)
    }
}

async fn run_generation(args: &Args) -> Result<u8, Box<dyn std::error::Error>> {
    use rigspark_core::generation::{GenerationCatalog, catalog_text};
    use rigspark_runtime::generation::{
        MAX_SEED, NativeOptions, default_comfyui_dir, prepare, random_seed, run_native,
        validate_prompt,
    };
    let catalog = GenerationCatalog::bundled()?;
    if args.command == "catalog" {
        if args.refresh
            || args.all
            || args.json
            || args.tui
            || args.accessible
            || args.catalog_path.is_some()
            || args.perf_path.is_some()
            || args.model.is_some()
            || args.parity
        {
            return Err("--generation only combines with --hardware".into());
        }
        let hardware = match (&args.hardware_json, &args.hardware) {
            (Some(_), Some(_)) => return Err("use either --hardware or --hardware-json".into()),
            (Some(raw), None) => parse_hardware_input(raw)?,
            (None, Some(path)) => parse_hardware_input(&read_bounded(path, 65536)?)?,
            (None, None) => detected_hardware().await?,
        };
        print!("{}", catalog_text(&catalog, &hardware));
        return Ok(0);
    }
    if args.command != "generate" {
        return Err("--generation is only supported by catalog".into());
    }
    if args.accessible {
        return Err("--accessible is not supported by generate; use --tui or plain output".into());
    }
    let port = args
        .runtime_port()?
        .unwrap_or(rigspark_runtime::generation::DEFAULT_PORT);
    if let Some(seed) = args.seed.filter(|seed| *seed > MAX_SEED) {
        return Err(format!("invalid --seed {seed} (expected 0..={MAX_SEED})").into());
    }
    let selection = rigspark_cli::tui_mode::resolve(
        &rigspark_cli::tui_mode::Options {
            json: args.json,
            tui: args.tui,
            no_tui: args.no_tui,
            no_color: args.no_color,
            environment_no_color: std::env::var_os("NO_COLOR").is_some(),
            ..Default::default()
        },
        &rigspark_cli::tui_mode::capture(),
    )
    .map_err(|reason| format!("interactive UI is incompatible with this invocation ({reason})"))?;
    let interactive =
        selection.mode == rigspark_cli::tui_mode::Mode::Tui && (args.tui || args.prompt.is_none());
    let comfyui_dir = args.comfyui_dir.clone().or_else(default_comfyui_dir);
    if interactive {
        use rigspark_cli::tui_generate::{GenerateView, model_rows, run};
        if let Some(query) = args.model.as_deref() {
            catalog.resolve(query)?;
        }
        let hardware = detected_hardware().await?;
        let mut view = GenerateView::new(model_rows(&catalog, &hardware), std::env::current_dir()?);
        view.select(args.model.as_deref().unwrap_or("image"));
        view.prompt = args.prompt.clone().unwrap_or_default();
        view.directory = comfyui_dir
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        view.seed = args.seed.map(|seed| seed.to_string()).unwrap_or_default();
        view.port = port;
        view.bypass = args.bypass;
        let code = run(
            &mut view,
            &rigspark_runtime::generation::NativeGenerator,
            selection.color,
        )
        .await?;
        for path in &view.saved {
            println!("Saved: {}", strip_control(&path.display().to_string()));
        }
        return Ok(code);
    }
    let query = args
        .model
        .as_deref()
        .ok_or("generate requires image, video, or a generation model id")?;
    let prompt = args.prompt.as_deref().ok_or("--prompt is required")?;
    validate_prompt(prompt)?;
    let options = NativeOptions {
        model: query.into(),
        prompt: prompt.into(),
        seed: Some(args.seed.unwrap_or_else(random_seed)),
        comfyui_dir: comfyui_dir.ok_or(
            "--comfyui-dir (or RIGSPARK_COMFYUI_DIR) must point at your ComfyUI installation",
        )?,
        port,
        bypass: args.bypass,
        output: args.output.clone(),
        output_dir: std::env::current_dir()?,
    };
    prepare(&options)?;
    let events: rigspark_runtime::generation::Events = std::sync::Arc::new(|message: String| {
        eprintln!("{}", strip_control(&message));
    });
    let cancel = tokio_util::sync::CancellationToken::new();
    let operation = run_native(&options, Some(events), &cancel);
    tokio::pin!(operation);
    let outcome = tokio::select! {
        result = &mut operation => result?,
        _ = tokio::signal::ctrl_c() => {
            cancel.cancel();
            operation.await?
        }
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&sanitized(
                &json!({"type": "generation", "result": outcome.result, "fit": outcome.fit})
            ))?
        );
    } else {
        println!(
            "Saved {}: {} ({} bytes)\nModel: {}  Seed: {}  ComfyUI prompt: {}",
            outcome.result.kind.name(),
            strip_control(&outcome.result.path.display().to_string()),
            outcome.result.bytes,
            outcome.result.model,
            outcome.result.seed,
            outcome.result.prompt_id
        );
    }
    Ok(0)
}

async fn detected_hardware() -> Result<Hardware, Box<dyn std::error::Error>> {
    let (hardware, warnings) = detect().await?;
    for warning in warnings {
        eprintln!("{}", strip_control(&warning));
    }
    Ok(hardware)
}

fn read_file(path: &PathBuf) -> Result<String, Box<dyn std::error::Error>> {
    read_bounded(path, 16 * 1024 * 1024)
}

fn load_catalog_raw(args: &Args) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(path) = &args.catalog_path {
        return read_file(path);
    }
    if args.parity {
        return Ok(rigspark_core::MODELS_JSON.into());
    }
    let config = rigspark_runtime::state::Config::load()?;
    let loaded = rigspark_runtime::catalog_update::CatalogStore::official(config.home).load()?;
    for warning in &loaded.status.warnings {
        eprintln!("Catalog: {warning}");
    }
    Ok(loaded.raw)
}

fn read_bounded(path: &PathBuf, limit: usize) -> Result<String, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(format!("{} exceeds {} KiB", path.display(), limit / 1024).into());
    }
    Ok(String::from_utf8(bytes)?)
}

async fn run_plan(
    args: &Args,
    catalog: &Catalog,
    perf: &PerfDataset,
    supplied: Option<Hardware>,
) -> Result<u8, Box<dyn std::error::Error>> {
    let query = args.model.as_deref().ok_or("model is required")?;
    let resolved = resolve(catalog, query).map_err(|error| error.message)?;
    let mut model = resolved.model.clone();
    if let Some(quant) = resolved.quant {
        model.quantizations = vec![quant.clone()];
    }
    let hardware = match supplied {
        Some(hardware) => hardware,
        None => {
            let (hardware, warnings) = detect().await?;
            for warning in warnings {
                eprintln!("{}", strip_control(&warning));
            }
            hardware
        }
    };
    let backend = args.backend.as_deref().unwrap_or("ollama");
    let plan = rigspark_core::plan::plan_with_cache(
        &model,
        &hardware,
        perf,
        args.context,
        backend,
        args.kv_cache(),
    )?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        print!("{}", rigspark_core::plan::format_plan(&plan));
    }
    Ok(u8::from(plan.recommended.is_none()))
}

fn parse_hardware_input(raw: &str) -> Result<Hardware, Box<dyn std::error::Error>> {
    if raw.len() > 65536 {
        return Err("hardware input exceeds 64 KiB".into());
    }
    let hardware: Hardware = serde_json::from_str(raw)?;
    validate_hardware(&hardware)?;
    Ok(hardware)
}

struct TerminalEngine {
    provider: String,
    model: Option<String>,
    agent: Option<String>,
    skills: Vec<String>,
    capture: bool,
    expected: std::sync::Mutex<Option<rigspark_runtime::state::RuntimeState>>,
}
impl rigspark_cli::terminal::ChatEngine for TerminalEngine {
    async fn reply(
        &self,
        messages: &[rigspark_runtime::harness::HarnessMessage],
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<rigspark_cli::terminal::ChatReply, String> {
        let (deltas, _ignored) = tokio::sync::mpsc::unbounded_channel();
        self.reply_streaming(messages, cancel, deltas).await
    }

    async fn reply_streaming(
        &self,
        messages: &[rigspark_runtime::harness::HarnessMessage],
        cancel: &tokio_util::sync::CancellationToken,
        deltas: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> Result<rigspark_cli::terminal::ChatReply, String> {
        let (message, history) = messages.split_last().ok_or("empty conversation")?;
        let expected = if self.provider == "local" {
            let config =
                rigspark_runtime::state::Config::load().map_err(|error| error.to_string())?;
            let current = rigspark_runtime::state::StateStore::new(config)
                .read()
                .map_err(|error| error.to_string())?;
            let mut expected = self
                .expected
                .lock()
                .map_err(|_| "session state unavailable")?;
            Some(expected.get_or_insert(current).clone())
        } else {
            None
        };
        let options = rigspark_runtime::native_chat::NativeChatOptions {
            provider: self.provider.clone(),
            model: self.model.clone(),
            message: message.content.clone(),
            agent: self.agent.clone(),
            skills: self.skills.clone(),
            capture: self.capture,
        };
        let result = rigspark_runtime::native_chat::run_with_history(
            &options,
            Some(history),
            expected.as_ref(),
            cancel,
            &mut |text| {
                // A closed receiver only means nobody is rendering partial output.
                let _ = deltas.send(text.to_owned());
                Ok(())
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        Ok(rigspark_cli::terminal::ChatReply {
            content: result["content"].as_str().ok_or("missing response")?.into(),
            memory_warning: result["memoryCaptured"] == false,
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ParityRequest {
    hardware: Hardware,
    #[serde(default)]
    options: AdviceOptions,
    queries: Option<Vec<String>>,
}

fn picker_models<'catalog>(
    catalog: &'catalog Catalog,
    command: &str,
    active: Option<&rigspark_runtime::state::ServerState>,
) -> Vec<&'catalog rigspark_core::catalog::CatalogModel> {
    catalog
        .models
        .iter()
        .filter(|model| {
            command != "switch"
                || active
                    .is_none_or(|active| active.backend == "ollama" && model.id != active.model_id)
        })
        .collect()
}

async fn execute(mut args: Args) -> Result<u8, Box<dyn std::error::Error>> {
    if args.update || args.status {
        if args.command != "catalog"
            || args.refresh
            || args.all
            || args.catalog_path.is_some()
            || args.perf_path.is_some()
            || args.hardware.is_some()
            || args.hardware_json.is_some()
            || args.parity
            || args.tui
            || args.accessible
            || args.model.is_some()
            || args.generation
        {
            return Err("--update and --status require catalog without browse, refresh, fixture, or interactive options".into());
        }
        let config = rigspark_runtime::state::Config::load()?;
        let store = rigspark_runtime::catalog_update::CatalogStore::official(config.home);
        let status = if args.update {
            store
                .update(&rigspark_runtime::catalog_update::OfficialCatalogTransport)
                .await?
        } else {
            store.load()?.status
        };
        println!(
            "Source: {}\nGenerated: {}\nRevision: {}\nDigest: {}\nModels: {}\nUpdates: {}",
            status.source,
            status.generated_at,
            status
                .revision
                .map(|revision| revision.to_string())
                .unwrap_or_else(|| "bundled".into()),
            status.digest,
            status.model_count,
            if status.updates_configured {
                "configured"
            } else {
                "unavailable (production signing key not provisioned)"
            }
        );
        if let Some(published) = status.published_at {
            println!("Published: {published}");
        }
        for warning in status.warnings {
            eprintln!("Catalog: {warning}");
        }
        return Ok(0);
    }
    if args.accessible && (args.no_tui || args.json || args.message.is_some()) {
        return Err("--accessible conflicts with --no-tui, --json, and --message".into());
    }
    if !args.parity {
        AdviceOptions {
            task: args.task.clone(),
            context: args.context,
            context_percent: args.context_percent,
            max_context: args.max_context,
            backend: args.backend.clone(),
            available_backends: None,
            kv_cache: args
                .kv_cache()
                .filter(|_| ["recommend", "can-run", "plan"].contains(&args.command.as_str())),
        }
        .validate()?;
        for (label, value) in [
            ("model", args.model.as_deref()),
            ("--model", args.chat_model.as_deref()),
            ("--from", args.from.as_deref()),
            ("--to", args.to.as_deref()),
        ] {
            if value.is_some_and(|value| {
                value.trim().is_empty() || value.len() > 8192 || value.chars().any(char::is_control)
            }) {
                return Err(format!("invalid {label} reference").into());
            }
        }
        if args.installed
            && let Some(model) = args.model.as_deref()
        {
            rigspark_runtime::adapters::model_id(model)?;
        }
        if ["recommend", "can-run"].contains(&args.command.as_str())
            && args.port.is_some()
            && !args.installed
        {
            return Err("--port requires --installed".into());
        }
        if ["up", "switch"].contains(&args.command.as_str())
            && args.installed
            && (!args.bypass
                || args
                    .backend
                    .as_deref()
                    .is_some_and(|backend| backend != "ollama"))
        {
            return Err("installed models require --bypass and Ollama".into());
        }
        if args.command == "chat" {
            let harness = args.harness.as_deref().unwrap_or("local");
            if !["local", "claude", "openai", "openai-compatible", "opencode"].contains(&harness) {
                return Err("unknown chat harness".into());
            }
            if args.command == "chat" && harness != "local" && args.chat_model.is_none() {
                return Err("--model is required for remote chat".into());
            }
            rigspark_runtime::library::validate_selection(args.agent.as_deref(), &args.skills)?;
        }
        if args
            .message
            .as_ref()
            .is_some_and(|message| message.trim().is_empty() || message.len() > 1024 * 1024)
        {
            return Err("--message must contain text and be at most 1 MiB".into());
        }
    }
    if args.command == "gui" {
        use rigspark_cli::gui_launcher::{GuiOptions, run_gui};
        let port = args.port.map(|port| port.to_string());
        let options = GuiOptions::new(port.as_deref(), args.no_open)?
            .with_harness(args.harness.as_deref())?
            .with_json(args.json);
        return Ok(u8::try_from(run_gui(options).await?).unwrap_or(1));
    }
    if args.command == "generate" || args.generation {
        return run_generation(&args).await;
    }
    let port = args.runtime_port()?;
    let presentation_title = args.command.clone();
    let visual_supported = ([
        "recommend",
        "can-run",
        "catalog",
        "doctor",
        "ls",
        "up",
        "switch",
        "down",
    ]
    .contains(&args.command.as_str())
        || (args.command == "chat" && args.message.is_none() && !args.json && !args.accessible))
        && !args.parity;
    if args.tui && !visual_supported {
        return Err("--tui requires a native read-only command or interactive chat without --accessible, --json, or --message".into());
    }
    let presentation = if visual_supported {
        Some(
            rigspark_cli::tui_mode::resolve(
                &rigspark_cli::tui_mode::Options {
                    json: args.json,
                    tui: args.tui,
                    no_tui: args.no_tui,
                    no_color: args.no_color,
                    accessible: args.accessible,
                    environment_no_color: std::env::var_os("NO_COLOR").is_some(),
                    ..Default::default()
                },
                &rigspark_cli::tui_mode::capture(),
            )
            .map_err(|reason| {
                format!("interactive UI is incompatible with this invocation ({reason})")
            })?,
        )
    } else {
        None
    };
    if args.refresh && (args.command != "catalog" || args.parity) {
        return Err("--refresh is only supported by catalog".into());
    }
    if args.accessible
        && ![
            "chat",
            "up",
            "switch",
            "down",
            "ls",
            "doctor",
            "can-run",
            "recommend",
            "catalog",
        ]
        .contains(&args.command.as_str())
    {
        return Err("--accessible is not supported for this command".into());
    }
    if args.command != "chat"
        && (args.chat_model.is_some()
            || (args.harness.is_some() && args.command != "gui")
            || args.message.is_some()
            || args.agent.is_some()
            || !args.skills.is_empty()
            || args.no_memory)
    {
        return Err("chat options require chat".into());
    }
    if args.command != "migrate"
        && (args.from.is_some()
            || args.to.is_some()
            || args.move_memory
            || args.dry_run
            || (args.yes && args.command != "down"))
    {
        return Err("migration options require migrate".into());
    }
    if args.forget {
        if args.command != "down" || args.model.is_some() || args.parity {
            return Err("--forget is only supported by down without a model".into());
        }
        let (report, text) = rigspark_runtime::application::forget_attached_with_config(
            rigspark_runtime::state::Config::load()?,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await?;
        if args.json {
            println!("{}", serde_json::to_string_pretty(&sanitized(&report))?);
        } else {
            print!("{text}");
        }
        return Ok(0);
    }
    if ["chat", "migrate"].contains(&args.command.as_str()) {
        if args.model.is_some()
            || args.backend.is_some()
            || port.is_some()
            || args.bypass
            || args.installed
            || args.fits_only
            || args.all
            || args.max_context
            || args.context_percent.is_some()
            || args.available_backends
            || args.task.is_some()
            || args.parity
            || args.hardware_json.is_some()
            || args.hardware.is_some()
        {
            return Err("advice and lifecycle options cannot be used with chat or migrate".into());
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        if args.command == "chat" {
            if args.context.is_some() {
                return Err("--context is not a chat option".into());
            }
            if args.message.is_none() && !args.json {
                use rigspark_cli::terminal::{Mode, run_chat, stdin_turns};
                let engine = TerminalEngine {
                    provider: args.harness.unwrap_or_else(|| "local".into()),
                    model: args.chat_model,
                    agent: args.agent,
                    skills: args.skills,
                    capture: !args.no_memory,
                    expected: Default::default(),
                };
                if let Some(selection) = &presentation
                    && selection.mode == rigspark_cli::tui_mode::Mode::Tui
                {
                    let title = format!(
                        "{} / {}",
                        engine.provider,
                        engine.model.as_deref().unwrap_or("active model")
                    );
                    let (summary, code) =
                        rigspark_cli::tui_chat::run_chat(&title, &engine, selection.color, &cancel)
                            .await?;
                    println!(
                        "Chat session ended: {} turn{}, {} memory warning{}.",
                        summary.turns,
                        if summary.turns == 1 { "" } else { "s" },
                        summary.memory_warnings,
                        if summary.memory_warnings == 1 {
                            ""
                        } else {
                            "s"
                        }
                    );
                    return Ok(code);
                }
                let mut signals = rigspark_cli::cancellation::TerminalSignals::new()?;
                let mode = if args.accessible {
                    Mode::Accessible
                } else {
                    Mode::Plain
                };
                use std::io::IsTerminal;
                if std::io::stdin().is_terminal() {
                    eprintln!("Chatting using {}. End input to exit.", engine.provider);
                }
                let mut stdout = std::io::stdout();
                let mut stderr = std::io::stderr();
                let operation = run_chat(
                    stdin_turns(),
                    &engine,
                    mode,
                    &cancel,
                    &mut stdout,
                    &mut stderr,
                );
                tokio::pin!(operation);
                let mut signal_exit = None;
                let summary = tokio::select! {
                    result = &mut operation => result?,
                    code = signals.recv() => {
                        signal_exit = Some(code?);
                        cancel.cancel();
                        operation.await?
                    },
                };
                if let Some(code) = signal_exit {
                    return Ok(code);
                }
                return Ok(if summary.cancelled {
                    130
                } else {
                    u8::from(summary.failed_turns > 0)
                });
            }
            let message = match args.message {
                Some(message) => message,
                None => {
                    use std::io::IsTerminal;
                    if std::io::stdin().is_terminal() {
                        return Err("JSON chat requires --message or piped stdin".into());
                    }
                    let mut message = String::new();
                    std::io::stdin()
                        .take(1024 * 1024 + 1)
                        .read_to_string(&mut message)?;
                    if message.len() > 1024 * 1024 {
                        return Err("chat input exceeds 1 MiB".into());
                    }
                    message
                }
            };
            let options = rigspark_runtime::native_chat::NativeChatOptions {
                provider: args.harness.unwrap_or_else(|| "local".into()),
                model: args.chat_model,
                message,
                agent: args.agent,
                skills: args.skills,
                capture: !args.no_memory,
            };
            let mut sink = |text: &str| {
                if !args.json {
                    std::io::stdout()
                        .write_all(strip_control(text).as_bytes())
                        .and_then(|_| std::io::stdout().flush())
                        .map_err(|_| rigspark_runtime::harness::HarnessError::Transport)?;
                }
                Ok(())
            };
            let operation = rigspark_runtime::native_chat::run(&options, &cancel, &mut sink);
            tokio::pin!(operation);
            let result = tokio::select! { result=&mut operation=>result?,_=tokio::signal::ctrl_c()=>{cancel.cancel();operation.await?} };
            if args.json {
                println!("{}", serde_json::to_string_pretty(&sanitized(&result))?);
            } else {
                println!();
                if result["memoryCaptured"] == false {
                    eprintln!("Memory capture failed; the reply was not recorded.");
                }
            }
            return Ok(0);
        }
        if args.yes && !args.move_memory {
            return Err("--yes requires --move".into());
        }
        let from = args.from.as_deref().ok_or("--from is required")?;
        let to = args.to.as_deref().ok_or("--to is required")?;
        if args.move_memory && !args.yes && !args.dry_run {
            return Err("--move requires explicit --yes".into());
        }
        let catalog_raw = load_catalog_raw(&args)?;
        let catalog = Catalog::parse(&catalog_raw)?;
        let context = match args.context {
            Some(context)
                if context.is_finite()
                    && context.fract() == 0.0
                    && (1.0..=10000000.0).contains(&context) =>
            {
                context as u32
            }
            Some(_) => return Err("invalid migration context".into()),
            None => resolve(&catalog, to)
                .map_err(|_| "target model is ambiguous or unknown; specify --context")?
                .model
                .context_length
                .ok_or("target context is unknown; specify --context")? as u32,
        };
        let from = resolve(&catalog, from)
            .map(|resolved| resolved.model.id.as_str())
            .unwrap_or(from);
        let to = resolve(&catalog, to)
            .map(|resolved| resolved.model.id.as_str())
            .unwrap_or(to);
        let stamp = rigspark_runtime::native_chat::timestamp()?;
        let request = rigspark_runtime::migration_service::MigrationRequest {
            source: from,
            target: to,
            context,
            dry_run: args.dry_run,
            move_source: args.move_memory,
            timestamp: &stamp,
            embedder: None,
            target_dimension: None,
        };
        let operation = rigspark_runtime::migration_service::run_native(request, &cancel);
        tokio::pin!(operation);
        let summary = tokio::select! { result=&mut operation=>result?,_=tokio::signal::ctrl_c()=>{cancel.cancel();operation.await?} };
        write_migration_result(
            from,
            to,
            args.dry_run,
            args.move_memory,
            &summary,
            args.json,
            &mut std::io::stdout(),
        )?;
        return Ok(0);
    }
    if !args.parity {
        if !["can-run", "plan", "up", "switch", "down"].contains(&args.command.as_str())
            && args.model.is_some()
        {
            return Err("this command does not accept a model argument".into());
        }
        if args.command != "catalog" && args.all {
            return Err("--all is only supported by catalog".into());
        }
        if args.command != "recommend"
            && (args.task.is_some()
                || args.context_percent.is_some()
                || args.max_context
                || args.available_backends)
        {
            return Err("recommendation options are not supported by this command".into());
        }
        if !["recommend", "can-run", "plan", "up", "switch"].contains(&args.command.as_str())
            && (args.context.is_some() || args.backend.is_some())
        {
            return Err("--context and --backend require recommend or can-run".into());
        }
        if ["can-run", "up", "switch"].contains(&args.command.as_str())
            && args.model.is_none()
            && (args.installed
                || !presentation.as_ref().is_some_and(|selection| {
                    matches!(
                        selection.mode,
                        rigspark_cli::tui_mode::Mode::Tui
                            | rigspark_cli::tui_mode::Mode::Accessible
                    )
                }))
        {
            return Err("model is required".into());
        }
        if !["up", "switch"].contains(&args.command.as_str())
            && (args.bypass
                || ((port.is_some() || args.installed)
                    && !(args.installed
                        && ["recommend", "can-run"].contains(&args.command.as_str()))))
        {
            return Err("runtime selection flags require up or switch".into());
        }
        if args.fits_only && !(args.command == "recommend" && args.installed) {
            return Err("--fits-only requires recommend --installed".into());
        }
        if args.installed
            && ["recommend", "can-run"].contains(&args.command.as_str())
            && (args.task.is_some()
                || args.context_percent.is_some()
                || args.max_context
                || args.available_backends
                || args
                    .backend
                    .as_ref()
                    .is_some_and(|backend| backend != "ollama"))
        {
            return Err("installed comparison only supports Ollama context options".into());
        }
    }
    if args.command == "ls" && !args.parity {
        let store =
            rigspark_runtime::state::StateStore::new(rigspark_runtime::state::Config::load()?);
        let (report, text) = match store.read()?.active {
            None => (json!({"type":"empty"}), "No active model.\n".to_owned()),
            Some(active) => {
                let mut report = json!({"type":"active","modelId":active.model_id,"backend":active.backend,"endpoint":active.endpoint,"port":active.port,"ownedByUs":active.owned_by_us});
                if let Some(id) = &active.runtime_model_id {
                    report["runtimeModelId"] = json!(id);
                }
                if let Some(context) = active.context {
                    report["context"] = json!(context);
                }
                if let Some(cache) = &active.cache {
                    report["cache"] = json!(cache);
                }
                let mut text = format!(
                    "{}\n",
                    rigspark_core::reports::table(
                        &[
                            ("Model", false),
                            ("Backend", false),
                            ("Endpoint", false),
                            ("Port", true),
                            ("Status", false)
                        ],
                        vec![vec![
                            active.model_id,
                            active.backend,
                            active.endpoint,
                            active.port.to_string(),
                            if active.owned_by_us {
                                "owned".into()
                            } else {
                                "attached".into()
                            }
                        ]]
                    )
                );
                if let Some(id) = active.runtime_model_id {
                    text.push_str(&format!("Runtime model: {}\n", strip_control(&id)));
                }
                if let Some(cache) = &active.cache {
                    text.push_str(&format!("Cache: {}\n", cache.summary()));
                }
                if let Some(context) = active.context {
                    text.push_str(&format!("Context: {context} tokens\n"));
                }
                (report, text)
            }
        };
        if args.json {
            println!("{}", serde_json::to_string_pretty(&sanitized(&report))?);
        } else {
            if presentation
                .as_ref()
                .is_some_and(|selection| selection.mode == rigspark_cli::tui_mode::Mode::Accessible)
            {
                let screen = rigspark_cli::accessible_read_only::active_server_screen(&report)?;
                let exit = show_accessible_screen(&screen).await?;
                if exit != 0 {
                    return Ok(exit);
                }
            }
            let presentation_exit =
                present_read_only(presentation.as_ref(), &presentation_title, &text).await?;
            if presentation_exit != 0 {
                return Ok(presentation_exit);
            }
        }
        return Ok(0);
    }
    let mut supplied_hardware = match (&args.hardware_json, &args.hardware) {
        (Some(_), Some(_)) => return Err("use either --hardware or --hardware-json".into()),
        (Some(raw), None) => Some(parse_hardware_input(raw)?),
        (None, Some(path)) => Some(parse_hardware_input(&read_bounded(path, 65536)?)?),
        (None, None) => None,
    };
    if args.command == "doctor" {
        let report = collect_doctor(&args, supplied_hardware).await;
        let exit = u8::from(report["ok"] == false);
        if args.json {
            println!("{}", serde_json::to_string_pretty(&sanitized(&report))?);
        } else {
            if presentation
                .as_ref()
                .is_some_and(|selection| selection.mode == rigspark_cli::tui_mode::Mode::Accessible)
            {
                let screen = rigspark_cli::accessible_read_only::doctor_screen(&report)?;
                let code = show_accessible_screen(&screen).await?;
                if code != 0 {
                    return Ok(code);
                }
            }
            let code = present_read_only(
                presentation.as_ref(),
                "doctor",
                &diagnostics::format_report(&report),
            )
            .await?;
            if code != 0 {
                return Ok(code);
            }
        }
        return Ok(exit);
    }
    let catalog_raw = load_catalog_raw(&args)?;
    let perf_raw = args
        .perf_path
        .as_ref()
        .map(read_file)
        .transpose()?
        .unwrap_or_else(|| rigspark_core::PERF_JSON.into());
    let mut catalog = Catalog::parse(&catalog_raw)?;
    if let Some(months) = args.month {
        let today = match &args.today {
            Some(day) => day.clone(),
            None => rigspark_runtime::native_chat::timestamp()?[..10].to_string(),
        };
        catalog.retain_recent(&today, months)?;
        if catalog.models.is_empty() {
            println!(
                "No catalog models were released or added in the last {months} month{}. Run `rigspark catalog --update` for the newest signed catalog, or drop --month.",
                if months == 1 { "" } else { "s" }
            );
            return Ok(0);
        }
    }
    let perf = PerfDataset::parse(&perf_raw)?;
    if args.command == "plan" {
        return run_plan(&args, &catalog, &perf, supplied_hardware.take()).await;
    }
    let mut cooked_input = None;
    if args.model.is_none()
        && ["can-run", "up", "switch"].contains(&args.command.as_str())
        && let Some(selection) = &presentation
        && matches!(
            selection.mode,
            rigspark_cli::tui_mode::Mode::Tui | rigspark_cli::tui_mode::Mode::Accessible
        )
    {
        let active = if args.command == "switch" {
            rigspark_runtime::state::StateStore::new(rigspark_runtime::state::Config::load()?)
                .read()?
                .active
        } else {
            None
        };
        let models = picker_models(&catalog, &args.command, active.as_ref());
        if models.is_empty() {
            return Ok(130);
        }
        let choices: Vec<_> = models
            .iter()
            .map(|model| {
                if selection.mode == rigspark_cli::tui_mode::Mode::Accessible {
                    model.id.clone()
                } else {
                    format!("{}  {}  {}", model.id, model.params, model.family)
                }
            })
            .collect();
        let (selected, code) = if selection.mode == rigspark_cli::tui_mode::Mode::Accessible {
            let input = cooked_input.get_or_insert_with(rigspark_cli::accessible::stdin_answers);
            let cancel = tokio_util::sync::CancellationToken::new();
            let title = format!("{} / choose model", args.command);
            let mut output = std::io::stderr();
            let selected = tokio::select! {
                biased;
                signal = rigspark_cli::tui_lifecycle::interruption() => {return Ok(signal?);},
                result = rigspark_cli::accessible::pick_model(&title,&choices,input,&mut output,&cancel) => result?,
            };
            (selected, 0)
        } else {
            rigspark_cli::tui_view::pick(
                &format!("{} / choose model", args.command),
                &choices,
                selection.color,
            )
            .await?
        };
        let Some(selected) = selected else {
            return Ok(if code == 0 { 130 } else { code });
        };
        args.model = Some(models[selected].id.clone());
    }
    if ["up", "switch", "down"].contains(&args.command.as_str()) && !args.parity {
        let options = args.lifecycle_options()?;
        rigspark_runtime::application::check_selection_availability(&options, &catalog)?;
        if let Some(selection) = &presentation
            && !(options.command == "down" && args.yes)
            && matches!(
                selection.mode,
                rigspark_cli::tui_mode::Mode::Tui | rigspark_cli::tui_mode::Mode::Accessible
            )
        {
            let description = format!(
                "Proceed: {} {} / backend {} / port {} / context {}{}",
                options.command,
                options.model.as_deref().unwrap_or("owned servers"),
                options.backend.as_deref().unwrap_or("default"),
                options
                    .port
                    .map_or_else(|| "default".into(), |port| port.to_string()),
                options
                    .context
                    .map_or_else(|| "default".into(), |context| context.to_string()),
                if options.bypass {
                    " / hardware fit bypass enabled"
                } else {
                    ""
                }
            );
            let (selected, code) = if selection.mode == rigspark_cli::tui_mode::Mode::Accessible {
                let input =
                    cooked_input.get_or_insert_with(rigspark_cli::accessible::stdin_answers);
                let cancel = tokio_util::sync::CancellationToken::new();
                let mut output = std::io::stderr();
                let lines = vec![description];
                let title = if options.command == "down" {
                    "Confirm shutdown"
                } else {
                    "Confirm activation"
                };
                let accepted = tokio::select! {
                    biased;
                    signal = rigspark_cli::tui_lifecycle::interruption() => {return Ok(signal?);},
                    result = rigspark_cli::accessible::confirm(&options.command,title,&lines,"Proceed",input,&mut output,&cancel) => result?,
                };
                (if accepted { Some(1) } else { None }, 0)
            } else {
                rigspark_cli::tui_view::pick(
                    &format!("{} / confirm", options.command),
                    &["Cancel".into(), description],
                    selection.color,
                )
                .await?
            };
            if selected != Some(1) {
                return Ok(if code == 0 { 130 } else { code });
            }
        }
        let hardware = if options.command == "down" || options.installed {
            None
        } else if let Some(hardware) = supplied_hardware.take() {
            Some(hardware)
        } else {
            let (hardware, warnings) = tokio::select! {
                biased;
                signal = rigspark_cli::tui_lifecycle::interruption() => return Ok(signal?),
                result = detect() => result?,
            };
            for warning in warnings {
                eprintln!("{}", strip_control(&warning));
            }
            Some(hardware)
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        let (observer, events) =
            rigspark_runtime::application::events::LifecycleObserver::channel();
        let visual = presentation
            .as_ref()
            .filter(|selection| selection.mode == rigspark_cli::tui_mode::Mode::Tui);
        let diagnostics = rigspark_runtime::application::events::DiagnosticObserver::default();
        let observer = if visual.is_some() {
            observer.with_diagnostics(diagnostics.clone())
        } else {
            observer
        };
        let operation = rigspark_runtime::application::run_native_observed(
            &options,
            &catalog,
            hardware.as_ref(),
            &cancel,
            Some(&observer),
        );
        let interactive = presentation.as_ref().is_some_and(|selection| {
            matches!(
                selection.mode,
                rigspark_cli::tui_mode::Mode::Tui | rigspark_cli::tui_mode::Mode::Accessible
            )
        });
        let outcome = if let Some(selection) = visual {
            rigspark_cli::tui_lifecycle::run_visual(
                &options.command,
                options.model.as_deref().unwrap_or("recorded servers"),
                operation,
                &cancel,
                selection.color,
                events,
                diagnostics,
            )
            .await?
        } else {
            rigspark_cli::tui_lifecycle::run_observed(
                &options.command,
                options.model.as_deref().unwrap_or("recorded servers"),
                operation,
                &cancel,
                interactive,
                Some(events),
            )
            .await?
        };
        let evidence = rigspark_cli::tui_lifecycle::evidence(&outcome);
        let result_view = if let Some(selection) = &presentation
            && selection.mode == rigspark_cli::tui_mode::Mode::Tui
            && ![129, 130, 143].contains(&outcome.exit_code)
            && !outcome.presentation_restored
            && outcome.presentation_error.is_none()
        {
            rigspark_cli::tui_view::show_report(
                &format!(
                    "{} / {}",
                    options.command,
                    if outcome.result.is_ok() {
                        "result"
                    } else {
                        "recovery"
                    }
                ),
                &evidence,
                selection.color,
            )
            .await
        } else {
            Ok(0)
        };
        return write_lifecycle_result(
            &options.command,
            outcome,
            result_view,
            interactive,
            args.json,
            &mut std::io::stdout(),
            &mut std::io::stderr(),
        );
    }
    if args.parity {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err("parity input exceeds 8 MiB".into());
        }
        let requests: Vec<ParityRequest> = serde_json::from_slice(&bytes)?;
        if requests.len() > 1024 {
            return Err("too many parity requests".into());
        }
        let mut outputs = Vec::new();
        for request in requests {
            validate_hardware(&request.hardware)?;
            let recommendation = recommend(&catalog, &request.hardware, &perf, &request.options)?;
            let mut checks = Vec::new();
            for query in request.queries.unwrap_or_default() {
                checks.push(match resolve(&catalog, &query) {
                    Ok(resolved) => {
                        let (report, text) =
                            can_run(&catalog, &request.hardware, &perf, &query, &request.options)?;
                        json!({"resolved":resolved,"report":report,"text":text})
                    }
                    Err(error) => json!({"error":error}),
                });
            }
            let verdicts: Vec<_> = catalog
                .models
                .iter()
                .map(|model| {
                    verdict(
                        model,
                        &request.hardware,
                        &perf,
                        request.options.tokens(model),
                        request.options.backend.as_deref().unwrap_or("ollama"),
                    )
                })
                .collect::<Result<_, _>>()?;
            outputs.push(json!({"recommendation":sanitized(&recommendation),"text":recommendation_text(&recommendation,&request.options),"score":hardware_score(&request.hardware),"catalogText":catalog_text(&catalog,&request.hardware,true)?,"checks":checks,"verdicts":verdicts}));
        }
        serde_json::to_writer(std::io::stdout().lock(), &outputs)?;
        println!();
        return Ok(0);
    }
    let hardware = if let Some(hardware) = supplied_hardware {
        hardware
    } else {
        let (hardware, warnings) = detect().await?;
        for warning in warnings {
            eprintln!("{}", strip_control(&warning));
        }
        hardware
    };
    let backends = if args.available_backends {
        Some(diagnostics::probe_backends(&hardware).await)
    } else {
        None
    };
    let options = match args.advice_request(backends)? {
        AdviceRequest::Installed {
            model,
            port,
            context,
            fits_only,
        } => {
            let cancel = tokio_util::sync::CancellationToken::new();
            let operation = rigspark_runtime::application::installed_inventory(
                &hardware,
                model.as_deref(),
                port,
                context,
                fits_only,
                &cancel,
            );
            tokio::pin!(operation);
            let (report, text, exit) = tokio::select! {result=&mut operation=>result?,_=tokio::signal::ctrl_c()=>{cancel.cancel();operation.await?}};
            if args.json {
                println!("{}", serde_json::to_string_pretty(&sanitized(&report))?);
            } else {
                if let Some(selection) = presentation.as_ref().filter(|selection| {
                    matches!(
                        selection.mode,
                        rigspark_cli::tui_mode::Mode::Accessible
                            | rigspark_cli::tui_mode::Mode::Tui
                    )
                }) {
                    use rigspark_cli::accessible_installed::{InstalledCommand, build_installed};
                    let command = if args.command == "can-run" {
                        InstalledCommand::CanRun
                    } else {
                        InstalledCommand::Recommend
                    };
                    let view = build_installed(&report, command)?;
                    let presentation_exit = if selection.mode == rigspark_cli::tui_mode::Mode::Tui {
                        show_model_view(rigspark_cli::tui_models::ModelView::from_installed(
                            &view,
                            selection.color,
                        )?)
                        .await?
                    } else {
                        show_accessible_list(&AccessibleList::Installed(view)).await?
                    };
                    if presentation_exit != 0 {
                        return Ok(presentation_exit);
                    }
                    std::io::stdout().lock().write_all(text.as_bytes())?;
                    return Ok(exit);
                }
                let presentation_exit =
                    present_read_only(presentation.as_ref(), &presentation_title, &text).await?;
                if presentation_exit != 0 {
                    return Ok(presentation_exit);
                }
            }
            return Ok(exit);
        }
        AdviceRequest::Catalog(options) => options,
    };
    let mut accessible_can_run = None;
    let mut accessible_list = None;
    let mut visual_can_run = None;
    let is_accessible = presentation
        .as_ref()
        .is_some_and(|selection| selection.mode == rigspark_cli::tui_mode::Mode::Accessible);
    let visual = presentation
        .as_ref()
        .filter(|selection| selection.mode == rigspark_cli::tui_mode::Mode::Tui);
    let detailed_models = is_accessible || visual.is_some();
    let (report, text, exit) = match args.command.as_str() {
        "recommend" => {
            if detailed_models {
                let view = rigspark_cli::accessible_recommend::build_recommendation(
                    &catalog, &hardware, &perf, &options,
                )?;
                let text = format!("{}\n", view.final_text());
                accessible_list = Some(AccessibleList::Recommend(view));
                (Value::Null, text, 0)
            } else {
                let report = recommend(&catalog, &hardware, &perf, &options)?;
                let text = recommendation_text(&report, &options);
                (report, format!("{text}\n"), 0)
            }
        }
        "can-run" => {
            let query = args.model.as_deref().ok_or("model is required")?;
            let report = rigspark_core::reports::can_run_report(
                &catalog, &hardware, &perf, query, &options,
            )?;
            if presentation
                .as_ref()
                .is_some_and(|selection| selection.mode == rigspark_cli::tui_mode::Mode::Accessible)
            {
                accessible_can_run = Some(rigspark_cli::accessible_read_only::can_run_screen(
                    &report.evidence,
                )?);
            }
            if let Some(selection) = visual {
                visual_can_run = Some(rigspark_cli::tui_models::ModelView::from_can_run(
                    &report.evidence,
                    selection.color,
                )?);
            }
            let exit = u8::from(report.json["verdict"] == "no");
            (report.json, format!("{}\n", report.text), exit)
        }
        "catalog" => {
            if args.json {
                return Err("catalog --json is not part of the existing CLI contract".into());
            }
            let text = if args.refresh {
                use rigspark_core::enrich::{Mode, enrich, format_diff, parse_candidates};
                let candidates = parse_candidates(rigspark_core::REGISTRY_SNAPSHOT_JSON)?;
                let result = enrich(
                    &catalog,
                    &candidates,
                    Mode::Incremental,
                    &rigspark_runtime::native_chat::timestamp()?,
                    None,
                )?;
                if detailed_models {
                    let diff = &result.diff;
                    let options = rigspark_cli::accessible_catalog::CatalogOptions {
                        all: args.all,
                        refresh: Some(rigspark_cli::accessible_catalog::CatalogRefresh {
                            added: diff.added.clone(),
                            updated: diff.updated.clone(),
                            removed: diff.removed.clone(),
                            skipped: diff.skipped.clone(),
                            capped: diff.capped.clone(),
                        }),
                    };
                    accessible_list = Some(AccessibleList::Catalog(
                        rigspark_cli::accessible_catalog::build_catalog(
                            &result.catalog,
                            &hardware,
                            &options,
                        )?,
                    ));
                }
                format!(
                    "{}{}",
                    format_diff(&result.diff),
                    catalog_text(&result.catalog, &hardware, args.all)?
                )
            } else {
                if detailed_models {
                    accessible_list = Some(AccessibleList::Catalog(
                        rigspark_cli::accessible_catalog::build_catalog(
                            &catalog,
                            &hardware,
                            &rigspark_cli::accessible_catalog::CatalogOptions {
                                all: args.all,
                                ..Default::default()
                            },
                        )?,
                    ));
                }
                catalog_text(&catalog, &hardware, args.all)?
            };
            (Value::Null, text, 0)
        }
        _ => return Err("unsupported command".into()),
    };
    if let Some(screen) = accessible_can_run {
        let input = cooked_input.get_or_insert_with(rigspark_cli::accessible::stdin_answers);
        let presentation_exit = show_accessible_screen_with_input(&screen, input).await?;
        if presentation_exit != 0 {
            return Ok(presentation_exit);
        }
    }
    let mut models_presented = false;
    if let Some(view) = visual_can_run {
        let code = show_model_view(view).await?;
        if code != 0 {
            return Ok(code);
        }
        models_presented = true;
    }
    if let Some(list) = accessible_list {
        let presentation_exit = if let Some(selection) = visual {
            use rigspark_cli::tui_models::ModelView;
            let dates: std::collections::BTreeMap<String, String> = catalog
                .models
                .iter()
                .filter_map(|model| Some((model.id.clone(), model.recency()?.0.to_string())))
                .collect();
            let today = match &args.today {
                Some(day) => day.clone(),
                None => rigspark_runtime::native_chat::timestamp()?[..10].to_string(),
            };
            let view = match &list {
                AccessibleList::Catalog(view) => {
                    ModelView::from_catalog(view, selection.color)?.with_recency(&dates, &today)?
                }
                AccessibleList::Recommend(view) => {
                    ModelView::from_recommendation(view, selection.color)?
                        .with_recency(&dates, &today)?
                }
                AccessibleList::Installed(view) => {
                    ModelView::from_installed(view, selection.color)?
                }
            };
            models_presented = true;
            show_model_view(view).await?
        } else {
            show_accessible_list(&list).await?
        };
        if presentation_exit != 0 {
            return Ok(presentation_exit);
        }
    }
    let output = if args.json {
        format!("{}\n", serde_json::to_string_pretty(&sanitized(&report))?)
    } else {
        text
    };
    let presentation_exit = present_read_only(
        if models_presented {
            None
        } else {
            presentation.as_ref()
        },
        &presentation_title,
        &output,
    )
    .await?;
    if presentation_exit != 0 {
        return Ok(presentation_exit);
    }
    Ok(exit)
}

async fn collect_doctor(args: &Args, supplied: Option<Hardware>) -> Value {
    let hardware = match supplied {
        Some(hardware) => Ok(hardware),
        None => detect()
            .await
            .map(|(hardware, _)| hardware)
            .map_err(|error| error.to_string()),
    };
    let catalog = load_catalog_raw(args)
        .map_err(|error| error.to_string())
        .and_then(|raw| Catalog::parse(&raw).map_err(|error| error.to_string()));
    let backends = diagnostics::probe_available_backends(hardware.as_ref().ok()).await;
    let home = std::env::var_os("RIGSPARK_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| PathBuf::from(home).join(".rigspark"))
        });
    let exists = home
        .as_ref()
        .is_none_or(|home| home.join("state.json").try_exists().unwrap_or(true));
    let mut report = diagnostics::report_inputs(
        catalog.as_ref().map_err(String::as_str),
        hardware.as_ref().map_err(String::as_str),
        &backends,
        exists,
    );
    let state_check = if let Ok(catalog) = &catalog {
        let options = rigspark_runtime::application::LifecycleOptions {
            command: "doctor".into(),
            model: None,
            backend: None,
            port: None,
            context: None,
            installed: false,
            bypass: false,
            cache: Default::default(),
        };
        match rigspark_runtime::application::run_native(
            &options,
            catalog,
            None,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        {
            Ok((check, _)) => check,
            Err(error) => {
                json!({"name":"state","status":"fail","detail":format!("runtime state/readiness failed: {}",strip_control(&error.to_string()))})
            }
        }
    } else if exists {
        json!({"name":"state","status":"warn","detail":"catalog unavailable; active-server identity and readiness not checked"})
    } else {
        json!({"name":"state","status":"ok","detail":"no active server recorded"})
    };
    if let Some(checks) = report["checks"].as_array_mut() {
        if let Some(check) = checks.iter_mut().find(|check| check["name"] == "state") {
            *check = state_check;
        }
        let ok = checks.iter().all(|check| check["status"] != "fail");
        report["ok"] = json!(ok);
    }
    report
}

async fn show_accessible_screen(screen: &str) -> std::io::Result<u8> {
    let mut input = rigspark_cli::accessible::stdin_answers();
    show_accessible_screen_with_input(screen, &mut input).await
}

async fn show_model_view(view: rigspark_cli::tui_models::ModelView) -> std::io::Result<u8> {
    use rigspark_cli::tui_models::{ModelOutcome, show_models};
    let result = show_models(view).await.map(|outcome| match outcome {
        ModelOutcome::Exit { code } => code,
        ModelOutcome::PrintCommand { .. } => 0,
    });
    recover_read_only_presentation(result, &mut std::io::stderr())
}

enum AccessibleList {
    Catalog(rigspark_cli::accessible_catalog::CatalogPresentation),
    Recommend(rigspark_cli::accessible_recommend::Recommendation),
    Installed(rigspark_cli::accessible_installed::InstalledView),
}

async fn show_accessible_list(list: &AccessibleList) -> std::io::Result<u8> {
    let mut signals = rigspark_cli::cancellation::TerminalSignals::new()?;
    let mut input = rigspark_cli::accessible::stdin_answers();
    let mut output = std::io::stderr();
    let cancel = tokio_util::sync::CancellationToken::new();
    let operation = async {
        match list {
            AccessibleList::Installed(view) => {
                use rigspark_cli::accessible_installed::{InstalledOutcome, run_installed};
                Ok(
                    match run_installed(view, &mut input, &mut output, &cancel).await? {
                        InstalledOutcome::Cancelled => 130,
                        InstalledOutcome::Exited => 0,
                    },
                )
            }
            AccessibleList::Catalog(view) => rigspark_cli::accessible_catalog::run_catalog(
                view,
                &mut input,
                &mut output,
                &cancel,
            )
            .await
            .map(|()| 0),
            AccessibleList::Recommend(view) => {
                use rigspark_cli::accessible_recommend::{RecommendOutcome, run_recommendation};
                Ok(
                    match run_recommendation(view, &mut input, &mut output, &cancel).await? {
                        RecommendOutcome::Cancelled => 130,
                        RecommendOutcome::Exited | RecommendOutcome::PrintCommand { .. } => 0,
                    },
                )
            }
        }
    };
    let result = tokio::select! {
        biased;
        code=signals.recv()=>{cancel.cancel();code},
        result=operation=>result,
    };
    recover_read_only_presentation(result, &mut output)
}

async fn show_accessible_screen_with_input(
    screen: &str,
    input: &mut tokio::sync::mpsc::Receiver<std::io::Result<String>>,
) -> std::io::Result<u8> {
    let mut output = std::io::stderr();
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut signals = rigspark_cli::cancellation::TerminalSignals::new()?;
    let result = tokio::select! {
        biased;
        code=signals.recv()=>{cancel.cancel();code},
        result=rigspark_cli::accessible_read_only::run_screen(screen,input,&mut output,&cancel)=>result.map(|()|0),
    };
    recover_read_only_presentation(result, &mut output)
}

fn write_command_error(command: &str, error: &str, stderr: &mut impl Write) -> std::io::Result<()> {
    let command = strip_control(command);
    let error = strip_control(error);
    if error.starts_with(&format!("{command}: ")) {
        writeln!(stderr, "{error}")
    } else {
        writeln!(stderr, "{command}: {error}")
    }
}

fn write_migration_result(
    from: &str,
    to: &str,
    dry_run: bool,
    move_source: bool,
    summary: &rigspark_runtime::memory::MigrationSummary,
    json_mode: bool,
    stdout: &mut impl Write,
) -> Result<(), Box<dyn std::error::Error>> {
    if json_mode {
        writeln!(
            stdout,
            "{}",
            serde_json::to_string_pretty(&sanitized(&json!({
                "from": from, "to": to, "dryRun": dry_run, "move": move_source, "summary": summary
            })))?
        )?;
    } else {
        let heading = if dry_run {
            "[dry-run] Planned migration"
        } else {
            "Migrated memory"
        };
        let suffix = if move_source && !dry_run {
            " (source removed)"
        } else {
            ""
        };
        writeln!(
            stdout,
            "{heading}: {} -> {}{suffix}",
            strip_control(from),
            strip_control(to)
        )?;
        writeln!(stdout, "  turns carried:       {}", summary.turns_carried)?;
        writeln!(
            stdout,
            "  turns summarized:    {}",
            summary.turns_summarized
        )?;
        writeln!(
            stdout,
            "  vectors re-embedded: {}",
            summary.vectors_reembedded
        )?;
        writeln!(
            stdout,
            "  context strategy:    {}",
            strip_control(&summary.strategy)
        )?;
        writeln!(
            stdout,
            "  embedding strategy:  {}",
            strip_control(&summary.embedding_strategy)
        )?;
    }
    Ok(())
}

fn write_lifecycle_result(
    command: &str,
    mut outcome: rigspark_cli::tui_lifecycle::Outcome,
    result_view: std::io::Result<u8>,
    interactive: bool,
    json: bool,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> Result<u8, Box<dyn std::error::Error>> {
    let presentation_failed = outcome.presentation_error.take().is_some() || result_view.is_err();
    let evidence = rigspark_cli::tui_lifecycle::evidence(&outcome);
    let mut exit = outcome.exit_code;
    if let Ok(code) = result_view
        && code != 0
    {
        exit = code;
    }
    if presentation_failed {
        writeln!(stderr, "{command}: Lifecycle display failed.")?;
        if exit == 0 {
            exit = 1;
        }
    }
    let (report, text) = match outcome.result {
        Ok(result) => result,
        Err(error) => {
            if !interactive && !presentation_failed && exit == 1 {
                write_command_error(command, &error.to_string(), stderr)?;
            } else {
                write!(stderr, "{evidence}")?;
            }
            return Ok(exit.max(1));
        }
    };
    if interactive || exit != 0 {
        write!(stderr, "{evidence}")?;
    }
    if exit != 0 {
        return Ok(exit);
    }
    if json {
        writeln!(
            stdout,
            "{}",
            serde_json::to_string_pretty(&sanitized(&report))?
        )?;
    } else {
        write!(stdout, "{text}")?;
    }
    Ok(exit)
}

fn recover_read_only_presentation(
    result: std::io::Result<u8>,
    stderr: &mut impl Write,
) -> std::io::Result<u8> {
    match result {
        Ok(exit) => Ok(exit),
        Err(_) => {
            writeln!(
                stderr,
                "rigspark: interactive UI failed (renderer_runtime); terminal restored; final result follows"
            )?;
            Ok(0)
        }
    }
}

async fn present_read_only(
    selection: Option<&rigspark_cli::tui_mode::Selection>,
    title: &str,
    text: &str,
) -> Result<u8, Box<dyn std::error::Error>> {
    if let Some(selection) = selection
        && selection.mode == rigspark_cli::tui_mode::Mode::Tui
    {
        let result = rigspark_cli::tui_view::show_report(title, text, selection.color).await;
        let exit = recover_read_only_presentation(result, &mut std::io::stderr())?;
        if exit != 0 {
            return Ok(exit);
        }
    }
    std::io::stdout().lock().write_all(text.as_bytes())?;
    Ok(0)
}

// Windows main threads get 1 MiB, too small for the CLI future; run it on an explicitly sized thread.
const MAIN_STACK_BYTES: usize = 16 * 1024 * 1024;

fn main() -> ExitCode {
    let worker = std::thread::Builder::new()
        .name("llmup-main".into())
        .stack_size(MAIN_STACK_BYTES)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map(|runtime| runtime.block_on(run()))
        });
    match worker.map(std::thread::JoinHandle::join) {
        Ok(Ok(Ok(code))) => code,
        Ok(Err(panic)) => std::panic::resume_unwind(panic),
        Ok(Ok(Err(error))) => {
            let _ = writeln!(std::io::stderr(), "llmup: cannot start runtime: {error}");
            ExitCode::from(1)
        }
        Err(error) => {
            let _ = writeln!(
                std::io::stderr(),
                "llmup: cannot start main thread: {error}"
            );
            ExitCode::from(1)
        }
    }
}

async fn run() -> ExitCode {
    let args = match native_args::parse() {
        Ok(Some(args)) => args,
        Ok(None) => return ExitCode::SUCCESS,
        Err(error) => {
            let _ = write_command_error(
                &native_args::error_command(),
                &error.to_string(),
                &mut std::io::stderr(),
            );
            return ExitCode::from(1);
        }
    };
    let command = args.command.clone();
    match execute(args).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            let _ = write_command_error(&command, &error.to_string(), &mut std::io::stderr());
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod lifecycle_output_tests {
    use super::*;

    #[test]
    fn read_only_renderer_failure_recovers_without_leaking_error_or_masking_signals() {
        for (result, expected, notice) in [
            (Ok(0), 0, false),
            (Ok(130), 130, false),
            (Ok(143), 143, false),
            (Err(std::io::Error::other("untrusted\n\x1b[31m")), 0, true),
        ] {
            let mut stderr = Vec::new();
            assert_eq!(
                recover_read_only_presentation(result, &mut stderr).unwrap(),
                expected
            );
            assert_eq!(
                String::from_utf8(stderr).unwrap(),
                if notice {
                    "rigspark: interactive UI failed (renderer_runtime); terminal restored; final result follows\n"
                } else {
                    ""
                }
            );
        }
    }

    #[test]
    fn command_errors_have_exact_prefix_and_newline_without_duplicate_prefixes() {
        for command in [
            "recommend",
            "can-run",
            "up",
            "chat",
            "gui",
            "down",
            "switch",
            "migrate",
            "ls",
            "catalog",
            "doctor",
        ] {
            for message in [
                "contract failure".to_owned(),
                format!("{command}: contract failure"),
                "\u{1b}[31mcontract failure\u{1b}[0m\n".into(),
            ] {
                let mut stderr = Vec::new();
                write_command_error(command, &message, &mut stderr).unwrap();
                assert_eq!(
                    String::from_utf8(stderr).unwrap(),
                    format!("{command}: contract failure\n")
                );
            }
        }
    }

    #[test]
    fn noninteractive_lifecycle_success_writes_one_report_and_no_stderr() {
        for (command, kind, text) in [
            ("up", "active", "Model active.\n"),
            ("switch", "already-active", "Model already active.\n"),
            ("down", "no-active", "No active server to stop.\n"),
        ] {
            for json_mode in [false, true] {
                let mut outcome = success(0, None);
                outcome.result = Ok((json!({"type": kind}), text.into()));
                let mut stdout = Vec::new();
                let mut stderr = Vec::new();
                assert_eq!(
                    write_lifecycle_result(
                        command,
                        outcome,
                        Ok(0),
                        false,
                        json_mode,
                        &mut stdout,
                        &mut stderr
                    )
                    .unwrap(),
                    0
                );
                assert!(stderr.is_empty());
                if json_mode {
                    assert_eq!(
                        serde_json::from_slice::<Value>(&stdout).unwrap(),
                        json!({"type": kind})
                    );
                } else {
                    assert_eq!(stdout, text.as_bytes());
                }
            }
        }
    }

    fn success(
        exit_code: u8,
        presentation_error: Option<String>,
    ) -> rigspark_cli::tui_lifecycle::Outcome {
        rigspark_cli::tui_lifecycle::Outcome {
            result: Ok((
                json!({"type": "no-active"}),
                "No active server to stop.\n".into(),
            )),
            exit_code,
            presentation_error,
            diagnostics: None,
            presentation_restored: false,
        }
    }

    #[test]
    fn interrupted_results_keep_evidence_without_success_stdout() {
        for json in [false, true] {
            for code in [130, 143] {
                for (operation_exit, view_exit) in [(code, 0), (0, code)] {
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    let exit = write_lifecycle_result(
                        "down",
                        success(operation_exit, None),
                        Ok(view_exit),
                        true,
                        json,
                        &mut stdout,
                        &mut stderr,
                    )
                    .unwrap();
                    assert_eq!(exit, code);
                    assert!(stdout.is_empty());
                    assert!(
                        String::from_utf8(stderr)
                            .unwrap()
                            .contains("Result: no-active")
                    );
                }
            }
        }
    }

    #[test]
    fn result_display_failure_is_safe_and_has_no_success_stdout() {
        for json in [false, true] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = write_lifecycle_result(
                "down",
                success(0, None),
                Err(std::io::Error::other("private renderer detail")),
                true,
                json,
                &mut stdout,
                &mut stderr,
            )
            .unwrap();
            assert_eq!(exit, 1);
            assert!(stdout.is_empty());
            let stderr = String::from_utf8(stderr).unwrap();
            assert!(stderr.contains("Result: no-active"));
            assert_eq!(stderr.matches("down: ").count(), 1);
            assert!(!stderr.contains("private renderer detail"), "{stderr}");
        }
    }

    #[test]
    fn successful_result_quit_preserves_plain_and_json_stdout() {
        for json in [false, true] {
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = write_lifecycle_result(
                "down",
                success(0, None),
                Ok(0),
                true,
                json,
                &mut stdout,
                &mut stderr,
            )
            .unwrap();
            assert_eq!(exit, 0);
            if json {
                assert_eq!(
                    serde_json::from_slice::<Value>(&stdout).unwrap()["type"],
                    "no-active"
                );
            } else {
                assert_eq!(stdout, b"No active server to stop.\n");
            }
        }
    }

    #[test]
    fn operation_display_failure_retains_runtime_evidence_not_renderer_details() {
        for json in [false, true] {
            for runtime_failed in [false, true] {
                for code in [1, 130, 143] {
                    let mut outcome =
                        success(code, Some("private operation renderer detail".into()));
                    if runtime_failed {
                        outcome.result = Err(rigspark_runtime::adapters::BackendError(
                            "runtime cleanup failed".into(),
                        ));
                    }
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    let exit = write_lifecycle_result(
                        "down",
                        outcome,
                        Ok(0),
                        true,
                        json,
                        &mut stdout,
                        &mut stderr,
                    )
                    .unwrap();
                    assert_eq!(exit, code);
                    assert!(stdout.is_empty());
                    let stderr = String::from_utf8(stderr).unwrap();
                    assert!(
                        !stderr.contains("private operation renderer detail"),
                        "{stderr}"
                    );
                    assert_eq!(stderr.matches("down: ").count(), 1, "{stderr}");
                    if runtime_failed {
                        assert!(
                            stderr.contains("Runtime error: runtime cleanup failed"),
                            "{stderr}"
                        );
                        assert!(
                            stderr.contains("cleanup success is not confirmed"),
                            "{stderr}"
                        );
                    } else {
                        assert!(stderr.contains("Result: no-active"), "{stderr}");
                    }
                }
            }
        }
    }

    #[test]
    fn recovery_view_exit_does_not_hide_runtime_failure() {
        for view_exit in [0, 130, 143] {
            let mut outcome = success(1, None);
            outcome.result = Err(rigspark_runtime::adapters::BackendError(
                "runtime cleanup failed".into(),
            ));
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let exit = write_lifecycle_result(
                "down",
                outcome,
                Ok(view_exit),
                true,
                false,
                &mut stdout,
                &mut stderr,
            )
            .unwrap();
            assert_eq!(exit, if view_exit == 0 { 1 } else { view_exit });
            assert!(stdout.is_empty());
            assert!(
                String::from_utf8(stderr)
                    .unwrap()
                    .contains("Runtime error: runtime cleanup failed")
            );
        }
    }

    #[test]
    fn ordinary_lifecycle_errors_are_one_prefixed_line_without_stdout() {
        for command in ["up", "switch", "down"] {
            for json in [false, true] {
                for message in [
                    "contract failure".to_owned(),
                    format!("{command}: contract failure"),
                ] {
                    let mut outcome = success(1, None);
                    outcome.result = Err(rigspark_runtime::adapters::BackendError(message));
                    let mut stdout = Vec::new();
                    let mut stderr = Vec::new();
                    let exit = write_lifecycle_result(
                        command,
                        outcome,
                        Ok(0),
                        false,
                        json,
                        &mut stdout,
                        &mut stderr,
                    )
                    .unwrap();
                    assert_eq!(exit, 1);
                    assert!(stdout.is_empty());
                    assert_eq!(
                        String::from_utf8(stderr).unwrap(),
                        format!("{command}: contract failure\n")
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod request_tests {
    use super::*;

    #[test]
    fn picker_filters_switch_targets_without_changing_other_commands() {
        let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
        let first = &catalog.models[0];
        let mut active: rigspark_runtime::state::ServerState = serde_json::from_value(json!({
            "backend": "ollama", "modelId": first.id, "endpoint": "http://127.0.0.1:11434",
            "port": 11434, "ownedByUs": false
        }))
        .unwrap();
        let choices = picker_models(&catalog, "switch", Some(&active));
        assert_eq!(choices.len(), catalog.models.len() - 1);
        assert_eq!(choices[0].id, catalog.models[1].id);
        assert!(choices.iter().all(|model| model.id != first.id));
        for backend in ["llamacpp", "mlx", "lmstudio"] {
            active.backend = backend.into();
            assert!(picker_models(&catalog, "switch", Some(&active)).is_empty());
            assert_eq!(
                picker_models(&catalog, "up", Some(&active)).len(),
                catalog.models.len()
            );
        }
        assert_eq!(
            picker_models(&catalog, "switch", None).len(),
            catalog.models.len()
        );
    }

    #[test]
    fn installed_requests_select_inventory_with_exact_options() {
        for (flags, expected_model, expected_port, expected_fits) in [
            (
                vec![
                    "llmup",
                    "can-run",
                    "gemma4:e4b-it-qat",
                    "--installed",
                    "--context",
                    "65536",
                    "--port",
                    "11435",
                ],
                Some("gemma4:e4b-it-qat"),
                11435,
                false,
            ),
            (
                vec![
                    "llmup",
                    "recommend",
                    "--installed",
                    "--context",
                    "65536",
                    "--fits-only",
                ],
                None,
                11434,
                true,
            ),
        ] {
            let request = Args::try_parse_from(flags)
                .unwrap()
                .advice_request(None)
                .unwrap();
            let AdviceRequest::Installed {
                model,
                port,
                context,
                fits_only,
            } = request
            else {
                panic!("installed advice must not select catalog advice");
            };
            assert_eq!(model.as_deref(), expected_model);
            assert_eq!(port, expected_port);
            assert_eq!(context, Some(65536));
            assert_eq!(fits_only, expected_fits);
        }
    }

    #[test]
    fn backend_and_available_backends_reach_catalog_advice_together() {
        for available in [false, true] {
            let mut flags = vec!["llmup", "recommend", "--backend", "llamacpp"];
            if available {
                flags.push("--available-backends");
            }
            let backends = [("ollama", false), ("llamacpp", true)]
                .into_iter()
                .map(|(name, installed)| diagnostics::BackendInfo {
                    name: name.into(),
                    installed,
                    version: None,
                    is_default: false,
                    install_hint: String::new(),
                })
                .collect();
            let AdviceRequest::Catalog(options) = Args::try_parse_from(flags)
                .unwrap()
                .advice_request(Some(backends))
                .unwrap()
            else {
                panic!("catalog advice must not select installed inventory");
            };
            assert_eq!(options.backend.as_deref(), Some("llamacpp"));
            assert_eq!(
                options.available_backends,
                available.then(|| vec!["llamacpp".into()])
            );
            let catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
            let perf = PerfDataset::parse(rigspark_core::PERF_JSON).unwrap();
            let hardware = parse_hardware_input(r#"{"arch":"x64","platform":"linux","totalRamBytes":68719476736,"freeRamBytes":64424509440,"freeDiskBytes":536870912000,"gpu":[]}"#).unwrap();
            let report = recommend(&catalog, &hardware, &perf, &options).unwrap();
            let ranked = report["ranked"].as_array().unwrap();
            assert!(!ranked.is_empty());
            assert!(
                ranked
                    .iter()
                    .all(|entry| entry["throughputBackend"] == "llamacpp")
            );
            if available {
                assert!(ranked.iter().all(|entry| {
                    entry["backends"]
                        .as_array()
                        .unwrap()
                        .contains(&json!("llamacpp"))
                }));
            }
        }
    }

    #[test]
    fn lifecycle_request_preserves_model_bypass_context_and_boundary_ports() {
        for command in ["up", "switch"] {
            for port in [1, 65535] {
                let args = Args::try_parse_from([
                    "llmup",
                    command,
                    "gemma4:e4b-it-qat",
                    "--bypass",
                    "--context",
                    "65536",
                    "--port",
                    &port.to_string(),
                ])
                .unwrap();
                let request = args.lifecycle_options().unwrap();
                assert_eq!(request.command, command);
                assert_eq!(request.model.as_deref(), Some("gemma4:e4b-it-qat"));
                assert_eq!(request.port, Some(port));
                assert_eq!(request.context, Some(65536));
                assert!(request.bypass);
                assert!(!request.installed);
                assert!(request.backend.is_none());
            }
        }
    }

    #[test]
    fn down_yes_does_not_change_shutdown_request() {
        for flags in [vec!["llmup", "down"], vec!["llmup", "down", "--yes"]] {
            let request = Args::try_parse_from(flags)
                .unwrap()
                .lifecycle_options()
                .unwrap();
            assert_eq!(request.command, "down");
            assert!(request.model.is_none());
            assert!(request.port.is_none());
            assert!(request.context.is_none());
            assert!(request.backend.is_none());
            assert!(!request.bypass);
            assert!(!request.installed);
        }
    }
}

#[cfg(test)]
mod migration_output_tests {
    use super::*;

    #[test]
    fn migration_plain_and_json_preserve_summary_and_mode() {
        let summary = rigspark_runtime::memory::MigrationSummary {
            turns_carried: 2,
            turns_summarized: 0,
            vectors_reembedded: 0,
            strategy: "none".into(),
            embedding_strategy: "none".into(),
        };
        for dry_run in [false, true] {
            for move_source in [false, true] {
                for json_mode in [false, true] {
                    let mut output = Vec::new();
                    write_migration_result(
                        "llama3.1:8b",
                        "qwen2.5:14b",
                        dry_run,
                        move_source,
                        &summary,
                        json_mode,
                        &mut output,
                    )
                    .unwrap();
                    if json_mode {
                        assert_eq!(
                            serde_json::from_slice::<Value>(&output).unwrap(),
                            json!({
                                "from": "llama3.1:8b", "to": "qwen2.5:14b", "dryRun": dry_run, "move": move_source, "summary": {
                                    "turnsCarried": 2, "turnsSummarized": 0, "vectorsReembedded": 0, "strategy": "none", "embeddingStrategy": "none"
                                }
                            })
                        );
                    } else {
                        let golden = include_str!(
                            "../../../tests/fixtures/noninteractive/migrate-plain.txt"
                        );
                        let expected = if dry_run {
                            golden.replace("Migrated memory:", "[dry-run] Planned migration:")
                        } else if move_source {
                            golden.replace("qwen2.5:14b\n", "qwen2.5:14b (source removed)\n")
                        } else {
                            golden.to_owned()
                        };
                        assert_eq!(String::from_utf8(output).unwrap(), expected);
                    }
                }
            }
        }
    }
}
