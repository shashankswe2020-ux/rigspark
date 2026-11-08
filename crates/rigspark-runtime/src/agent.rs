use crate::{
    harness::{DeltaSink, HarnessError},
    mcp::{Manager, ReviewedCall},
    ollama_inference::{ChatInput, ChatMessage, ChatResult, Tool},
    tool_policy::{SessionGrants, ToolRisk, redact_arguments, redact_result},
};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

#[async_trait::async_trait]
pub trait AgentChat: Send + Sync {
    async fn chat(
        &self,
        input: &ChatInput,
        cancel: &CancellationToken,
        sink: &mut DeltaSink<'_>,
    ) -> Result<ChatResult, HarnessError>;
}
#[derive(Clone, Copy)]
pub enum Decision {
    ApproveOnce,
    AllowSession,
    Deny,
}
#[async_trait::async_trait]
pub trait ToolApprover: Send + Sync {
    async fn decide(
        &self,
        call_id: &str,
        call: &ReviewedCall,
        cancel: &CancellationToken,
    ) -> Result<Decision, HarnessError>;
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AgentEvent {
    Delta { content: String },
    Tool(ToolEvent),
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolEvent {
    pub phase: String,
    pub call_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk: Option<ToolRisk>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}
pub type EventSink<'sink> = dyn FnMut(AgentEvent) -> Result<(), HarnessError> + Send + 'sink;
pub struct AgentOptions<'request> {
    pub session_id: &'request str,
    pub workspace_id: Option<&'request str>,
    pub model: &'request str,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f64>,
    pub max_steps: usize,
}
fn feedback(text: &str) -> String {
    if text.len() <= 8192 {
        return text.into();
    }
    let mut end = 8192;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n... [truncated]", &text[..end])
}
fn tool_event(phase: &str, call_id: &str, name: &str) -> ToolEvent {
    ToolEvent {
        phase: phase.into(),
        call_id: call_id.into(),
        name: name.into(),
        ..Default::default()
    }
}
async fn model_step(
    chat: &dyn AgentChat,
    input: &ChatInput,
    cancel: &CancellationToken,
    sink: &mut EventSink<'_>,
    output: &mut String,
) -> Result<ChatResult, HarnessError> {
    if serde_json::to_vec(input)
        .map_err(|_| HarnessError::Invalid)?
        .len()
        > 4 * 1024 * 1024
    {
        return Err(HarnessError::Limit);
    }
    let mut streamed = String::new();
    let mut emit = |text: &str| {
        if output.len().saturating_add(text.len()) > 16 * 1024 * 1024 {
            return Err(HarnessError::Limit);
        }
        if cancel.is_cancelled() {
            return Err(HarnessError::Cancelled);
        }
        if !text.is_empty() {
            sink(AgentEvent::Delta {
                content: text.into(),
            })?;
            output.push_str(text);
            streamed.push_str(text);
        }
        Ok(())
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(HarnessError::Cancelled),
        result = tokio::time::timeout(Duration::from_secs(120), chat.chat(input, cancel, &mut emit)) => result.map_err(|_| HarnessError::Timeout)??,
    };
    if result.content.len() > 16 * 1024 * 1024 || result.tool_calls.len() > 32 {
        return Err(HarnessError::Limit);
    }
    if result.tool_calls.is_empty() {
        let remainder = result
            .content
            .strip_prefix(&streamed)
            .ok_or(HarnessError::Response)?;
        if output.len().saturating_add(remainder.len()) > 16 * 1024 * 1024 {
            return Err(HarnessError::Limit);
        }
        if !remainder.is_empty() {
            sink(AgentEvent::Delta {
                content: remainder.into(),
            })?;
            output.push_str(remainder);
        }
    }
    Ok(result)
}
pub async fn run_turn(
    chat: &dyn AgentChat,
    manager: &mut Manager,
    grants: &mut SessionGrants,
    approver: Option<&dyn ToolApprover>,
    options: AgentOptions<'_>,
    cancel: &CancellationToken,
    sink: &mut EventSink<'_>,
) -> Result<String, HarnessError> {
    if options.session_id.is_empty()
        || options.session_id.len() > 128
        || options.max_steps == 0
        || options.max_steps > 16
        || options.messages.len() > 10000
        || options.temperature.is_some_and(|value| !value.is_finite())
    {
        return Err(HarnessError::Invalid);
    }
    grants.select(options.session_id, options.workspace_id);
    let mut routing = BTreeMap::new();
    let mut tools = Vec::new();
    for (connector, tool) in manager.tools() {
        let name = tool.name.clone();
        if routing.contains_key(&name) {
            continue;
        }
        tools.push(Tool {
            name: name.clone(),
            description: tool.description.clone(),
            parameters: serde_json::from_value(tool.input_schema.clone())
                .map_err(|_| HarnessError::Invalid)?,
        });
        routing.insert(name, (connector, tool));
    }
    if tools.len() > 1024 {
        return Err(HarnessError::Limit);
    }
    let mut input = ChatInput {
        model: options.model.into(),
        messages: options.messages,
        tools,
        temperature: options.temperature,
        slot: None,
    };
    let mut output = String::new();
    let mut call_count = 0usize;
    for _ in 0..options.max_steps {
        let result = model_step(chat, &input, cancel, sink, &mut output).await?;
        if result.tool_calls.is_empty() {
            return Ok(output);
        }
        input.messages.push(ChatMessage {
            role: "assistant".into(),
            content: result.content,
            tool_calls: result.tool_calls.clone(),
            tool_name: None,
        });
        for call in result.tool_calls {
            if cancel.is_cancelled() {
                return Err(HarnessError::Cancelled);
            }
            call_count += 1;
            if call_count > 64 {
                return Err(HarnessError::Limit);
            }
            let call_id = uuid::Uuid::new_v4().to_string();
            let arguments =
                serde_json::to_value(&call.arguments).map_err(|_| HarnessError::Invalid)?;
            if serde_json::to_vec(&arguments)
                .map_err(|_| HarnessError::Invalid)?
                .len()
                > 65536
            {
                return Err(HarnessError::Limit);
            }
            let route = routing.get(&call.name);
            let reviewed = route.and_then(|(connector, tool)| {
                manager
                    .review(connector, &tool.name, arguments.clone())
                    .ok()
            });
            let mut proposed = tool_event("proposed", &call_id, &call.name);
            proposed.connector = route.map(|(connector, _)| connector.clone());
            proposed.risk = Some(
                reviewed
                    .as_ref()
                    .map(|review| review.risk)
                    .unwrap_or(ToolRisk::Unknown),
            );
            proposed.arguments = Some(redact_arguments(&arguments));
            sink(AgentEvent::Tool(proposed))?;
            let mut approval = None;
            if let Some(reviewed) = reviewed {
                approval = manager.session_approval(reviewed, grants).ok();
            }
            if approval.is_none()
                && let Some((connector, tool)) = route
            {
                let reviewed = manager
                    .review(connector, &tool.name, arguments.clone())
                    .map_err(|_| HarnessError::Drift)?;
                let mut required = tool_event("approval-required", &call_id, &call.name);
                required.connector = Some(connector.clone());
                required.risk = Some(reviewed.risk);
                required.arguments = Some(redact_arguments(&arguments));
                sink(AgentEvent::Tool(required))?;
                let decision = if let Some(approver) = approver {
                    tokio::select! { biased; _ = cancel.cancelled() => return Err(HarnessError::Cancelled), result = tokio::time::timeout(Duration::from_secs(120), approver.decide(&call_id, &reviewed, cancel)) => result.ok().and_then(Result::ok).unwrap_or(Decision::Deny) }
                } else {
                    Decision::Deny
                };
                approval = match decision {
                    Decision::ApproveOnce => {
                        Some(manager.approve(reviewed).map_err(|_| HarnessError::Drift)?)
                    }
                    Decision::AllowSession => Some(
                        manager
                            .approve_session(reviewed, grants)
                            .map_err(|_| HarnessError::Drift)?,
                    ),
                    Decision::Deny => None,
                };
            }
            if cancel.is_cancelled() {
                return Err(HarnessError::Cancelled);
            }
            let content = if let (Some(approval), Some((connector, tool))) = (approval, route) {
                sink(AgentEvent::Tool(tool_event("start", &call_id, &call.name)))?;
                let started = Instant::now();
                let result = manager
                    .call(connector, &tool.name, arguments, Some(approval), cancel)
                    .await;
                if cancel.is_cancelled() {
                    return Err(HarnessError::Cancelled);
                }
                let (content, is_error) = match result {
                    Ok(result) => (feedback(&result.content), result.is_error),
                    Err(_) => ("Tool execution failed.".into(), true),
                };
                let preview = redact_result(&content);
                let mut event = tool_event("done", &call_id, &call.name);
                event.result = Some(preview.text);
                event.result_truncated = Some(preview.truncated);
                event.is_error = Some(is_error);
                event.duration_ms =
                    Some(started.elapsed().as_millis().min(u64::MAX as u128) as u64);
                sink(AgentEvent::Tool(event))?;
                content
            } else {
                sink(AgentEvent::Tool(tool_event("denied", &call_id, &call.name)))?;
                "The user denied this tool call.".into()
            };
            input.messages.push(ChatMessage {
                role: "tool".into(),
                content,
                tool_calls: vec![],
                tool_name: Some(call.name),
            });
        }
    }
    input.tools.clear();
    let final_reply = model_step(chat, &input, cancel, sink, &mut output).await?;
    if !final_reply.tool_calls.is_empty() {
        return Err(HarnessError::Response);
    }
    Ok(output)
}
