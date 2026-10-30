use super::Args;
use clap::{
    Arg, ArgAction, Command, CommandFactory, FromArgMatches, error::ErrorKind, parser::ValueSource,
};

struct CommandSpec {
    name: &'static str,
    description: &'static str,
    flags: &'static [&'static str],
    model: bool,
    ui: bool,
}

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "recommend",
        description: "Detect hardware and rank local models (default command)",
        flags: &[
            "task",
            "month",
            "today",
            "context",
            "max_context",
            "context_percent",
            "backend",
            "available_backends",
            "installed",
            "port",
            "fits_only",
            "catalog_path",
            "perf_path",
            "hardware_json",
            "hardware",
            "kv_cache",
        ],
        model: false,
        ui: true,
    },
    CommandSpec {
        name: "can-run",
        description: "Check whether this machine can run a model",
        flags: &[
            "context",
            "backend",
            "installed",
            "port",
            "catalog_path",
            "perf_path",
            "hardware_json",
            "hardware",
            "kv_cache",
        ],
        model: true,
        ui: true,
    },
    CommandSpec {
        name: "plan",
        description: "Show every execution path (GPU, multi-GPU, offload, CPU) for one model",
        flags: &[
            "context",
            "backend",
            "catalog_path",
            "perf_path",
            "hardware_json",
            "hardware",
            "kv_cache",
        ],
        model: true,
        ui: false,
    },
    CommandSpec {
        name: "up",
        description: "Install verified weights and start a loopback server",
        flags: &[
            "port",
            "backend",
            "bypass",
            "installed",
            "context",
            "catalog_path",
            "perf_path",
            "hardware_json",
            "kv_cache",
            "flash_attn",
            "prompt_cache",
            "speculative",
            "parallel",
        ],
        model: true,
        ui: true,
    },
    CommandSpec {
        name: "switch",
        description: "Switch the active model without moving memory",
        flags: &[
            "port",
            "backend",
            "bypass",
            "installed",
            "context",
            "catalog_path",
            "perf_path",
            "hardware_json",
            "kv_cache",
            "flash_attn",
            "prompt_cache",
            "speculative",
            "parallel",
        ],
        model: true,
        ui: true,
    },
    CommandSpec {
        name: "down",
        description: "Stop owned servers or detach and forget an attached daemon without stopping it",
        flags: &["yes", "forget", "catalog_path", "perf_path"],
        model: true,
        ui: true,
    },
    CommandSpec {
        name: "ls",
        description: "List active server state from local state (not installed-model inventory)",
        flags: &[],
        model: false,
        ui: true,
    },
    CommandSpec {
        name: "doctor",
        description: "Diagnose hardware, runtimes, ports, and state",
        flags: &["catalog_path", "perf_path", "hardware_json", "hardware"],
        model: false,
        ui: true,
    },
    CommandSpec {
        name: "catalog",
        description: "Show the model catalog or preview a catalog refresh",
        flags: &[
            "all",
            "month",
            "today",
            "generation",
            "refresh",
            "update",
            "status",
            "catalog_path",
            "perf_path",
            "hardware_json",
            "hardware",
        ],
        model: false,
        ui: true,
    },
    CommandSpec {
        name: "chat",
        description: "Chat interactively, from piped input, or with one message",
        flags: &[
            "chat_model",
            "harness",
            "message",
            "agent",
            "skills",
            "no_memory",
        ],
        model: false,
        ui: true,
    },
    CommandSpec {
        name: "migrate",
        description: "Copy or move memory between models",
        flags: &[
            "from",
            "to",
            "move_memory",
            "dry_run",
            "yes",
            "context",
            "catalog_path",
        ],
        model: false,
        ui: true,
    },
    CommandSpec {
        name: "gui",
        description: "Launch the loopback browser GUI",
        flags: &["port", "harness", "no_open"],
        model: false,
        ui: false,
    },
    CommandSpec {
        name: "bench",
        description: "Measure TTFT, prefill and decode speed of the active server",
        flags: &[
            "iterations",
            "warmup",
            "output_tokens",
            "concurrency",
            "fixture",
        ],
        model: false,
        ui: false,
    },
    CommandSpec {
        name: "connect",
        description: "Start a coding agent against the active server, or show how to configure it",
        flags: &["dry_run"],
        model: true,
        ui: false,
    },
    CommandSpec {
        name: "add",
        description: "Add a GGUF model from Hugging Face to your catalog (requires network)",
        flags: &["search", "remove", "dry_run", "bypass"],
        model: true,
        ui: false,
    },
    CommandSpec {
        name: "generate",
        description: "Generate an image or video locally through ComfyUI",
        flags: &["prompt", "output", "seed", "comfyui_dir", "port", "bypass"],
        model: true,
        ui: true,
    },
];

impl CommandSpec {
    fn accepts(&self, id: &str) -> bool {
        self.flags.contains(&id)
            || (id == "json" && self.name != "catalog")
            || (self.model && id == "model")
            || (self.ui && ["tui", "no_tui", "accessible", "no_color"].contains(&id))
    }
}

