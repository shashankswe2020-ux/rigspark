use rigspark_runtime::connect::{Agent, ConnectError, ConnectPlan, plan};

const OLLAMA: &str = "http://127.0.0.1:11434";

fn launch(plan: ConnectPlan) -> (String, Vec<String>, Vec<(String, String)>) {
    match plan {
        ConnectPlan::Launch { binary, args, env } => (binary.into(), args, env),
        ConnectPlan::Instructions { .. } => panic!("expected a launch"),
    }
}

fn env<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

#[test]
fn agent_names_are_a_fixed_registry() {
    for (name, agent) in [
        ("claude", Agent::ClaudeCode),
        ("claude-code", Agent::ClaudeCode),
        ("codex", Agent::Codex),
        ("opencode", Agent::OpenCode),
        ("pi", Agent::Pi),
        ("hermes", Agent::Hermes),
        ("openclaw", Agent::OpenClaw),
        ("cline", Agent::Cline),
    ] {
        assert_eq!(Agent::parse(name), Some(agent), "{name}");
    }
    for bad in ["", "Claude", "../codex", "bash", "codex;rm"] {
        assert_eq!(Agent::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn claude_code_talks_to_the_local_anthropic_endpoint_without_cloud_keys() {
    let (binary, args, pairs) =
        launch(plan(Agent::ClaudeCode, OLLAMA, "ollama", "qwen3:14b").unwrap());
    assert_eq!(binary, "claude");
    assert_eq!(args, ["--model", "qwen3:14b"]);
    assert_eq!(env(&pairs, "ANTHROPIC_BASE_URL"), Some(OLLAMA));
    assert_eq!(
        env(&pairs, "ANTHROPIC_API_KEY"),
        Some(""),
        "a cloud key must not win"
    );
    assert!(env(&pairs, "ANTHROPIC_AUTH_TOKEN").is_some_and(|token| !token.is_empty()));
    assert_eq!(env(&pairs, "ANTHROPIC_SMALL_FAST_MODEL"), Some("qwen3:14b"));
    assert_eq!(
        env(&pairs, "ANTHROPIC_DEFAULT_HAIKU_MODEL"),
        Some("qwen3:14b")
    );
}

#[test]
fn codex_gets_a_one_off_responses_provider_instead_of_a_config_edit() {
    let (binary, args, pairs) = launch(
        plan(
            Agent::Codex,
            "http://127.0.0.1:8080",
            "llamacpp",
            "qwen3:14b",
        )
        .unwrap(),
    );
    assert_eq!(binary, "codex");
    assert!(pairs.is_empty());
    assert_eq!(
        args,
        [
            "-c",
            "model_provider=\"rigspark\"",
            "-c",
            "model_providers.rigspark={name=\"RigSpark (local)\",base_url=\"http://127.0.0.1:8080/v1\",wire_api=\"responses\"}",
            "-m",
            "qwen3:14b",
        ]
    );
}

#[test]
fn opencode_receives_inline_config_only() {
    let (binary, args, pairs) =
        launch(plan(Agent::OpenCode, "http://127.0.0.1:8080", "mlx", "mlx-model").unwrap());
    assert_eq!(binary, "opencode");
    assert!(args.is_empty());
    let config: serde_json::Value =
        serde_json::from_str(env(&pairs, "OPENCODE_CONFIG_CONTENT").unwrap()).unwrap();
    assert_eq!(config["model"], "rigspark/mlx-model");
    assert_eq!(
        config["provider"]["rigspark"]["options"]["baseURL"],
        "http://127.0.0.1:8080/v1"
    );
    assert_eq!(
        config["provider"]["rigspark"]["npm"],
        "@ai-sdk/openai-compatible"
    );
    assert!(config["provider"]["rigspark"]["models"]["mlx-model"].is_object());
}

#[test]
fn ui_and_file_configured_agents_get_exact_instructions() {
    for agent in [Agent::Pi, Agent::Hermes, Agent::OpenClaw, Agent::Cline] {
        match plan(agent, OLLAMA, "ollama", "qwen3:14b").unwrap() {
            ConnectPlan::Instructions { text } => {
                assert!(text.contains("http://127.0.0.1:11434/v1"), "{agent:?}");
                assert!(text.contains("qwen3:14b"), "{agent:?}");
            }
            ConnectPlan::Launch { .. } => panic!("{agent:?} must not be launched"),
        }
    }
}

#[test]
fn agents_are_refused_when_the_backend_lacks_their_api() {
    for (agent, backend) in [
        (Agent::ClaudeCode, "mlx"),
        (Agent::ClaudeCode, "lmstudio"),
        (Agent::ClaudeCode, "bitnet"),
        (Agent::Codex, "mlx"),
        (Agent::Codex, "bitnet"),
    ] {
        assert!(
            matches!(
                plan(agent, OLLAMA, backend, "m"),
                Err(ConnectError::Unsupported { .. })
            ),
            "{agent:?} on {backend}"
        );
    }
    plan(Agent::Codex, OLLAMA, "lmstudio", "m").unwrap();
    plan(Agent::OpenCode, OLLAMA, "bitnet", "m").unwrap();
}

#[test]
fn only_loopback_endpoints_and_safe_model_ids_are_connected() {
    for endpoint in [
        "http://10.0.0.5:11434",
        "http://example.com:11434",
        "file:///etc/passwd",
    ] {
        assert!(
            plan(Agent::Codex, endpoint, "ollama", "qwen3:14b").is_err(),
            "{endpoint}"
        );
    }
    for model in ["", "a\"b", "x y", "../up", "m\n"] {
        assert!(
            plan(Agent::Codex, OLLAMA, "ollama", model).is_err(),
            "{model:?}"
        );
    }
}
