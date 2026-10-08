use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use rigspark_core::generation::{GenerationCatalog, fit};
use rigspark_gui::{Host, engine::NativeEngine, router};
use rigspark_runtime::generation::{
    Events, GenerationError, GenerationFuture, GenerationOutcome, Generator, NativeOptions,
    NativeOutcome,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-image";

/// Writes a PNG into rigspark's output directory, like a successful ComfyUI run.
struct Paints;
impl Generator for Paints {
    fn generate(
        &self,
        options: NativeOptions,
        events: Events,
        _: CancellationToken,
    ) -> GenerationFuture<'_> {
        Box::pin(async move {
            events("Queued ComfyUI prompt abc-123; generating image".into());
            let model = GenerationCatalog::bundled()
                .unwrap()
                .resolve(&options.model)
                .unwrap()
                .clone();
            let path = options.output_dir.join("rigspark-image-abc-123.png");
            std::fs::write(&path, PNG).unwrap();
            let hardware = serde_json::from_value(json!({"arch":"arm64","platform":"darwin",
                "totalRamBytes":3.8e10,"freeRamBytes":3.8e10,"freeDiskBytes":5e11,"gpu":[]}))
            .unwrap();
            Ok(NativeOutcome {
                fit: fit(&model, &hardware),
                result: GenerationOutcome {
                    model: model.id,
                    kind: model.kind,
                    path,
                    bytes: PNG.len() as u64,
                    prompt_id: "abc-123".into(),
                    seed: options.seed.unwrap(),
                    weights: Vec::new(),
                },
            })
        })
    }
}

/// Runs until cancelled.
struct Waits;
impl Generator for Waits {
    fn generate(
        &self,
        _: NativeOptions,
        _: Events,
        cancel: CancellationToken,
    ) -> GenerationFuture<'_> {
        Box::pin(async move {
            cancel.cancelled().await;
            Err(GenerationError::Cancelled)
        })
    }
}

fn host(home: &std::path::Path, generator: Arc<dyn Generator>) -> Arc<Host> {
    Host::with_generator(
        home,
        43211,
        Arc::new(NativeEngine { home: home.into() }),
        generator,
    )
    .unwrap()
}

