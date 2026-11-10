use crate::harness::{ChatHarness, DeltaSink, HarnessError, HarnessRequest};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::PathBuf, process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio_util::sync::CancellationToken;
pub struct LaunchSpec {
    pub args: Vec<String>,
    pub config: String,
    /// The `provider/model` passed to OpenCode, also used for commit attribution.
    pub model: String,
}
pub fn launch_spec(input: &HarnessRequest, unrestricted: bool) -> Result<LaunchSpec, HarnessError> {
    input.validate()?;
    let raw = rigspark_core::reports::strip_control(&input.model)
        .trim()
        .to_owned();
    if raw.is_empty() {
        return Err(HarnessError::Invalid);
    }
    let model = if raw.contains('/') {
        raw
    } else {
        format!("ollama/{raw}")
    };
    let (provider, name) = model.split_once('/').ok_or(HarnessError::Invalid)?;
    if provider.is_empty() || name.is_empty() {
        return Err(HarnessError::Invalid);
    }
    let prompt = input
        .messages
        .iter()
        .map(|message| {
            format!(
                "[{}]\n{}",
                message.role,
                rigspark_core::reports::strip_control(&message.content)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    if prompt.len() > 1048576 {
        return Err(HarnessError::Limit);
    }
    let permission = if unrestricted { "allow" } else { "deny" };
    let mut config = json!({"autoupdate":unrestricted,"share":if unrestricted{"auto"}else{"disabled"},"snapshot":unrestricted,"permission":permission,"agent":{"rigspark-chat":{"description":if unrestricted{"Chat invoked by rigspark (unrestricted)"}else{"Text-only chat invoked by rigspark"},"mode":"primary","permission":permission}}});
    if provider == "ollama" {
        config["provider"] = json!({"ollama":{"npm":"@ai-sdk/openai-compatible","name":"Ollama (local)","options":{"baseURL":"http://127.0.0.1:11434/v1"},"models":{name:{"name":name}}}});
    }
    Ok(LaunchSpec {
        args: vec![
            "run".into(),
            prompt,
            "--model".into(),
            model.clone(),
            "--agent".into(),
            "rigspark-chat".into(),
            "--format".into(),
            "json".into(),
        ],
        config: config.to_string(),
        model,
    })
}
pub fn parse_event(raw: &str) -> Result<String, HarnessError> {
    if raw.trim().is_empty() {
        return Ok(String::new());
    }
    if raw.len() > 1048576 {
        return Err(HarnessError::Limit);
    }
    let event: Value = serde_json::from_str(raw).map_err(|_| HarnessError::Response)?;
    if !event["timestamp"].as_f64().is_some_and(f64::is_finite)
        || !event["sessionID"]
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 1024)
    {
        return Err(HarnessError::Response);
    }
    let part = &event["part"];
    match event["type"].as_str() {
        Some("text") if part["type"] == "text" => part["text"]
            .as_str()
            .map(rigspark_core::reports::strip_control)
            .ok_or(HarnessError::Response),
        Some("error") => Err(HarnessError::Response),
        Some("step_start" | "step_finish" | "reasoning" | "tool_use") => {
            if !part["type"]
                .as_str()
                .is_some_and(|kind| !kind.is_empty() && kind.len() <= 100)
            {
                return Err(HarnessError::Response);
            }
            let capped = |value: &str, maximum: usize| {
                rigspark_core::reports::strip_control(value)
                    .chars()
                    .take(maximum)
                    .collect::<String>()
            };
            if event["type"] == "reasoning" {
                let text = capped(part["text"].as_str().unwrap_or(""), 400);
                return Ok(if text.is_empty() {
                    String::new()
                } else {
                    format!("\n\n> \u{1f4ad} {text}\n")
                });
            }
            if event["type"] == "tool_use" {
                let tool = part["tool"].as_str().unwrap_or("tool");
                let state = &part["state"];
                let input = &state["input"];
                let target = [
                    &state["title"],
                    &input["filePath"],
                    &input["command"],
                    &input["pattern"],
                ]
                .into_iter()
                .filter_map(Value::as_str)
                .find(|text| !text.is_empty())
                .unwrap_or("");
                let target = capped(target, 160);
                let output = capped(state["output"].as_str().unwrap_or(""), 400);
                return Ok(format!(
                    "\n\n> \u{1f527} `{}`{}{}\n",
                    capped(tool, 100),
                    if target.is_empty() {
                        String::new()
                    } else {
                        format!(" \u{b7} {target}")
                    },
                    if output.is_empty() {
                        String::new()
                    } else {
                        format!("\n\n```\n{output}\n```")
                    }
                ));
            }
            Ok(String::new())
        }
        _ => Err(HarnessError::Response),
    }
}
#[async_trait::async_trait]
pub trait OpenCodeRunner: Send + Sync {
    async fn available(&self) -> bool {
        true
    }
    async fn run(
        &self,
        spec: &LaunchSpec,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<(), HarnessError>;
}
pub struct NativeOpenCodeRunner {
    pub binary: PathBuf,
    pub env: BTreeMap<String, String>,
    /// The RigSpark home. When set, commits OpenCode makes carry RigSpark's trailers;
    /// the hooks are installed there when OpenCode is launched.
    pub commit_home: Option<PathBuf>,
}
#[async_trait::async_trait]
impl OpenCodeRunner for NativeOpenCodeRunner {
    async fn available(&self) -> bool {
        self.binary.is_absolute() && self.binary.is_file()
    }
    async fn run(
        &self,
        spec: &LaunchSpec,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<(), HarnessError> {
        if cancel.is_cancelled() {
            return Err(HarnessError::Cancelled);
        }
        if !self.binary.is_absolute() {
            return Err(HarnessError::Invalid);
        }
        let mut child = tokio::process::Command::new(&self.binary)
            .args(&spec.args)
            .env_clear()
            .envs(&self.env)
            .envs(
                self.commit_home
                    .as_deref()
                    .and_then(|home| crate::commit_attribution::install(home).ok())
                    .iter()
                    .flat_map(|installed| {
                        let model = spec.model.rsplit('/').next().unwrap_or(&spec.model);
                        installed.env(model, self.env.get("GIT_CONFIG_COUNT").map(String::as_str))
                    }),
            )
            .env("OPENCODE_CONFIG_CONTENT", &spec.config)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| HarnessError::Unavailable)?;
        let stdout = child.stdout.take().ok_or(HarnessError::Transport)?;
        let stderr = child.stderr.take().ok_or(HarnessError::Transport)?;
        let total = std::sync::atomic::AtomicUsize::new(0);
        let output = async {
            let secrets = self
                .env
                .iter()
                .filter(|(name, _)| {
                    let name = name.to_ascii_lowercase();
                    ["key", "token", "secret", "password", "credential"]
                        .iter()
                        .any(|word| name.contains(word))
                })
                .map(|(_, value)| value.clone());
            let mut redactor = crate::redaction::StreamRedactor::new(secrets);
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let count = (&mut reader)
                    .take(1048577)
                    .read_until(b'\n', &mut line)
                    .await
                    .map_err(|_| HarnessError::Transport)?;
                if count == 0 {
                    break;
                }
                if count > 1048576
                    || total.fetch_add(count, std::sync::atomic::Ordering::Relaxed) + count
                        > 16 * 1024 * 1024
                {
                    return Err(HarnessError::Limit);
                }
                let text = redactor.push(&parse_event(
                    std::str::from_utf8(&line).map_err(|_| HarnessError::Response)?,
                )?);
                if !text.is_empty() {
                    sink(&text)?;
                }
            }
            let tail = redactor.finish();
            if !tail.is_empty() {
                sink(&tail)?;
            }
            Ok::<_, HarnessError>(())
        };
        let errors = async {
            let mut reader = stderr;
            let mut buffer = [0u8; 8192];
            loop {
                let count = reader
                    .read(&mut buffer)
                    .await
                    .map_err(|_| HarnessError::Transport)?;
                if count == 0 {
                    break;
                }
                if total.fetch_add(count, std::sync::atomic::Ordering::Relaxed) + count
                    > 16 * 1024 * 1024
                {
                    return Err(HarnessError::Limit);
                }
            }
            Ok::<_, HarnessError>(())
        };
        let operation = async {
            tokio::try_join!(output, errors)?;
            if !child
                .wait()
                .await
                .map_err(|_| HarnessError::Transport)?
                .success()
            {
                return Err(HarnessError::Transport);
            }
            Ok(())
        };
        let result = tokio::select! {biased;_=cancel.cancelled()=>Err(HarnessError::Cancelled),result=tokio::time::timeout(Duration::from_secs(300),operation)=>result.map_err(|_|HarnessError::Timeout).and_then(|result|result)};
        if result.is_err() {
            let _ = tokio::time::timeout(Duration::from_secs(5), child.kill()).await;
        }
        result
    }
}
pub struct OpenCodeHarness<'runtime> {
    pub runner: &'runtime dyn OpenCodeRunner,
    pub unrestricted: bool,
}
#[async_trait::async_trait]
impl ChatHarness for OpenCodeHarness<'_> {
    fn name(&self) -> &'static str {
        "opencode"
    }
    async fn available(&self) -> bool {
        self.runner.available().await
    }
    async fn chat(
        &self,
        input: &HarnessRequest,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<String, HarnessError> {
        let spec = launch_spec(input, self.unrestricted)?;
        let mut text = String::new();
        self.runner
            .run(&spec, cancel, &mut |chunk| {
                if text.len() + chunk.len() > 16 * 1024 * 1024 {
                    return Err(HarnessError::Limit);
                }
                sink(chunk)?;
                text.push_str(chunk);
                Ok(())
            })
            .await?;
        Ok(text)
    }
}
