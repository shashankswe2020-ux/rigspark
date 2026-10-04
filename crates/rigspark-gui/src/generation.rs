//! Loopback API for local image/video generation jobs (one job at a time).
use crate::{
    Host,
    routes::{ApiError, ApiResult, bad, body, json_response, missing},
};
use axum::{
    extract::Request,
    http::{StatusCode, header},
    response::IntoResponse,
};
use rigspark_core::generation::{GenerationCatalog, fit};
use rigspark_runtime::generation::{
    DEFAULT_PORT, Events, NativeOptions, default_comfyui_dir, output_extension, prepare,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::VecDeque, path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

const EVENT_LIMIT: usize = 200;
const MAX_OUTPUT_BYTES: u64 = 1024 * 1024 * 1024;
/// Finished outputs stay viewable so earlier results in a chat thread keep working.
const OUTPUT_HISTORY: usize = 64;

#[derive(Default)]
pub struct Jobs {
    current: std::sync::Mutex<Option<Job>>,
    outputs: std::sync::Mutex<VecDeque<(String, PathBuf, &'static str)>>,
}

struct Job {
    id: String,
    model: String,
    kind: &'static str,
    status: &'static str,
    events: VecDeque<String>,
    result: Option<Value>,
    error: Option<String>,
    output: Option<PathBuf>,
    cancel: CancellationToken,
}
impl Job {
    fn snapshot(&self) -> Value {
        json!({
            "id": self.id,
            "model": self.model,
            "kind": self.kind,
            "status": self.status,
            "events": self.events,
            "result": self.result,
            "error": self.error,
            "outputUrl": self.output.as_ref().map(|_| format!("/api/generation/jobs/{}/output", self.id)),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JobRequest {
    model: String,
    prompt: String,
    seed: Option<u64>,
    comfyui_dir: String,
    port: Option<u16>,
    #[serde(default)]
    bypass: bool,
}

fn authorized(host: &Host, request: &Request) -> Result<(), ApiError> {
    if request
        .headers()
        .get("x-llmup-token")
        .and_then(|value| value.to_str().ok())
        != Some(host.token.as_str())
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "missing or invalid capability token",
        ));
    }
    Ok(())
}

fn update(host: &Host, id: &str, change: impl FnOnce(&mut Job)) {
    if let Ok(mut slot) = host.generation.current.lock()
        && let Some(job) = slot.as_mut().filter(|job| job.id == id)
    {
        change(job);
    }
}

fn output_dir(host: &Host) -> Result<PathBuf, ApiError> {
    let directory = host.home.join("generations");
    std::fs::create_dir_all(&directory).map_err(|_| bad())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| bad())?;
    }
    Ok(directory)
}

async fn models() -> ApiResult {
    let catalog = GenerationCatalog::bundled().map_err(|_| bad())?;
    let (hardware, _) = rigspark_runtime::hardware::detect()
        .await
        .map_err(|_| bad())?;
    let models: Vec<_> = catalog
        .models
        .iter()
        .map(|model| {
            json!({
                "id": model.id,
                "kind": model.kind,
                "params": model.params,
                "license": model.license,
                "releaseDate": model.release_date,
                "default": model.default,
                "weightsBytes": model.total_bytes(),
                "source": model.source,
                "fit": fit(model, &hardware),
            })
        })
        .collect();
    Ok(json_response(json!({
        "models": models,
        "defaults": {
            "comfyuiDir": default_comfyui_dir().map(|dir| dir.display().to_string()),
            "port": DEFAULT_PORT,
        },
    })))
}

async fn start(host: Arc<Host>, request: Request) -> ApiResult {
    authorized(&host, &request)?;
    let input: JobRequest = body(request, crate::MAX_REQUEST_BYTES).await?;
    let options = NativeOptions {
        model: input.model,
        prompt: input.prompt,
        seed: input.seed,
        comfyui_dir: PathBuf::from(input.comfyui_dir.trim()),
        port: input.port.unwrap_or(DEFAULT_PORT),
        bypass: input.bypass,
        output: None,
        output_dir: output_dir(&host)?,
    };
    let (model, seed, _) = match prepare(&options) {
        Ok(prepared) => prepared,
        Err(error) => return Ok(crate::error(StatusCode::BAD_REQUEST, &error.to_string())),
    };
    let options = NativeOptions {
        seed: Some(seed),
        ..options
    };
    let id = uuid::Uuid::new_v4().simple().to_string();
    let cancel = host.shutdown.child_token();
    {
        let mut slot = host.generation.current.lock().map_err(|_| bad())?;
        if slot.as_ref().is_some_and(|job| job.status == "running") {
            return Ok(crate::error(
                StatusCode::CONFLICT,
                "a generation is already running",
            ));
        }
        *slot = Some(Job {
            id: id.clone(),
            model: model.id.clone(),
            kind: model.kind.name(),
            status: "running",
            events: VecDeque::from([format!("Starting {} with seed {seed}", model.id)]),
            result: None,
            error: None,
            output: None,
            cancel: cancel.clone(),
        });
    }
    let events: Events = {
        let host = host.clone();
        let id = id.clone();
        Arc::new(move |message: String| {
            let message: String = rigspark_core::reports::strip_control(&message)
                .chars()
                .take(512)
                .collect();
            update(&host, &id, |job| {
                if job.events.len() == EVENT_LIMIT {
                    job.events.pop_front();
                }
                job.events.push_back(message);
            });
        })
    };
    let worker = host.clone();
    let job_id = id.clone();
    host.tasks.spawn(async move {
        let outcome = worker
            .generator
            .generate(options, events, cancel.clone())
            .await;
        if let Ok(outcome) = &outcome
            && let Ok(mut outputs) = worker.generation.outputs.lock()
        {
            if outputs.len() == OUTPUT_HISTORY {
                outputs.pop_front();
            }
            outputs.push_back((
                job_id.clone(),
                outcome.result.path.clone(),
                outcome.result.kind.name(),
            ));
        }
        update(&worker, &job_id, |job| match outcome {
            Ok(outcome) => {
                job.status = "succeeded";
                job.events
                    .push_back(format!("Saved {}", outcome.result.path.display()));
                job.output = Some(outcome.result.path.clone());
                job.result = serde_json::to_value(&outcome).ok();
            }
            Err(_) if cancel.is_cancelled() => job.status = "cancelled",
            Err(error) => {
                job.status = "failed";
                job.error = Some(error.to_string());
            }
        });
    });
    let snapshot = host
        .generation
        .current
        .lock()
        .map_err(|_| bad())?
        .as_ref()
        .map(Job::snapshot);
    Ok((StatusCode::ACCEPTED, axum::Json(json!({"job": snapshot}))).into_response())
}

async fn output(host: Arc<Host>, id: &str) -> ApiResult {
    let (path, kind) = host
        .generation
        .outputs
        .lock()
        .map_err(|_| bad())?
        .iter()
        .find(|(job, _, _)| job == id)
        .map(|(_, path, kind)| (path.clone(), *kind))
        .ok_or_else(missing)?;
    let extension = if kind == "video" {
        output_extension(rigspark_core::generation::GenerationKind::Video)
    } else {
        output_extension(rigspark_core::generation::GenerationKind::Image)
    };
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|_| missing())?;
    if !metadata.is_file() || metadata.len() > MAX_OUTPUT_BYTES {
        return Err(missing());
    }
    let bytes = tokio::fs::read(&path).await.map_err(|_| missing())?;
    let content_type = if extension == "webp" {
        "image/webp"
    } else {
        "image/png"
    };
    Ok((
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_DISPOSITION, "inline"),
        ],
        bytes,
    )
        .into_response())
}

pub async fn dispatch(host: Arc<Host>, request: Request) -> ApiResult {
    if request.uri().query().is_some() {
        return Err(bad());
    }
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    match (method.as_str(), path.as_str()) {
        ("GET", "/api/generation/models") => models().await,
        ("POST", "/api/generation/jobs") => start(host, request).await,
        ("GET", "/api/generation/jobs/current") => {
            let job = host
                .generation
                .current
                .lock()
                .map_err(|_| bad())?
                .as_ref()
                .map(Job::snapshot);
            Ok(json_response(json!({"job": job})))
        }
        ("POST", "/api/generation/jobs/current/cancel") => {
            authorized(&host, &request)?;
            let slot = host.generation.current.lock().map_err(|_| bad())?;
            let job = slot
                .as_ref()
                .filter(|job| job.status == "running")
                .ok_or_else(missing)?;
            job.cancel.cancel();
            Ok(json_response(json!({"job": job.snapshot()})))
        }
        ("GET", _) => {
            let id = path
                .strip_prefix("/api/generation/jobs/")
                .and_then(|rest| rest.strip_suffix("/output"))
                .filter(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .ok_or_else(bad)?;
            output(host, id).await
        }
        _ => Err(bad()),
    }
}
