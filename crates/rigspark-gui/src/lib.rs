use axum::{
    Json, Router,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
};
mod approval;
mod chat;
pub mod engine;
mod generation;
pub mod models;
pub mod options;
mod routes;
mod runtime;
mod telemetry;
mod update;
use rigspark_runtime::{
    context::DisclosureStore,
    library::Library,
    mcp::{ConnectorStore, Manager},
    sessions::SessionRepository,
    workspace::{EditReview, WorkspaceService},
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
static ASSETS: include_dir::Dir<'_> = include_dir::include_dir!("$CARGO_MANIFEST_DIR/static");
static VENDOR_ASSETS: include_dir::Dir<'_> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/vendor");

pub struct Host {
    pub inference_usage:
        std::sync::Mutex<Arc<std::sync::Mutex<rigspark_runtime::usage::InferenceUsage>>>,
    pub telemetry: Arc<std::sync::Mutex<telemetry::Telemetry>>,
    pub chat_cancel: std::sync::Mutex<Option<CancellationToken>>,
    pub desktop: std::sync::atomic::AtomicBool,
    pub home: PathBuf,
    pub port: u16,
    pub token: String,
    pub sessions: SessionRepository,
    pub library: Library,
    pub ui: Mutex<UiState>,
    pub workspace: Mutex<WorkspaceService>,
    pub connectors: Mutex<Manager>,
    pub disclosures: Mutex<DisclosureStore>,
    pub reviews: Mutex<BTreeMap<String, EditReview>>,
    pub shutdown: CancellationToken,
    pub engine: Arc<dyn engine::Engine>,
    pub approvals: approval::Approvals,
    pub grants: Mutex<rigspark_runtime::tool_policy::SessionGrants>,
    pub admission: Arc<tokio::sync::Semaphore>,
    pub tasks: tokio_util::task::TaskTracker,
    pub runtimes: Mutex<runtime::RuntimeController>,
    pub generation: generation::Jobs,
    pub generator: Arc<dyn rigspark_runtime::generation::Generator>,
}
pub struct UiState {
    pub harness: String,
    pub model: String,
    pub session: Option<String>,
    pub workspace: Option<String>,
}
impl Host {
    pub fn new(home: &Path, port: u16) -> Result<Arc<Self>, Box<dyn std::error::Error>> {
        Self::with_engine(
            home,
            port,
            Arc::new(engine::NativeEngine { home: home.into() }),
        )
    }
    pub fn with_engine(
        home: &Path,
        port: u16,
        engine: Arc<dyn engine::Engine>,
    ) -> Result<Arc<Self>, Box<dyn std::error::Error>> {
        Self::with_generator(
            home,
            port,
            engine,
            Arc::new(rigspark_runtime::generation::NativeGenerator),
        )
    }
    /// Host with an injected image/video generator (tests use fakes instead of ComfyUI).
    pub fn with_generator(
        home: &Path,
        port: u16,
        engine: Arc<dyn engine::Engine>,
        generator: Arc<dyn rigspark_runtime::generation::Generator>,
    ) -> Result<Arc<Self>, Box<dyn std::error::Error>> {
        let config = rigspark_runtime::state::Config::from_home(home)?;
        let store = rigspark_runtime::state::StateStore::new(config.clone());
        store.lock(std::time::Duration::from_secs(10))?.release()?;
        let definitions = ConnectorStore::new(&config.home).load()?;
        Ok(Arc::new(Self {
            inference_usage: std::sync::Mutex::new(Arc::new(std::sync::Mutex::new(
                Default::default(),
            ))),
            telemetry: Arc::new(std::sync::Mutex::new(telemetry::Telemetry::new(
                &config.home,
            ))),
            chat_cancel: std::sync::Mutex::new(None),
            desktop: std::sync::atomic::AtomicBool::new(false),
            home: config.home.clone(),
            port,
            engine,
            runtimes: Mutex::new(Default::default()),
            generation: Default::default(),
            generator,
            approvals: approval::Approvals::default(),
            grants: Mutex::new(Default::default()),
            admission: Arc::new(tokio::sync::Semaphore::new(1)),
            tasks: tokio_util::task::TaskTracker::new(),
            token: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            sessions: SessionRepository::new(&config.home),
            library: Library::new(&config.home),
            ui: Mutex::new(UiState {
                harness: "local".into(),
                model: "local".into(),
                session: None,
                workspace: None,
            }),
            workspace: Mutex::new(WorkspaceService::new()),
            connectors: Mutex::new(Manager::new(definitions)?),
            disclosures: Mutex::new(DisclosureStore::default()),
            reviews: Mutex::new(BTreeMap::new()),
            shutdown: CancellationToken::new(),
        }))
    }
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    pub fn cancel_chat(&self) -> bool {
        self.approvals.clear();
        self.chat_cancel
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .is_some_and(|token| {
                let active = !token.is_cancelled();
                token.cancel();
                active
            })
    }
}
pub fn router(host: Arc<Host>) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/static/{*path}", get(asset))
        .route("/vendor/{*path}", get(vendor))
        .fallback(api)
        .layer(middleware::from_fn_with_state(host.clone(), boundary))
        .with_state(host)
}
fn error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}
async fn boundary(State(host): State<Arc<Host>>, request: Request, next: Next) -> Response {
    if host.shutdown.is_cancelled() {
        return error(StatusCode::SERVICE_UNAVAILABLE, "host is shutting down");
    }
    let expected = format!("127.0.0.1:{}", host.port);
    if request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return error(StatusCode::BAD_REQUEST, "host header mismatch");
    }
    let origin = host.origin();
    if request
        .headers()
        .get(header::ORIGIN)
        .is_some_and(|value| value.as_bytes() != origin.as_bytes())
        || request
            .headers()
            .get("sec-fetch-site")
            .is_some_and(|value| value == "cross-site")
    {
        return error(StatusCode::FORBIDDEN, "cross-origin request refused");
    }
    let mutation = !matches!(request.method().as_str(), "GET" | "HEAD");
    if mutation
        && request
            .headers()
            .get(header::ORIGIN)
            .and_then(|value| value.to_str().ok())
            != Some(origin.as_str())
    {
        return error(StatusCode::FORBIDDEN, "same-origin request required");
    }
    if request.uri().path().starts_with("/api/workspace")
        && request
            .headers()
            .get("x-llmup-token")
            .and_then(|value| value.to_str().ok())
            != Some(host.token.as_str())
    {
        return error(StatusCode::FORBIDDEN, "missing or invalid capability token");
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("x-content-type-options", "nosniff".parse().unwrap());
    response
        .headers_mut()
        .insert("referrer-policy", "no-referrer".parse().unwrap());
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
async fn index(State(host): State<Arc<Host>>) -> Response {
    let Some(index) = ASSETS
        .get_file("index.html")
        .and_then(|file| file.contents_utf8())
    else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "frontend unavailable");
    };
    let html = index.replace(
        "</head>",
        &format!(
            "<meta name=\"llmup-token\" content=\"{}\" /></head>",
            host.token
        ),
    );
    let mut response = ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response();
    response.headers_mut().insert("content-security-policy", "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; frame-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'".parse().unwrap());
    if host.desktop.load(std::sync::atomic::Ordering::Relaxed) {
        response.headers_mut().insert("content-security-policy", "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost; frame-src 'self'; font-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'".parse().unwrap());
    }
    response
        .headers_mut()
        .insert("x-frame-options", "DENY".parse().unwrap());
    response
}
async fn asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    if path.contains(['\\', '%']) || path.split('/').any(|part| part == ".." || part == ".") {
        return error(StatusCode::BAD_REQUEST, "invalid asset path");
    }
    let Some(file) = ASSETS.get_file(&path) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let mime = if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    ([(header::CONTENT_TYPE, mime)], file.contents()).into_response()
}
async fn vendor(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    if path.contains(['\\', '%']) || path.split('/').any(|part| part == ".." || part == ".") {
        return error(StatusCode::BAD_REQUEST, "invalid asset path");
    }
    let Some(file) = VENDOR_ASSETS.get_file(&path) else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    let mime = if path.ends_with(".js") {
        "application/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".ttf") {
        "font/ttf"
    } else {
        return error(StatusCode::NOT_FOUND, "not found");
    };
    ([(header::CONTENT_TYPE, mime)], file.contents()).into_response()
}
async fn api(State(host): State<Arc<Host>>, request: Request) -> Response {
    match routes::dispatch(host, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}
pub async fn serve(listener: tokio::net::TcpListener, host: Arc<Host>) -> std::io::Result<()> {
    if listener.local_addr()?
        != std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, host.port))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "listener must match the configured loopback origin",
        ));
    }
    let result = axum::serve(listener, router(host.clone()))
        .with_graceful_shutdown(host.shutdown.clone().cancelled_owned())
        .await;
    host.shutdown.cancel();
    host.cancel_chat();
    let _ = host.connectors.lock().await.shutdown().await;
    host.approvals.clear();
    host.tasks.close();
    host.tasks.wait().await;
    if let Ok(config) = rigspark_runtime::state::Config::from_home(&host.home) {
        host.runtimes.lock().await.shutdown(config).await;
    }
    result
}