fn help_command(spec: &CommandSpec, flat: &Command) -> Command {
    let mut command = Command::new(spec.name).about(spec.description);
    for argument in flat.get_arguments() {
        let id = argument.get_id().as_str();
        if spec.accepts(id) {
            let mut argument = argument.clone().help(match id {
                "model" if spec.name == "down" => "Guard shutdown against a different active model",
                "model" if spec.name == "connect" => {
                    "Agent: claude, codex, opencode, pi, hermes, openclaw, cline"
                }
                "model" if spec.name == "add" => {
                    "owner/repo[:QUANT] or a Hugging Face URL; with --remove, an added model id"
                }
                "model" if spec.name == "generate" => {
                    "image, video, or a model id from `catalog --generation`"
                }
                "prompt" => "Text prompt describing the image or video",
                "output" => "Output file (.png for images, .webp for videos); never overwritten",
                "seed" => "Sampler seed for reproducible results (random when omitted)",
                "comfyui_dir" => "ComfyUI installation directory (default: $RIGSPARK_COMFYUI_DIR)",
                "generation" => "Show local image and video generation models",
                "model" => "Model ID, alias, or search query (required outside interactive mode)",
                "chat_model" => "Chat model (defaults to the active model for the local harness)",
                "harness" => "Chat harness: local, claude, openai, openai-compatible, opencode",
                "message" => "Send one message instead of starting interactive chat",
                "agent" => "Agent from the local library",
                "skills" => "Skill from the local library (repeatable)",
                "no_memory" => "Disable conversation memory capture",
                "no_open" => "Do not open the browser automatically",
                "from" => "Source model whose memory will be migrated",
                "to" => "Target model receiving migrated memory",
                "move_memory" => "Delete source memory after successful migration (requires --yes)",
                "dry_run" if spec.name == "connect" => {
                    "Print the command and environment instead of starting the agent"
                }
                "dry_run" if spec.name == "add" => "List quantizations and fit without adding",
                "dry_run" => "Preview migration without writing memory",
                "speculative" => {
                    "Speculative decoding for a runtime rigspark starts: auto, draft, ngram, off"
                }
                "parallel" => "Concurrent request slots for a runtime rigspark starts (1-16)",
                "iterations" => "Measured runs (1-20, default 5)",
                "warmup" => "Warmup runs excluded from results (0-5, default 1)",
                "output_tokens" => "Maximum tokens generated per run (16-2048, default 128)",
                "concurrency" => "Requests sent at once per run (1-16, default 1)",
                "fixture" => "Prompt fixture: short or long",
                "search" => "Search Hugging Face for GGUF repositories (requires network)",
                "remove" => "Remove a model you added",
                "yes" => "Skip confirmation; retain drift protection (migration requires --move)",
                "forget" => {
                    "Clear a stale attached-server pointer without probing or stopping any process"
                }
                "json" if spec.name == "gui" => {
                    "Print the server URL as JSON without opening a browser"
                }
                "json" => "Emit machine-readable JSON",
                "context" => "Context size in tokens (integer in 1..10000000)",
                "max_context" => "Report the largest context each model can hold",
                "context_percent" => "Use 25, 50, 75, or 100 percent of each model's context",
                "kv_cache" if ["up", "switch"].contains(&spec.name) => {
                    "KV cache type for a runtime rigspark starts: f16, q8_0, q4_0"
                }
                "kv_cache" => {
                    "Size the KV cache as f16 (default), q8_0, or q4_0; needs a context flag"
                }
                "flash_attn" => "Flash attention for a runtime rigspark starts: auto, on, off",
                "prompt_cache" => "Reuse cached prompt prefixes (llama.cpp): off, reuse",
                "task" => {
                    "Boost models for a task: chat, code, vision, reasoning, tools, embedding"
                }
                "month" => "Only models released (or auto-added) in the last 1, 2 or 3 months",
                "today" => "Date the --month window ends on (YYYY-MM-DD)",
                "backend" => "Runtime: ollama, llamacpp, mlx, lmstudio, bitnet",
                "port" if spec.name == "gui" => "Loopback GUI port (default: 4000)",
                "port" if spec.name == "generate" => "Loopback ComfyUI port (default: 8188)",
                "port" if ["recommend", "can-run"].contains(&spec.name) => {
                    "Ollama port for --installed (default: 11434)"
                }
                "port" => "Backend server port (default: 11434)",
                "bypass" if spec.name == "add" => {
                    "Add a quantization even when it does not fit this machine"
                }
                "bypass" if spec.name == "generate" => {
                    "Generate even when weights do not fit memory; retain integrity checks"
                }
                "bypass" => "Override estimated fit; retain integrity checks",
                "installed" if ["up", "switch"].contains(&spec.name) => {
                    "Use an installed Ollama tag (requires --bypass)"
                }
                "installed" => "Inspect models installed in a local Ollama daemon",
                "fits_only" => "Only show installed models with known full-context fit",
                "available_backends" => "Only show models an installed backend can serve",
                "all" => "Show every catalog model, including non-fitting models",
                "refresh" => "Preview catalog enrichment without writing the catalog",
                "update" => "Download and activate the latest signed catalog (requires network)",
                "status" => "Show the selected catalog source, date, revision, and digest offline",
                "tui" => "Use the interactive terminal UI (fails when incompatible)",
                "no_tui" => "Force plain noninteractive output",
                "no_color" => "Disable terminal color while retaining layout",
                "accessible" => "Use the line-oriented accessible interactive UI",
                "catalog_path" => "Load a catalog from this JSON file",
                "perf_path" => "Load throughput data from this JSON file",
                "hardware_json" => "Use a supplied hardware fixture",
                "hardware" => "Evaluate a hardware profile JSON file instead of this machine",
                _ => unreachable!("all command options have help"),
            });
            if id == "model" {
                argument = argument.index(1);
            }
            command = command.arg(argument);
        }
    }
    command
}

