//! Points third-party coding agents at the active local server without editing their own
//! configuration files: agents that accept per-launch settings are launched with them,
//! the rest get exact instructions.

use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    ClaudeCode,
    Codex,
    OpenCode,
    Pi,
    Hermes,
    OpenClaw,
    Cline,
}

impl Agent {
    pub const NAMES: [&'static str; 7] = [
        "claude", "codex", "opencode", "pi", "hermes", "openclaw", "cline",
    ];
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "claude" | "claude-code" => Self::ClaudeCode,
            "codex" => Self::Codex,
            "opencode" => Self::OpenCode,
            "pi" => Self::Pi,
            "hermes" => Self::Hermes,
            "openclaw" => Self::OpenClaw,
            "cline" => Self::Cline,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
            Self::Hermes => "Hermes Agent",
            Self::OpenClaw => "OpenClaw",
            Self::Cline => "Cline",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectPlan {
    /// Run `binary` (looked up on PATH) with these arguments and extra environment.
    Launch {
        binary: &'static str,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    Instructions {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConnectError {
    #[error("{agent} needs the {api} API, which the {backend} backend does not serve")]
    Unsupported {
        agent: &'static str,
        api: &'static str,
        backend: String,
    },
    #[error("only a loopback rigspark endpoint can be connected")]
    Endpoint,
    #[error("invalid runtime model id")]
    Model,
}

/// Wire APIs each backend serves (verified: llama.cpp b10090, current Ollama, LM Studio).
fn serves(backend: &str, api: &str) -> bool {
    match api {
        "Anthropic Messages" => matches!(backend, "ollama" | "llamacpp"),
        "OpenAI Responses" => matches!(backend, "ollama" | "llamacpp" | "lmstudio"),
        _ => true,
    }
}

pub fn plan(
    agent: Agent,
    endpoint: &str,
    backend: &str,
    model: &str,
) -> Result<ConnectPlan, ConnectError> {
    let url = crate::state::loopback(endpoint).map_err(|_| ConnectError::Endpoint)?;
    let host = url.host_str().ok_or(ConnectError::Endpoint)?;
    let port = url.port_or_known_default().ok_or(ConnectError::Endpoint)?;
    let base = format!("{}://{host}:{port}", url.scheme());
    let v1 = format!("{base}/v1");
    crate::adapters::model_id(model).map_err(|_| ConnectError::Model)?;
    let require = |api: &'static str| {
        if serves(backend, api) {
            Ok(())
        } else {
            Err(ConnectError::Unsupported {
                agent: agent.label(),
                api,
                backend: rigspark_core::reports::strip_control(backend),
            })
        }
    };
    let pair = |key: &str, value: &str| (key.to_owned(), value.to_owned());
    Ok(match agent {
        Agent::ClaudeCode => {
            require("Anthropic Messages")?;
            ConnectPlan::Launch {
                binary: "claude",
                args: vec!["--model".into(), model.into()],
                env: vec![
                    pair("ANTHROPIC_BASE_URL", &base),
                    pair("ANTHROPIC_AUTH_TOKEN", "rigspark-local"),
                    pair("ANTHROPIC_API_KEY", ""),
                    pair("ANTHROPIC_SMALL_FAST_MODEL", model),
                    pair("ANTHROPIC_DEFAULT_HAIKU_MODEL", model),
                ],
            }
        }
        Agent::Codex => {
            require("OpenAI Responses")?;
            ConnectPlan::Launch {
                binary: "codex",
                args: vec![
                    "-c".into(),
                    "model_provider=\"rigspark\"".into(),
                    "-c".into(),
                    format!(
                        "model_providers.rigspark={{name=\"RigSpark (local)\",base_url=\"{v1}\",wire_api=\"responses\"}}"
                    ),
                    "-m".into(),
                    model.into(),
                ],
                env: Vec::new(),
            }
        }
        Agent::OpenCode => {
            let config = json!({
                "model": format!("rigspark/{model}"),
                "provider": {"rigspark": {
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "RigSpark (local)",
                    "options": {"baseURL": v1},
                    "models": {model: {"name": model}},
                }},
            });
            ConnectPlan::Launch {
                binary: "opencode",
                args: Vec::new(),
                env: vec![pair("OPENCODE_CONFIG_CONTENT", &config.to_string())],
            }
        }
        Agent::Pi => {
            let snippet = json!({"providers": {"rigspark": {
                "baseUrl": v1,
                "api": "openai-completions",
                "apiKey": "rigspark-local",
                "models": [{"id": model}],
            }}});
            ConnectPlan::Instructions {
                text: format!(
                    "Pi reads providers from its models.json (default ~/.pi/agent/models.json).\nMerge this provider, then pick {model} with /model:\n\n{}\n",
                    serde_json::to_string_pretty(&snippet).unwrap_or_default()
                ),
            }
        }
        Agent::Hermes => ConnectPlan::Instructions {
            text: format!(
                "Run `hermes model`, choose More providers > Custom endpoint, and enter:\n  Base URL: {v1}\n  API key:  (leave blank)\n  Model:    {model}\n"
            ),
        },
        Agent::OpenClaw => ConnectPlan::Instructions {
            text: format!(
                "Add an OpenAI-compatible custom provider in OpenClaw:\n  Base URL: {v1}\n  API:      openai-completions\n  API key:  rigspark-local (ignored)\n  Model:    {model}\n"
            ),
        },
        Agent::Cline => ConnectPlan::Instructions {
            text: format!(
                "In Cline: Settings > API Configuration > API Provider = OpenAI Compatible.\n  Base URL: {v1}\n  API key:  (leave blank)\n  Model ID: {model}\nSet the context window to at least 32K tokens.\n"
            ),
        },
    })
}
