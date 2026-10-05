use crate::{Host, MAX_REQUEST_BYTES, error};
use axum::{
    Json,
    body::to_bytes,
    extract::Request,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use rigspark_runtime::{
    library::{Kind, LibraryItem, LibraryUpdate},
    mcp::{Connector, ConnectorFile, ConnectorStore},
    workspace::{EditProposal, LineRange},
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, sync::Arc};

pub struct ApiError(pub StatusCode, pub &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        error(self.0, self.1)
    }
}
pub type ApiResult = Result<Response, ApiError>;
pub fn bad() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid request")
}
pub fn missing() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not found")
}
fn checked<T, E>(result: Result<T, E>) -> Result<T, ApiError> {
    result.map_err(|_| bad())
}
pub fn json_response(value: Value) -> Response {
    Json(value).into_response()
}
pub async fn body<T: DeserializeOwned>(request: Request, maximum: usize) -> Result<T, ApiError> {
    if !request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(';').next() == Some("application/json"))
    {
        return Err(ApiError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "application/json required",
        ));
    }
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        to_bytes(request.into_body(), maximum),
    )
    .await
    .map_err(|_| ApiError(StatusCode::REQUEST_TIMEOUT, "body timeout"))?
    .map_err(|_| ApiError(StatusCode::PAYLOAD_TOO_LARGE, "request body exceeds limit"))?;
    serde_json::from_slice(&bytes).map_err(|_| bad())
}
fn number(query: &BTreeMap<String, String>, name: &str, default: usize) -> Result<usize, ApiError> {
    query
        .get(name)
        .map(|value| value.parse().map_err(|_| bad()))
        .unwrap_or(Ok(default))
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionPatch {
    title: Option<String>,
    archived: Option<bool>,
    expected_revision: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    body: String,
    enabled: Option<bool>,
    #[serde(default)]
    skills: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RootRequest {
    path: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdRequest {
    id: String,
}

pub async fn dispatch(host: Arc<Host>, request: Request) -> ApiResult {
    if request.method() == "GET" && request.uri().path() == "/api/telemetry" {
        let telemetry = host.telemetry.clone();
        let sample = tokio::task::spawn_blocking(move || {
            telemetry
                .lock()
                .map(|mut sampler| sampler.sample())
                .map_err(|_| ())
        })
        .await
        .map_err(|_| bad())?
        .map_err(|_| bad())?;
        let usage = host.inference_usage.lock().map_err(|_| bad())?.clone();
        let mut value = serde_json::to_value(sample).map_err(|_| bad())?;
        value["inferenceUsage"] =
            serde_json::to_value(&*usage.lock().map_err(|_| bad())?).map_err(|_| bad())?;
        return Ok(json_response(value));
    }
    if request.uri().path().starts_with("/api/generation/") {
        return crate::generation::dispatch(host, request).await;
    }
    if request.uri().path().starts_with("/api/models/")
        || request.uri().path().starts_with("/api/catalog/")
        || request.uri().path().starts_with("/api/runtimes")
        || request.uri().path() == "/api/hardware"
    {
        return crate::models::dispatch(host, request).await;
    }
    if request.method() == "POST"
        && ["/api/chat", "/api/chat/cancel", "/api/chat/tool-decision"]
            .contains(&request.uri().path())
    {
        return crate::chat::handle(host, request).await;
    }
    let method = request.method().as_str().to_owned();
    let url = url::Url::parse(&format!("{}{}", host.origin(), request.uri())).map_err(|_| bad())?;
    let path = url.path();
    let query: BTreeMap<String, String> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    let stamp = rigspark_runtime::native_chat::timestamp().map_err(|_| bad())?;
    if path == "/api/harnesses" && method == "GET" {
        return Ok(json_response(
            json!({"harnesses":["local","claude","openai","openai-compatible","opencode"]}),
        ));
    }
    if path == "/api/update" && method == "GET" {
        return Ok(json_response(crate::update::fetch().await));
    }
    if path == "/api/status" || path == "/api/history" {
        if method != "GET" {
            return Err(bad());
        }
        let ui = host.ui.lock().await;
        let history = ui
            .session
            .as_ref()
            .map(|id| host.sessions.get(id))
            .transpose()
            .map_err(|_| bad())?
            .flatten()
            .map(|doc| doc.messages.into_iter().rev().take(20).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .rev()
            .map(|message| json!({"role":message.role,"content":message.content}))
            .collect::<Vec<_>>();
        return Ok(json_response(if path.ends_with("history") {
            json!({"history":history})
        } else {
            json!({"harness":ui.harness,"model":ui.model,"memory":{"turns":history.len()}})
        }));
    }
    if path == "/api/harness" && method == "POST" {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Switch {
            harness: String,
        }
        let input: Switch = body(request, MAX_REQUEST_BYTES).await?;
        if !["local", "claude", "openai", "openai-compatible", "opencode"]
            .contains(&input.harness.as_str())
        {
            return Err(bad());
        }
        host.ui.lock().await.harness = input.harness.clone();
        host.cancel_chat();
        return Ok(json_response(json!({"harness":input.harness})));
    }
    if path == "/api/sessions" {
        if method == "GET" {
            let archived = query.get("archived").is_some_and(|value| value == "1");
            if let Some(query) = query.get("q").filter(|query| !query.trim().is_empty()) {
                let results = checked(host.sessions.search(query, archived))?
                    .into_iter()
                    .map(|(summary, snippet)| json!({"summary":summary,"snippet":snippet}))
                    .collect::<Vec<_>>();
                return Ok(json_response(json!({"results":results})));
            }
            let (sessions, next) = checked(host.sessions.list(
                archived,
                number(&query, "cursor", 0)?,
                number(&query, "limit", 50)?,
            ))?;
            return Ok(json_response(
                json!({"sessions":sessions,"nextCursor":next,"activeSessionId":host.ui.lock().await.session}),
            ));
        }
        if method == "POST" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Create {
                title: Option<String>,
            }
            let input: Create = body(request, MAX_REQUEST_BYTES).await?;
            let session = checked(
                host.sessions
                    .create(input.title.as_deref().unwrap_or(""), &stamp),
            )?;
            activate(&host, &session.id).await?;
            return Ok((
                StatusCode::CREATED,
                Json(json!({"session":session.summary()})),
            )
                .into_response());
        }
    }
    if let Some(rest) = path.strip_prefix("/api/sessions/") {
        let segments: Vec<_> = rest.split('/').collect();
        let id = segments[0];
        let doc = checked(host.sessions.get(id))?.ok_or_else(missing)?;
        match (method.as_str(), segments.get(1).copied()) {
            ("GET", None) => return Ok(json_response(json!({"session":doc.summary()}))),
            ("GET", Some("messages")) => {
                let (messages, next) = checked(host.sessions.messages(
                    id,
                    number(&query, "cursor", 0)?,
                    number(&query, "limit", 100)?,
                ))?;
                return Ok(json_response(
                    json!({"messages":messages,"nextCursor":next}),
                ));
            }
            ("POST", Some("activate")) => {
                activate(&host, id).await?;
                return Ok(json_response(json!({"activeSessionId":id})));
            }
            ("DELETE", None) => {
                host.cancel_chat();
                checked(host.sessions.runs.cancel(id, None))?;
                checked(host.sessions.remove(id))?;
                let mut ui = host.ui.lock().await;
                if ui.session.as_deref() == Some(id) {
                    ui.session = None;
                    *host.inference_usage.lock().map_err(|_| bad())? =
                        Arc::new(std::sync::Mutex::new(Default::default()));
                }
                return Ok(json_response(json!({"removed":id})));
            }
            ("PATCH", None) => {
                let input: SessionPatch = body(request, MAX_REQUEST_BYTES).await?;
                if input.title.is_none() && input.archived.is_none() {
                    return Err(bad());
                }
                let mut doc = doc;
                if let Some(title) = input.title {
                    doc = host
                        .sessions
                        .rename(id, &title, input.expected_revision, &stamp)
                        .map_err(session_error)?;
                }
                if let Some(archived) = input.archived {
                    doc = host
                        .sessions
                        .archive(id, archived, Some(doc.revision), &stamp)
                        .map_err(session_error)?;
                }
                return Ok(json_response(json!({"session":doc.summary()})));
            }
            _ => return Err(missing()),
        }
    }
    if path.starts_with("/api/agents") || path.starts_with("/api/skills") {
        let kind = if path.starts_with("/api/agents") {
            Kind::Agent
        } else {
            Kind::Skill
        };
        let segments: Vec<_> = path.split('/').collect();
        let id = segments.get(3).copied();
        match (method.as_str(), id) {
            ("GET", None) => {
                return Ok(json_response(
                    json!({"items":checked(host.library.list(kind))?}),
                ));
            }
            ("POST", None) => {
                let input: Draft = body(request, MAX_REQUEST_BYTES).await?;
                let item = checked(host.library.create(
                    kind,
                    LibraryItem {
                        id: String::new(),
                        name: input.name,
                        description: input.description,
                        enabled: input.enabled.unwrap_or(true),
                        body: input.body,
                        skills: input.skills,
                    },
                ))?;
                return Ok((StatusCode::CREATED, Json(json!({"item":item}))).into_response());
            }
            ("GET", Some(id)) => {
                return Ok(json_response(
                    json!({"item":checked(host.library.get(kind,id))?.ok_or_else(missing)?}),
                ));
            }
            ("PUT", Some(id)) => {
                let input: LibraryUpdate = body(request, MAX_REQUEST_BYTES).await?;
                return Ok(json_response(
                    json!({"item":checked(host.library.update(kind,id,input))?}),
                ));
            }
            ("DELETE", Some(id)) => {
                checked(host.library.remove(kind, id))?;
                return Ok(json_response(json!({"removed":id})));
            }
            _ => return Err(missing()),
        }
    }
    if path.starts_with("/api/workspace/") {
        if path == "/api/workspace/status" && method == "GET" {
            return Ok(json_response(
                json!({"rootId":host.ui.lock().await.workspace}),
            ));
        }
        if ["/api/workspace/root", "/api/workspace/root/create"].contains(&path) && method == "POST"
        {
            let create = path.ends_with("create");
            let input: RootRequest = body(request, MAX_REQUEST_BYTES).await?;
            if input.path.is_empty() || input.path.len() > 4096 {
                return Err(bad());
            }
            let root = {
                let mut workspace = host.workspace.lock().await;
                checked(if create {
                    workspace.create_root(Path::new(&input.path))
                } else {
                    workspace.register(Path::new(&input.path))
                })?
            };
            host.ui.lock().await.workspace = Some(root.id.clone());
            host.cancel_chat();
            host.grants.lock().await.clear();
            host.reviews.lock().await.clear();
            host.disclosures.lock().await.clear();
            return Ok((StatusCode::CREATED, Json(json!({"root":root}))).into_response());
        }
        if path == "/api/workspace/root/revoke" && method == "POST" {
            let input: IdRequest = body(request, MAX_REQUEST_BYTES).await?;
            host.workspace.lock().await.revoke(&input.id);
            host.ui.lock().await.workspace = None;
            host.cancel_chat();
            host.grants.lock().await.clear();
            host.reviews.lock().await.clear();
            host.disclosures.lock().await.clear();
            return Ok(json_response(json!({"revoked":input.id})));
        }
        let workspace = host.workspace.lock().await;
        let id = query.get("id").map(String::as_str).unwrap_or("");
        let relative = query.get("path").map(String::as_str).unwrap_or("");
        if method == "GET" {
            match path {
                "/api/workspace/tree" => {
                    return Ok(json_response(
                        json!({"path":relative,"entries":checked(workspace.tree(id,relative))?}),
                    ));
                }
                "/api/workspace/search" => {
                    return Ok(json_response(
                        serde_json::to_value(checked(workspace.search(
                            id,
                            query.get("q").map(String::as_str).unwrap_or(""),
                            number(&query, "limit", 50)?,
                            query.get("cursor").map(String::as_str),
                        ))?)
                        .map_err(|_| bad())?,
                    ));
                }
                "/api/workspace/file" => {
                    let range = if query.contains_key("startLine") || query.contains_key("endLine")
                    {
                        Some(LineRange {
                            start_line: number(&query, "startLine", 0)?,
                            end_line: number(&query, "endLine", 0)?,
                        })
                    } else {
                        None
                    };
                    return Ok(json_response(
                        json!({"snapshot":checked(workspace.read(id,relative,range))?}),
                    ));
                }
                "/api/workspace/git" => {
                    return Ok(json_response(
                        json!({"snapshot":checked(workspace.git_context(id,query.get("mode").map(String::as_str).unwrap_or("status")))?}),
                    ));
                }
                _ => (),
            }
        }
        if method == "POST" && path == "/api/workspace/edits/review" {
            let proposal: EditProposal = body(request, 1024 * 1024).await?;
            let key = serde_json::to_string(&proposal).map_err(|_| bad())?;
            let review = checked(workspace.review(&proposal))?;
            let value = serde_json::to_value(&review).map_err(|_| bad())?;
            let mut reviews = host.reviews.lock().await;
            if reviews.len() >= 100 {
                reviews.clear();
            }
            reviews.insert(key, review);
            return Ok(json_response(json!({"review":value})));
        }
        if method == "POST" && path == "/api/workspace/edits/apply" {
            let input: EditProposal = body(request, 1024 * 1024).await?;
            let key = serde_json::to_string(&input).map_err(|_| bad())?;
            let review = host.reviews.lock().await.remove(&key).ok_or_else(bad)?;
            return Ok(json_response(
                json!({"result":checked(workspace.apply(review,&host.home.join("edit-records"),&stamp))?}),
            ));
        }
        if method == "POST" && path == "/api/workspace/edits/revert" {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct Revert {
                application_id: String,
            }
            let input: Revert = body(request, MAX_REQUEST_BYTES).await?;
            return Ok(json_response(
                json!({"result":checked(workspace.revert(&input.application_id,&host.home.join("edit-records")))?}),
            ));
        }
    }
    if path.starts_with("/api/connectors") {
        let store = ConnectorStore::new(&host.home);
        if method != "GET" {
            host.cancel_chat();
        }
        let mut manager = host.connectors.lock().await;
        if method != "GET" {
            host.grants.lock().await.clear();
        }
        match (method.as_str(), path) {
            ("GET", "/api/connectors") => {
                return Ok(json_response(json!({"connectors":manager.list()})));
            }
            ("GET", "/api/connectors/config") => {
                return Ok(json_response(
                    json!({"config":masked_connectors(manager.snapshot())}),
                ));
            }
            ("PUT", "/api/connectors/config") => {
                let file: ConnectorFile = body(request, MAX_REQUEST_BYTES).await?;
                let file = unmask_connectors(file, &manager.snapshot())?;
                checked(manager.replace(file, &store).await)?;
                return Ok(json_response(json!({"connectors":manager.list()})));
            }
            ("POST", "/api/connectors") => {
                let mut input: Value = body(request, MAX_REQUEST_BYTES).await?;
                if input.get("id").is_some() {
                    return Err(bad());
                }
                let name = input["name"].as_str().ok_or_else(bad)?.trim();
                let mut slug = String::new();
                for character in name.to_lowercase().chars() {
                    if character.is_ascii_alphanumeric() {
                        slug.push(character);
                    } else if !slug.ends_with('-') {
                        slug.push('-');
                    }
                }
                let slug = slug.trim_matches('-').chars().take(48).collect::<String>();
                let base = if slug.is_empty() { "connector" } else { &slug };
                let definitions = manager.snapshot();
                let id = (1..1000)
                    .map(|number| {
                        if number == 1 {
                            base.to_owned()
                        } else {
                            format!("{base}-{number}")
                        }
                    })
                    .find(|id| !definitions.connectors.iter().any(|entry| entry.id() == id))
                    .ok_or_else(bad)?;
                input["id"] = json!(id);
                let connector: Connector = serde_json::from_value(input).map_err(|_| bad())?;
                checked(manager.add(connector, &store).await)?;
                return Ok((
                    StatusCode::CREATED,
                    Json(json!({"connector":manager.list().into_iter().find(|entry|entry.id==id)})),
                )
                    .into_response());
            }
            _ => (),
        }
        let segments: Vec<_> = path.split('/').collect();
        if let Some(id) = segments.get(3) {
            if method == "DELETE" && segments.len() == 4 {
                checked(manager.remove(id, &store).await)?;
                return Ok(json_response(json!({"removed":id})));
            }
            if method == "POST" {
                match segments.get(4).copied() {
                    Some("connect") => checked(
                        manager
                            .connect(id, &rigspark_runtime::mcp_sdk::SdkFactory, &host.shutdown)
                            .await,
                    )?,
                    Some("disconnect") => checked(manager.disconnect(id).await)?,
                    _ => return Err(missing()),
                };
                return Ok(json_response(
                    json!({"connector":manager.list().into_iter().find(|entry|entry.id==*id)}),
                ));
            }
        }
    }
    if let Some(name) = path.strip_prefix("/api/images/")
        && method == "GET"
    {
        let image =
            match rigspark_runtime::context::read_artifact(&host.home.join("artifacts"), name) {
                Err(error) if error.0 == rigspark_runtime::context::ARTIFACT_NOT_FOUND => {
                    return Err(missing());
                }
                result => checked(result)?,
            };
        return Ok((
            [
                (header::CONTENT_TYPE, image.content_type),
                (
                    header::CONTENT_SECURITY_POLICY,
                    "default-src 'none'; sandbox",
                ),
            ],
            image.content,
        )
            .into_response());
    }
    Err(missing())
}
const MASKED_SECRET: &str = "••••••";
/// Connector env values are credentials; the browser only ever sees a placeholder.
fn masked_connectors(mut file: ConnectorFile) -> ConnectorFile {
    for connector in &mut file.connectors {
        if let Connector::Stdio { env: Some(env), .. } = connector {
            for value in env.values_mut() {
                *value = MASKED_SECRET.into();
            }
        }
    }
    file
}
fn unmask_connectors(
    mut file: ConnectorFile,
    stored: &ConnectorFile,
) -> Result<ConnectorFile, ApiError> {
    for connector in &mut file.connectors {
        let id = connector.id().to_owned();
        if let Connector::Stdio { env: Some(env), .. } = connector {
            for (key, value) in env.iter_mut() {
                if value != MASKED_SECRET {
                    continue;
                }
                *value = stored
                    .connectors
                    .iter()
                    .find_map(|previous| match previous {
                        Connector::Stdio {
                            id: previous_id,
                            env: Some(previous_env),
                            ..
                        } if *previous_id == id => previous_env.get(key).cloned(),
                        _ => None,
                    })
                    .ok_or_else(bad)?;
            }
        }
    }
    Ok(file)
}
pub fn session_error(error: rigspark_runtime::sessions::SessionError) -> ApiError {
    if matches!(error, rigspark_runtime::sessions::SessionError::Conflict) {
        ApiError(
            StatusCode::CONFLICT,
            "session was modified by another writer",
        )
    } else {
        bad()
    }
}
async fn activate(host: &Host, id: &str) -> Result<(), ApiError> {
    host.cancel_chat();
    checked(host.sessions.get(id))?.ok_or_else(missing)?;
    let mut ui = host.ui.lock().await;
    if let Some(old) = &ui.session {
        checked(host.sessions.runs.cancel(old, None))?;
    }
    ui.session = Some(id.into());
    *host.inference_usage.lock().map_err(|_| bad())? =
        Arc::new(std::sync::Mutex::new(Default::default()));
    host.disclosures.lock().await.clear();
    host.grants.lock().await.clear();
    Ok(())
}
