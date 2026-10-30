use crate::{
    Host,
    engine::SelectedEngine,
    routes::{ApiError, ApiResult, bad, body, json_response, session_error},
};
use axum::{
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Sse, sse::Event},
};
use rigspark_runtime::{
    chat_service::{ChatOptions, ChatService},
    context::ContextRef,
    harness::{HarnessError, HarnessMessage},
    workspace::LineRange,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    convert::Infallible,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio_stream::{Stream, wrappers::ReceiverStream};
use tokio_util::sync::CancellationToken;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Input {
    model: Option<String>,
    harness: Option<String>,
    messages: Vec<HarnessMessage>,
    agent_id: Option<String>,
    #[serde(default)]
    skill_ids: Vec<String>,
    system_prompt: Option<String>,
    temperature: Option<f64>,
    #[serde(default)]
    attachments: Vec<Attachment>,
    #[serde(default)]
    context_sources: Vec<ContextRef>,
    #[serde(default)]
    disclosure_ack: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Attachment {
    workspace_id: String,
    path: String,
    range: Option<LineRange>,
}
struct Events {
    inner: ReceiverStream<Result<Event, Infallible>>,
    cancel: CancellationToken,
}
impl Stream for Events {
    type Item = Result<Event, Infallible>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}
impl Drop for Events {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
fn event(value: serde_json::Value) -> Result<Event, Infallible> {
    Ok(Event::default().data(value.to_string()))
}
pub async fn handle(host: Arc<Host>, request: Request) -> ApiResult {
    if request.uri().path() == "/api/chat/tool-decision" {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Choice {
            call_id: String,
            decision: String,
        }
        let choice: Choice = body(request, crate::MAX_REQUEST_BYTES).await?;
        let decision = match choice.decision.as_str() {
            "approve-once" => rigspark_runtime::agent::Decision::ApproveOnce,
            "allow-session" => rigspark_runtime::agent::Decision::AllowSession,
            "deny" => rigspark_runtime::agent::Decision::Deny,
            _ => return Err(bad()),
        };
        return if host.approvals.resolve(&choice.call_id, decision) {
            Ok(json_response(json!({"ok":true})))
        } else {
            Err(crate::routes::missing())
        };
    }
    if request.uri().path() == "/api/chat/cancel" {
        let cancelled = host.cancel_chat();
        return Ok(json_response(json!({"cancelled":cancelled})));
    }
    let input: Input = body(request, crate::MAX_REQUEST_BYTES).await?;
    let permit = host
        .admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError(StatusCode::CONFLICT, "a run is already active"))?;
    let cancel = host.shutdown.child_token();
    *host.chat_cancel.lock().map_err(|_| bad())? = Some(cancel.clone());
    if input.messages.is_empty()
        || input.messages.len() > 20
        || input.messages.iter().any(|message| {
            !["user", "assistant"].contains(&message.role.as_str())
                || message.content.encode_utf16().count() > 4096
        })
        || input.skill_ids.len() > 20
        || input
            .temperature
            .is_some_and(|value| !value.is_finite() || !(0.0..=2.0).contains(&value))
        || input
            .system_prompt
            .as_ref()
            .is_some_and(|value| value.encode_utf16().count() > 8192)
    {
        return Err(bad());
    }
    let stamp = rigspark_runtime::native_chat::timestamp().map_err(|_| bad())?;
    let (session, provider, model) = {
        let mut ui = host.ui.lock().await;
        if ui.session.is_none() {
            ui.session = Some(host.sessions.create("", &stamp).map_err(session_error)?.id);
        }
        (
            ui.session.clone().ok_or_else(bad)?,
            input.harness.unwrap_or_else(|| ui.harness.clone()),
            input.model.unwrap_or_else(|| ui.model.clone()),
        )
    };
    let provider = match provider.as_str() {
        "local" => "local",
        "openai" => "openai",
        "claude" => "claude",
        "openai-compatible" => "openai-compatible",
        "opencode" => "opencode",
        _ => return Err(bad()),
    };
    if model.len() > 256 {
        return Err(bad());
    }
    if host
        .sessions
        .runs
        .active_id(&session)
        .map_err(session_error)?
        .is_some()
    {
        return Err(ApiError(StatusCode::CONFLICT, "a run is already active"));
    }
    let revision = host
        .sessions
        .get(&session)
        .map_err(session_error)?
        .ok_or_else(bad)?
        .revision;
    let mut refs = input.context_sources;
    refs.extend(input.attachments.into_iter().map(|item| ContextRef::File {
        workspace_id: item.workspace_id,
        path: item.path,
        range: item.range,
    }));
    let message = input.messages.last().ok_or_else(bad)?.content.clone();
    let prepared = {
        let workspace = host.workspace.lock().await;
        let mut disclosures = host.disclosures.lock().await;
        let mut service = ChatService {
            sessions: &host.sessions,
            library: &host.library,
            workspace: &workspace,
            disclosures: &mut disclosures,
        };
        let prepared = service
            .prepare_with_prompt(
                ChatOptions {
                    session_id: &session,
                    revision,
                    provider,
                    model: &model,
                    message: &message,
                    context: &refs,
                    agent: input.agent_id.as_deref(),
                    skills: &input.skill_ids,
                    temperature: input.temperature,
                },
                input.system_prompt.as_deref(),
            )
            .map_err(|_| bad())?;
        if input.disclosure_ack {
            service.approve_disclosure(&prepared);
        }
        prepared
    };
    let allowed = host
        .disclosures
        .lock()
        .await
        .allowed(&session, provider, prepared.context());
    let (sender, receiver) = tokio::sync::mpsc::channel(256);
    if cancel.is_cancelled() {
        return Err(ApiError(StatusCode::CONFLICT, "run scope changed"));
    }
    let stream_cancel = cancel.clone();
    if !allowed {
        let _=sender.try_send(event(json!({"type":"disclosure-required","provider":provider,"model":model,"items":prepared.context().manifest,"totalBytes":prepared.context().manifest.iter().filter(|item|item.included).map(|item|item.size).sum::<u64>(),"excludedCount":prepared.context().manifest.iter().filter(|item|!item.included).count()})));
    } else {
        let usage = Arc::new(std::sync::Mutex::new(
            rigspark_runtime::usage::InferenceUsage::default(),
        ));
        *host.inference_usage.lock().map_err(|_| bad())? = usage.clone();
        host.tasks.clone().spawn(rigspark_runtime::usage::scope(usage, async move {
            let _permit=permit;
            if !prepared.context().manifest.is_empty(){let _=sender.send(event(json!({"type":"context","attachments":prepared.context().manifest}))).await;}
            let workspace=rigspark_runtime::workspace::WorkspaceService::new();
            let mut disclosures=host.disclosures.lock().await.clone();
            let service=ChatService{sessions:&host.sessions,library:&host.library,workspace:&workspace,disclosures:&mut disclosures};
            let harness=SelectedEngine{name:provider,engine:host.engine.as_ref()};
            let mut text=rigspark_runtime::sessions::GuiTextStream::default();
            let mut sink=|delta:&str| {let content=text.push(delta); if content.is_empty(){return Ok(());} sender.try_send(event(json!({"type":"delta","content":content}))).map_err(|_|HarnessError::Cancelled)};
            let result=if provider=="local" && !host.connectors.lock().await.tools().is_empty() {
                let expected=rigspark_runtime::state::Config::from_home(&host.home).and_then(|config|rigspark_runtime::state::StateStore::new(config).read());
                match expected {
                    Ok(expected)=>{
                        let agent=crate::engine::SelectedAgent{engine:host.engine.as_ref(),expected};
                        let workspace_id=host.ui.lock().await.workspace.clone();
                        let mut manager=host.connectors.lock().await;
                        let mut grants=host.grants.lock().await;
                        let runtime=rigspark_runtime::chat_service::AgentRuntime{chat:&agent,manager:&mut manager,grants:&mut grants,approver:Some(&host.approvals),workspace_id:workspace_id.as_deref(),max_steps:6};
                        service.run_agent(prepared,runtime,&stamp,None,&cancel,&mut |frame| {
                            let value = serde_json::to_value(frame).map_err(|_|HarnessError::Response)?;
                            if value["type"] == "tool" && value["phase"] == "approval-required" {
                                host.approvals.register(value["callId"].as_str().ok_or(HarnessError::Response)?)?;
                            }
                            sender.try_send(event(value)).map_err(|_|HarnessError::Cancelled)
                        }).await
                    }
                    Err(_)=>Err(rigspark_runtime::chat_service::ChatError::Invalid),
                }
            }else{service.run(prepared,&harness,&stamp,None,&cancel,&mut sink).await};
            host.approvals.clear();
            if cancel.is_cancelled(){return;}
            let value=match result {Ok(_)=>json!({"type":"done","turnsAppended":1,"factsExtracted":0,"vectorsEmbedded":0}),Err(error)=>json!({"type":"error","message":failure_message(&error)})};
            let _=sender.send(event(value)).await;
        }));
    }
    Ok(Sse::new(Events {
        inner: ReceiverStream::new(receiver),
        cancel: stream_cancel,
    })
    .keep_alive(axum::response::sse::KeepAlive::default())
    .into_response())
}

/// What a failed chat tells the user: the cause class and the next step, never raw details.
fn failure_message(error: &rigspark_runtime::chat_service::ChatError) -> String {
    use rigspark_runtime::{chat_service::ChatError, harness::HarnessError};
    let cause = match error {
        ChatError::Harness(HarnessError::Drift) | ChatError::Harness(HarnessError::Invalid) => {
            "The model server is not the one RigSpark started; it may have restarted. Start the model again from Models."
        }
        ChatError::Harness(HarnessError::Unavailable) => {
            "No model is ready. Start one from Models."
        }
        ChatError::Harness(
            HarnessError::Transport | HarnessError::Timeout | HarnessError::Status(_),
        ) => "The model server did not answer. Check that it is still running in Tools.",
        ChatError::Harness(HarnessError::Limit) => "The reply was larger than RigSpark allows.",
        _ => "Chat failed.",
    };
    format!("{cause} No completed exchange was saved.")
}

#[cfg(test)]
mod failure_tests {
    use super::failure_message;
    use rigspark_runtime::{chat_service::ChatError, harness::HarnessError};

    #[test]
    fn failures_name_the_cause_and_next_step() {
        assert!(
            failure_message(&ChatError::Harness(HarnessError::Drift))
                .contains("Start the model again")
        );
        assert!(
            failure_message(&ChatError::Harness(HarnessError::Unavailable))
                .contains("Start one from Models")
        );
        assert!(
            failure_message(&ChatError::Harness(HarnessError::Status(500)))
                .contains("did not answer")
        );
        for error in [
            ChatError::Invalid,
            ChatError::Library,
            ChatError::Harness(HarnessError::Drift),
        ] {
            assert!(failure_message(&error).ends_with("No completed exchange was saved."));
        }
    }
}