async fn send(
    host: &Arc<Host>,
    method: &str,
    path: &str,
    payload: Option<Value>,
    token: bool,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", format!("127.0.0.1:{}", host.port))
        .header("origin", host.origin());
    if token {
        builder = builder.header("x-llmup-token", &host.token);
    }
    let body = match payload {
        Some(payload) => {
            builder = builder.header("content-type", "application/json");
            Body::from(payload.to_string())
        }
        None => Body::empty(),
    };
    let response = router(host.clone())
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let kind = response
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|value| value.to_str().unwrap().to_owned());
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    (status, kind, bytes)
}
async fn json_call(
    host: &Arc<Host>,
    method: &str,
    path: &str,
    payload: Option<Value>,
) -> (StatusCode, Value) {
    let (status, _, bytes) = send(host, method, path, payload, true).await;
    (status, serde_json::from_slice(&bytes).unwrap())
}
fn comfy() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("models")).unwrap();
    directory
}
async fn wait_for(host: &Arc<Host>, status: &str) -> Value {
    for _ in 0..200 {
        let (_, value) = json_call(host, "GET", "/api/generation/jobs/current", None).await;
        if value["job"]["status"] == status {
            return value["job"].clone();
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("job never reached {status}");
}

#[tokio::test]
async fn models_endpoint_reports_memory_fit_and_unknown_speed() {
    let home = tempfile::tempdir().unwrap();
    let host = host(home.path(), Arc::new(Paints));
    let (status, value) = json_call(&host, "GET", "/api/generation/models", None).await;
    assert_eq!(status, StatusCode::OK);
    let models = value["models"].as_array().unwrap();
    let catalog = GenerationCatalog::bundled().unwrap();
    assert_eq!(models.len(), catalog.models.len());
    for catalog_model in &catalog.models {
        assert!(models.iter().any(|model| model["id"] == catalog_model.id));
    }
    for model in models {
        assert!(["yes", "slow", "no"].contains(&model["fit"]["verdict"].as_str().unwrap()));
        assert_eq!(model["fit"]["speed"], "unknown");
        assert!(!model["license"].as_str().unwrap().is_empty());
        if model["provenance"] != "auto" {
            assert_eq!(model["license"], "apache-2.0");
        }
    }
    assert_eq!(value["defaults"]["port"], 8188);
}

#[tokio::test]
async fn image_job_runs_and_serves_the_png_preview() {
    let home = tempfile::tempdir().unwrap();
    let comfy = comfy();
    let host = host(home.path(), Arc::new(Paints));
    let request = json!({"model":"image","prompt":"a fox","seed":7,"comfyuiDir":comfy.path()});
    let (status, value) = json_call(&host, "POST", "/api/generation/jobs", Some(request)).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{value}");
    assert_eq!(value["job"]["model"], "flux1-schnell:fp8");
    let job = wait_for(&host, "succeeded").await;
    assert_eq!(job["result"]["result"]["seed"], 7);
    assert!(
        job["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line.as_str().unwrap().contains("Queued ComfyUI prompt"))
    );
    let url = job["outputUrl"].as_str().unwrap();
    let (status, kind, bytes) = send(&host, "GET", url, None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(kind.as_deref(), Some("image/png"));
    assert_eq!(bytes, PNG);
    assert!(
        home.path()
            .join("generations/rigspark-image-abc-123.png")
            .is_file()
    );
}

#[tokio::test]
async fn jobs_are_exclusive_and_cancellable() {
    let home = tempfile::tempdir().unwrap();
    let comfy = comfy();
    let host = host(home.path(), Arc::new(Waits));
    let request = json!({"model":"video","prompt":"waves","comfyuiDir":comfy.path()});
    assert_eq!(
        json_call(&host, "POST", "/api/generation/jobs", Some(request.clone()))
            .await
            .0,
        StatusCode::ACCEPTED
    );
    let (status, value) = json_call(&host, "POST", "/api/generation/jobs", Some(request)).await;
    assert_eq!(status, StatusCode::CONFLICT, "{value}");
    let (status, _, _) = send(
        &host,
        "POST",
        "/api/generation/jobs/current/cancel",
        None,
        false,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, value) =
        json_call(&host, "POST", "/api/generation/jobs/current/cancel", None).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let job = wait_for(&host, "cancelled").await;
    assert!(job["outputUrl"].is_null());
}

#[tokio::test]
async fn invalid_or_unauthorized_requests_are_refused() {
    let home = tempfile::tempdir().unwrap();
    let comfy = comfy();
    let host = host(home.path(), Arc::new(Paints));
    let good = json!({"model":"image","prompt":"a fox","comfyuiDir":comfy.path()});
    let (status, _, _) = send(
        &host,
        "POST",
        "/api/generation/jobs",
        Some(good.clone()),
        false,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "capability token is required"
    );
    for (payload, needle) in [
        (
            json!({"model":"audio","prompt":"x","comfyuiDir":comfy.path()}),
            "unknown generation model",
        ),
        (
            json!({"model":"image","prompt":" ","comfyuiDir":comfy.path()}),
            "prompt must be",
        ),
        (
            json!({"model":"image","prompt":"x","comfyuiDir":home.path()}),
            "models/ folder",
        ),
        (
            json!({"model":"image","prompt":"x","comfyuiDir":comfy.path(),"seed":9007199254740992u64}),
            "seed",
        ),
        (
            json!({"model":"image","prompt":"x","comfyuiDir":comfy.path(),"port":0}),
            "port",
        ),
    ] {
        let (status, value) = json_call(&host, "POST", "/api/generation/jobs", Some(payload)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{value}");
        assert!(value["error"].as_str().unwrap().contains(needle), "{value}");
    }
    let (status, _) = json_call(
        &host,
        "POST",
        "/api/generation/jobs",
        Some(json!({"model":"image","prompt":"x","comfyuiDir":"/c","workflow":{}})),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown fields such as custom workflows are rejected"
    );
    assert!(
        json_call(&host, "GET", "/api/generation/jobs/current", None)
            .await
            .1["job"]
            .is_null()
    );
    for (path, expected) in [
        (
            "/api/generation/jobs/0123456789abcdef0123456789abcdef/output",
            StatusCode::NOT_FOUND,
        ),
        (
            "/api/generation/jobs/../../etc/passwd/output",
            StatusCode::BAD_REQUEST,
        ),
        ("/api/generation/models?x=1", StatusCode::BAD_REQUEST),
    ] {
        assert_eq!(
            send(&host, "GET", path, None, true).await.0,
            expected,
            "{path}"
        );
    }
}

/// Each run writes a distinct file, like successive chat generations in one thread.
struct Numbered(std::sync::atomic::AtomicUsize);
impl Generator for Numbered {
    fn generate(
        &self,
        options: NativeOptions,
        events: Events,
        cancel: CancellationToken,
    ) -> GenerationFuture<'_> {
        let index = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move {
            let mut outcome = Paints.generate(options.clone(), events, cancel).await?;
            let path = options
                .output_dir
                .join(format!("rigspark-image-{index}.png"));
            std::fs::write(&path, [PNG, index.to_string().as_bytes()].concat()).unwrap();
            outcome.result.path = path;
            Ok(outcome)
        })
    }
}

#[tokio::test]
async fn earlier_outputs_stay_viewable_after_later_jobs_finish() {
    let home = tempfile::tempdir().unwrap();
    let comfy = comfy();
    let host = host(
        home.path(),
        Arc::new(Numbered(std::sync::atomic::AtomicUsize::new(0))),
    );
    let mut urls = Vec::new();
    for prompt in ["create mount fuji", "a second picture"] {
        let request = json!({"model":"image","prompt":prompt,"comfyuiDir":comfy.path()});
        let (status, value) = json_call(&host, "POST", "/api/generation/jobs", Some(request)).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{value}");
        let id = value["job"]["id"].as_str().unwrap().to_owned();
        let job = wait_for(&host, "succeeded").await;
        assert_eq!(job["id"], id);
        urls.push(job["outputUrl"].as_str().unwrap().to_owned());
    }
    for (index, url) in urls.iter().enumerate() {
        let (status, kind, bytes) = send(&host, "GET", url, None, false).await;
        assert_eq!(status, StatusCode::OK, "{url}");
        assert_eq!(kind.as_deref(), Some("image/png"));
        assert!(bytes.ends_with(index.to_string().as_bytes()));
    }
}