pub(super) fn error_command() -> String {
    let matches = Args::command()
        .disable_help_flag(true)
        .disable_version_flag(true)
        .ignore_errors(true)
        .mut_args(|argument| {
            if argument.get_action().takes_values() {
                argument.value_parser(clap::builder::StringValueParser::new())
            } else {
                argument
            }
        })
        .get_matches();
    matches
        .get_one::<String>("command")
        .cloned()
        .unwrap_or_else(|| "recommend".into())
}

pub(super) fn parse() -> Result<Option<Args>, clap::Error> {
    let flat = Args::command();
    let matches = flat
        .clone()
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("version")
                .short('v')
                .visible_short_alias('V')
                .long("version")
                .action(ArgAction::SetTrue),
        )
        .try_get_matches()?;
    let args = Args::from_arg_matches(&matches)?;
    let spec = COMMANDS
        .iter()
        .find(|spec| spec.name == args.command)
        .expect("validated command");
    if !args.parity {
        for argument in flat.get_arguments() {
            let id = argument.get_id().as_str();
            if !["command", "parity", "help", "version"].contains(&id)
                && matches.value_source(id) == Some(ValueSource::CommandLine)
                && !spec.accepts(id)
            {
                return Err(clap::Error::raw(
                    ErrorKind::UnknownArgument,
                    format!(
                        "{} is not supported by this command\n",
                        argument
                            .get_long()
                            .map(|name| format!("--{name}"))
                            .unwrap_or_else(|| id.to_owned())
                    ),
                ));
            }
        }
    }
    if (matches.get_flag("help") || matches.get_flag("version"))
        && args.port.is_some_and(|port| !(1..=65535).contains(&port))
    {
        return Err(clap::Error::raw(
            ErrorKind::ValueValidation,
            "invalid --port (expected an integer in 1..65535)\n",
        ));
    }
    if matches.get_flag("version") {
        println!("rigspark {}", env!("CARGO_PKG_VERSION"));
        return Ok(None);
    }
    if matches.get_flag("help") {
        let mut command = if matches.value_source("command") == Some(ValueSource::CommandLine) {
            help_command(spec, &flat).bin_name(format!("rigspark {}", spec.name))
        } else {
            let mut root = help_command(&COMMANDS[0], &flat)
                .name("rigspark")
                .bin_name("rigspark")
                .disable_help_subcommand(true)
                .about("Hardware-aware local model advice, runtime management, and chat")
                .after_help(format!(
                    "With no command, runs recommend. Use rigspark <command> --help for command options.\n\n{}",
                    rigspark_cli::distribution::SPONSOR_HELP
                ));
            for entry in COMMANDS {
                root = root.subcommand(help_command(entry, &flat));
            }
            root
        };
        command = command
            .version(env!("CARGO_PKG_VERSION"))
            .disable_version_flag(true)
            .arg(
                Arg::new("version")
                    .short('v')
                    .visible_short_alias('V')
                    .long("version")
                    .help("Print version")
                    .action(ArgAction::Version),
            );
        print!("{}", command.render_long_help());
        return Ok(None);
    }
    Ok(Some(args))
}

#[cfg(test)]
mod command_table_tests {
    use super::*;

    #[test]
    fn every_parsable_command_has_a_spec_and_help() {
        let flat = Args::command();
        let command = flat
            .get_arguments()
            .find(|argument| argument.get_id() == "command")
            .unwrap();
        let names: Vec<String> = command
            .get_possible_values()
            .iter()
            .map(|value| value.get_name().to_owned())
            .collect();
        assert!(names.len() > 10, "{names:?}");
        for name in &names {
            let spec = COMMANDS
                .iter()
                .find(|spec| spec.name == name)
                .unwrap_or_else(|| panic!("{name} parses but has no command spec"));
            help_command(spec, &flat);
        }
        for spec in COMMANDS {
            assert!(names.iter().any(|name| name == spec.name), "{}", spec.name);
        }
    }
}
