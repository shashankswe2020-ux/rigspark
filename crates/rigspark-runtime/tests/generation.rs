use rigspark_core::generation::{FileRole, GenerationCatalog, GenerationKind, GenerationModel};
use rigspark_runtime::{
    acquire::{DownloadResponse, DownloadTransport},
    generation::{ComfyUi, GenerationError, GenerationRequest, ensure_local_only, workflow},
    http::{HttpError, Request, Response, Transport},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

type Mutation = dyn Fn(&mut GenerationRequest);
const ENDPOINT: &str = "http://127.0.0.1:8188";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage-bytes";
const WEBP: &[u8] = b"RIFF\x10\0\0\0WEBPVP8 video";

#[derive(Debug, Clone)]
struct Call {
    method: &'static str,
    path: String,
    query: Option<String>,
    body: Option<Value>,
}

type Handler = dyn Fn(&Call, usize) -> Result<(u16, Vec<u8>), HttpError> + Send + Sync;
struct Comfy {
    calls: Mutex<Vec<Call>>,
    history_polls: AtomicUsize,
    handler: Box<Handler>,
}
impl Comfy {
    fn new(
        handler: impl Fn(&Call, usize) -> Result<(u16, Vec<u8>), HttpError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            history_polls: AtomicUsize::new(0),
            handler: Box::new(handler),
        }
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn submitted(&self) -> Option<Value> {
        self.calls()
            .into_iter()
            .find(|call| call.path == "/prompt")
            .and_then(|call| call.body)
            .map(|body| body["prompt"].clone())
    }
}
#[async_trait::async_trait]
impl Transport for Comfy {
    async fn send(&self, request: Request) -> Result<Response, HttpError> {
        let call = Call {
            method: if request.body().is_some() {
                "POST"
            } else {
                "GET"
            },
            path: request.url.path().to_owned(),
            query: request.url.query().map(str::to_owned),
            body: request.body().cloned(),
        };
        assert_eq!(request.url.host_str(), Some("127.0.0.1"));
        let polls = if call.path.starts_with("/history/") {
            self.history_polls.fetch_add(1, Ordering::SeqCst)
        } else {
            0
        };
        self.calls.lock().unwrap().push(call.clone());
        let (status, body) = (self.handler)(&call, polls)?;
        Ok(Response {
            status,
            body: Box::pin(std::io::Cursor::new(body)),
        })
    }
}
fn json_body(value: Value) -> Result<(u16, Vec<u8>), HttpError> {
    Ok((200, value.to_string().into_bytes()))
}

struct Hub {
    files: HashMap<String, Vec<u8>>,
    downloads: AtomicUsize,
}
#[async_trait::async_trait]
impl DownloadTransport for Hub {
    async fn get(&self, url: &str) -> Result<DownloadResponse, String> {
        self.downloads.fetch_add(1, Ordering::SeqCst);
        let parsed = url::Url::parse(url).unwrap();
        let segments: Vec<_> = parsed.path_segments().unwrap().collect();
        let revision = segments[3].to_owned();
        let file = segments[4..].join("/");
        let body = self.files.get(&file).cloned().ok_or("missing fixture")?;
        Ok(DownloadResponse {
            status: 200,
            commit: Some(revision),
            length: Some(body.len() as u64),
            body: Box::pin(std::io::Cursor::new(body)),
        })
    }
}

/// Bundled model with tiny, correctly pinned stand-in weights.
fn fixture(kind: &str) -> (GenerationModel, Hub) {
    fixture_query(kind)
}
fn fixture_query(query: &str) -> (GenerationModel, Hub) {
    let mut model = GenerationCatalog::bundled()
        .unwrap()
        .resolve(query)
        .unwrap()
        .clone();
    let mut files = HashMap::new();
    for file in &mut model.files {
        let content = format!("weights for {}", file.file).into_bytes();
        file.sha256 = format!("{:x}", Sha256::digest(&content));
        file.bytes = content.len() as u64;
        files.insert(file.file.clone(), content);
    }
    (
        model,
        Hub {
            files,
            downloads: AtomicUsize::new(0),
        },
    )
}
fn listing(model: &GenerationModel, windows: bool) -> HashMap<String, Value> {
    let mut listings: HashMap<String, Vec<String>> = HashMap::new();
    for file in &model.files {
        let (owner, repo) = file.repo.split_once('/').unwrap();
        let mut name = format!(
            "rigspark/comfyui/{owner}/{repo}@{}/{}",
            file.revision, file.file
        );
        if windows {
            name = name.replace('/', "\\");
        }
        listings
            .entry(format!("/models/{}", file.folder))
            .or_insert_with(|| vec!["other.safetensors".into()])
            .push(name);
    }
    listings
        .into_iter()
        .map(|(path, files)| (path, json!(files)))
        .collect()
}
fn completed(save: &str, filename: &str) -> Value {
    json!({"status": {"status_str": "success", "completed": true, "messages": []},
        "outputs": {save: {"images": [{"filename": filename, "subfolder": "", "type": "output"}]}}})
}
/// A healthy ComfyUI that completes on the second history poll.
fn healthy(model: &GenerationModel, windows: bool, entry: Value, media: &'static [u8]) -> Comfy {
    let listings = listing(model, windows);
    Comfy::new(move |call, polls| match call.path.as_str() {
        "/system_stats" => json_body(json!({"system": {"os": "posix"}, "devices": []})),
        "/prompt" => json_body(json!({"prompt_id": "abc-123", "number": 1, "node_errors": {}})),
        "/history/abc-123" if polls == 0 => json_body(json!({})),
        "/history/abc-123" => json_body(json!({"abc-123": entry.clone()})),
        "/view" => Ok((200, media.to_vec())),
        path => listings
            .get(path)
            .cloned()
            .map(json_body)
            .unwrap_or(Ok((404, Vec::new()))),
    })
}
fn client<'a>(http: &'a Comfy, hub: &'a Hub) -> ComfyUi<'a> {
    ComfyUi {
        http,
        download: hub,
        poll_interval: Duration::from_millis(1),
        deadline: Duration::from_secs(30),
        events: None,
    }
}
fn comfy_dir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("models")).unwrap();
    directory
}
fn request<'a>(
    model: &'a GenerationModel,
    comfy: &'a Path,
    output_dir: &'a Path,
    output: Option<&'a Path>,
) -> GenerationRequest<'a> {
    GenerationRequest {
        model,
        prompt: "a red fox in snow",
        seed: 42,
        endpoint: ENDPOINT,
        comfyui_dir: comfy,
        output,
        output_dir,
    }
}

#[tokio::test]
async fn image_generation_installs_verified_weights_and_saves_png() {
    let (model, hub) = fixture("image");
    let comfy = healthy(&model, false, completed("9", "rigspark_00001_.png"), PNG);
    let directory = comfy_dir();
    let out = tempfile::tempdir().unwrap();
    let outcome = client(&comfy, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(outcome.kind, GenerationKind::Image);
    assert_eq!(outcome.prompt_id, "abc-123");
    assert_eq!(outcome.seed, 42);
    assert_eq!(outcome.path, out.path().join("rigspark-image-abc-123.png"));
    assert_eq!(std::fs::read(&outcome.path).unwrap(), PNG);
    assert!(!outcome.weights[0].cached);
    let installed = directory
        .path()
        .join("models/checkpoints")
        .join(&outcome.weights[0].name);
    assert_eq!(
        std::fs::read(installed).unwrap(),
        hub.files[&model.files[0].file]
    );

    let graph = comfy.submitted().unwrap();
    ensure_local_only(&graph).unwrap();
    assert_eq!(
        graph["30"]["inputs"]["ckpt_name"],
        json!(outcome.weights[0].name)
    );
    assert_eq!(graph["31"]["inputs"]["seed"], 42);
    assert_eq!(graph["31"]["inputs"]["steps"], 4);
    assert_eq!(graph["6"]["inputs"]["text"], "a red fox in snow");
    let view = comfy
        .calls()
        .into_iter()
        .find(|call| call.path == "/view")
        .unwrap();
    assert_eq!(
        view.query.as_deref(),
        Some("filename=rigspark_00001_.png&subfolder=&type=output")
    );
    assert!(
        comfy
            .calls()
            .iter()
            .all(|call| !format!("{call:?}").contains("api_key"))
    );

    // Second run re-verifies cached weights and refuses to clobber the earlier output.
    let existing = outcome.path.clone();
    let error = client(&comfy, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), Some(&existing)),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, GenerationError::Exists(_)), "{error}");
    let second = client(&comfy, &hub)
        .generate(
            &request(
                &model,
                directory.path(),
                out.path(),
                Some(&out.path().join("again.png")),
            ),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(second.weights[0].cached);
    assert_eq!(hub.downloads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn video_generation_uses_comfyui_spelling_of_weight_names() {
    let (model, hub) = fixture("video");
    let comfy = healthy(&model, true, completed("28", "rigspark_00001_.webp"), WEBP);
    let directory = comfy_dir();
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("clip.webp");
    let outcome = client(&comfy, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), Some(&target)),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(outcome.path, target);
    assert_eq!(std::fs::read(&target).unwrap(), WEBP);
    assert_eq!(outcome.weights.len(), 3);
    let graph = comfy.submitted().unwrap();
    ensure_local_only(&graph).unwrap();
    for (node, input, role) in [
        ("37", "unet_name", FileRole::Diffusion),
        ("38", "clip_name", FileRole::TextEncoder),
        ("39", "vae_name", FileRole::Vae),
    ] {
        let name = graph[node]["inputs"][input].as_str().unwrap();
        assert!(name.contains('\\'), "{name}");
        assert!(
            outcome
                .weights
                .iter()
                .any(|w| w.role == role && w.name == name)
        );
    }
    assert_eq!(graph["40"]["inputs"]["length"], 49);
    assert_eq!(graph["28"]["class_type"], "SaveAnimatedWEBP");
}

#[tokio::test]
async fn split_flux_generation_loads_unet_dual_clip_and_vae_files() {
    let (model, hub) = fixture_query("flux1-schnell:fp16");
    let comfy = healthy(&model, true, completed("9", "rigspark_00001_.png"), PNG);
    let directory = comfy_dir();
    let out = tempfile::tempdir().unwrap();
    let outcome = client(&comfy, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap();

    assert_eq!(outcome.weights.len(), 4);
    let graph = comfy.submitted().unwrap();
    ensure_local_only(&graph).unwrap();
    assert_eq!(graph["12"]["class_type"], "UNETLoader");
    assert_eq!(graph["11"]["class_type"], "DualCLIPLoader");
    assert_eq!(graph["10"]["class_type"], "VAELoader");
    assert_eq!(graph["13"]["class_type"], "SamplerCustomAdvanced");
    assert_eq!(graph["17"]["inputs"]["steps"], 4);
    for (node, input) in [
        ("12", "unet_name"),
        ("11", "clip_name1"),
        ("11", "clip_name2"),
        ("10", "vae_name"),
    ] {
        assert!(
            graph[node]["inputs"][input]
                .as_str()
                .is_some_and(|name| name.contains('\\')),
            "{node}.{input}"
        );
    }
}

#[test]
fn only_allowlisted_local_nodes_are_submittable() {
    for kind in ["image", "video"] {
        let model = GenerationCatalog::bundled()
            .unwrap()
            .resolve(kind)
            .unwrap()
            .clone();
        let names = model
            .files
            .iter()
            .map(|file| (file.role, file.file.clone()))
            .collect();
        for devices in [vec![], vec!["mps".to_owned()]] {
            ensure_local_only(&workflow(&model, &names, "prompt", 1, &devices).unwrap()).unwrap();
        }
    }
    for graph in [
        json!({"1": {"class_type": "OpenAIDalle3", "inputs": {"prompt": "x"}}}),
        json!({"1": {"class_type": "KlingTextToVideoNode", "inputs": {}}}),
        json!({"1": {"class_type": "SaveImage"}}),
        json!({"1": {"inputs": {}}}),
        json!({}),
        json!([]),
    ] {
        assert!(ensure_local_only(&graph).is_err(), "{graph}");
    }
    assert!(matches!(
        ensure_local_only(&json!({"1": {"class_type": "GeminiImageNode", "inputs": {}}})),
        Err(GenerationError::NonLocalNode(class)) if class == "GeminiImageNode"
    ));
}

#[tokio::test]
async fn invalid_requests_fail_before_any_request_or_download() {
    let (model, hub) = fixture("image");
    let comfy = healthy(&model, false, completed("9", "x.png"), PNG);
    let directory = comfy_dir();
    let out = tempfile::tempdir().unwrap();
    let wrong_extension = out.path().join("image.webp");
    let cases: Vec<Box<Mutation>> = vec![
        Box::new(|r| r.endpoint = "http://192.168.1.5:8188"),
        Box::new(|r| r.endpoint = "https://comfy.example.com"),
        Box::new(|r| r.prompt = "   "),
        Box::new(|r| r.prompt = "bad\u{1b}[31m"),
        Box::new(|r| r.seed = u64::MAX),
    ];
    for mutate in cases {
        let mut value = request(&model, directory.path(), out.path(), None);
        mutate(&mut value);
        let error = client(&comfy, &hub)
            .generate(&value, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(error, GenerationError::Invalid(_)), "{error}");
    }
    let error = client(&comfy, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), Some(&wrong_extension)),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains(".png"), "{error}");
    let missing_models = tempfile::tempdir().unwrap();
    let error = client(&comfy, &hub)
        .generate(
            &request(&model, missing_models.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("models/"), "{error}");
    assert!(
        comfy
            .calls()
            .iter()
            .all(|call| call.path == "/system_stats")
    );
    assert_eq!(hub.downloads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn unreachable_or_foreign_listener_is_reported() {
    let (model, hub) = fixture("image");
    let directory = comfy_dir();
    let out = tempfile::tempdir().unwrap();
    let down = Comfy::new(|_, _| Err(HttpError::Transport));
    let error = client(&down, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, GenerationError::Unreachable(_)), "{error}");
    let foreign = Comfy::new(|_, _| json_body(json!({"version": "0.1"})));
    let error = client(&foreign, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("not ComfyUI"), "{error}");
    assert_eq!(hub.downloads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn integrity_mismatch_never_reaches_comfyui() {
    let (model, mut hub) = fixture("image");
    for content in hub.files.values_mut() {
        content[0] ^= 1;
    }
    let comfy = healthy(&model, false, completed("9", "x.png"), PNG);
    let directory = comfy_dir();
    let out = tempfile::tempdir().unwrap();
    let error = client(&comfy, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, GenerationError::Install(_)), "{error}");
    assert!(comfy.submitted().is_none());
}

#[tokio::test]
async fn weights_invisible_to_the_running_server_fail_closed() {
    let (model, hub) = fixture("image");
    let comfy = Comfy::new(|call, _| match call.path.as_str() {
        "/system_stats" => json_body(json!({"system": {}})),
        "/models/checkpoints" => json_body(json!(["someone-else.safetensors"])),
        _ => panic!("unexpected {call:?}"),
    });
    let directory = comfy_dir();
    let out = tempfile::tempdir().unwrap();
    let error = client(&comfy, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, GenerationError::NotVisible(_)), "{error}");
    assert!(error.to_string().contains("--comfyui-dir"));
}

#[tokio::test]
async fn validation_and_execution_errors_are_sanitised() {
    let (model, hub) = fixture("image");
    let listings = listing(&model, false);
    let rejected = Comfy::new(move |call, _| {
        match call.path.as_str() {
        "/system_stats" => json_body(json!({"system": {}})),
        "/prompt" => Ok((400, json!({"error": {"message": "Prompt outputs failed validation"},
            "node_errors": {"30": {"class_type": "CheckpointLoaderSimple",
                "errors": [{"message": "Value not in list", "details": "ckpt_name: 'x' not in []"}]}}})
            .to_string().into_bytes())),
        path => json_body(listings[path].clone()),
    }
    });
    let directory = comfy_dir();
    let out = tempfile::tempdir().unwrap();
    let error = client(&rejected, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, GenerationError::Rejected(_)), "{error}");
    assert!(error.to_string().contains("ckpt_name"), "{error}");

    let failed = healthy(
        &model,
        false,
        json!({"status": {"status_str": "error", "completed": false, "messages": [
            ["execution_start", {}],
            ["execution_error", {"node_type": "KSampler", "exception_message": "out of memory\u{1b}[31m"}]]}}),
        PNG,
    );
    let error = client(&failed, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, GenerationError::Failed(_)), "{error}");
    let text = error.to_string();
    assert!(
        text.contains("KSampler: out of memory") && !text.contains('\u{1b}'),
        "{text}"
    );
}

#[tokio::test]
async fn unsafe_or_wrong_media_outputs_are_refused() {
    let (model, hub) = fixture("image");
    let directory = comfy_dir();
    for (entry, media) in [
        (completed("9", "../../escape.png"), PNG),
        (completed("9", "image.svg"), PNG),
        (
            json!({"status": {"completed": true}, "outputs": {"9": {"images": [
                {"filename": "a.png", "subfolder": "../..", "type": "output"}]}}}),
            PNG,
        ),
        (
            json!({"status": {"completed": true}, "outputs": {"9": {"images": [
                {"filename": "a.png", "subfolder": "", "type": "temp"}]}}}),
            PNG,
        ),
        (
            completed("9", "a.png"),
            b"<html>not an image</html>".as_slice(),
        ),
        (json!({"status": {"completed": true}, "outputs": {}}), PNG),
    ] {
        let out = tempfile::tempdir().unwrap();
        let comfy = healthy(&model, false, entry.clone(), media);
        let error = client(&comfy, &hub)
            .generate(
                &request(&model, directory.path(), out.path(), None),
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(error, GenerationError::Response(_)),
            "{entry}: {error}"
        );
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0, "{entry}");
    }
}

#[tokio::test]
async fn cancellation_removes_the_queued_prompt() {
    let (model, hub) = fixture("image");
    let listings = listing(&model, false);
    let comfy = Comfy::new(move |call, _| match call.path.as_str() {
        "/system_stats" => json_body(json!({"system": {}})),
        "/prompt" => json_body(json!({"prompt_id": "abc-123", "node_errors": {}})),
        "/history/abc-123" => json_body(json!({})),
        "/queue" | "/interrupt" => json_body(json!({})),
        path => json_body(listings[path].clone()),
    });
    let directory = comfy_dir();
    let out = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let comfy_ref = &comfy;
    let watcher = async move {
        while comfy_ref.history_polls.load(Ordering::SeqCst) < 3 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        trigger.cancel();
    };
    let ui = client(&comfy, &hub);
    let value = request(&model, directory.path(), out.path(), None);
    let (result, ()) = tokio::join!(ui.generate(&value, &cancel), watcher);
    assert!(
        matches!(result, Err(GenerationError::Cancelled)),
        "{result:?}"
    );
    let calls = comfy.calls();
    let queue = calls.iter().find(|call| call.path == "/queue").unwrap();
    assert_eq!(queue.body, Some(json!({"delete": ["abc-123"]})));
    assert!(
        calls
            .iter()
            .any(|call| call.path == "/interrupt" && call.method == "POST")
    );
}

/// Live testing on Apple M4 Max: Wan's `uni_pc` sampler diverged into noise on MPS at 30 steps,
/// while `euler` at the official size produced coherent video. Other devices keep `uni_pc`.
#[tokio::test]
async fn wan_uses_euler_on_apple_mps_and_uni_pc_elsewhere() {
    for (devices, sampler) in [
        (json!([{"name": "mps", "type": "mps"}]), "euler"),
        (json!([{"name": "cuda:0 NVIDIA", "type": "cuda"}]), "uni_pc"),
        (json!([]), "uni_pc"),
    ] {
        let (model, hub) = fixture("video");
        let listings = listing(&model, false);
        let reported = devices.clone();
        let comfy = Comfy::new(move |call, polls| match call.path.as_str() {
            "/system_stats" => json_body(json!({"system": {}, "devices": reported.clone()})),
            "/prompt" => json_body(json!({"prompt_id": "abc-123", "node_errors": {}})),
            "/history" => json_body(json!({})),
            "/history/abc-123" if polls == 0 => json_body(json!({})),
            "/history/abc-123" => json_body(json!({"abc-123": completed("28", "v.webp")})),
            "/view" => Ok((200, WEBP.to_vec())),
            path => json_body(listings[path].clone()),
        });
        let directory = comfy_dir();
        let out = tempfile::tempdir().unwrap();
        let messages = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = messages.clone();
        let mut ui = client(&comfy, &hub);
        ui.events = Some(std::sync::Arc::new(move |line: String| {
            sink.lock().unwrap().push(line)
        }));
        ui.generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let graph = comfy.submitted().unwrap();
        assert_eq!(graph["3"]["inputs"]["sampler_name"], sampler, "{devices}");
        assert_eq!(graph["3"]["inputs"]["steps"], 30);
        assert_eq!(
            graph["40"]["inputs"]["length"], 49,
            "size stays official and duration is approximately three seconds"
        );
        let noted = messages
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains("Apple MPS"));
        assert_eq!(noted, sampler == "euler", "{devices}");
    }
}

#[tokio::test]
async fn flux_sampler_is_unchanged_on_mps() {
    let (model, hub) = fixture("image");
    let listings = listing(&model, false);
    let comfy = Comfy::new(move |call, polls| match call.path.as_str() {
        "/system_stats" => json_body(json!({"system": {}, "devices": [{"type": "mps"}]})),
        "/prompt" => json_body(json!({"prompt_id": "abc-123", "node_errors": {}})),
        "/history" => json_body(json!({})),
        "/history/abc-123" if polls == 0 => json_body(json!({})),
        "/history/abc-123" => json_body(json!({"abc-123": completed("9", "i.png")})),
        "/view" => Ok((200, PNG.to_vec())),
        path => json_body(listings[path].clone()),
    });
    let directory = comfy_dir();
    let out = tempfile::tempdir().unwrap();
    client(&comfy, &hub)
        .generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        comfy.submitted().unwrap()["31"]["inputs"]["sampler_name"],
        "euler"
    );
}

/// ComfyUI with `free` bytes reported and a previous prompt that loaded `previous` weights.
fn under_pressure(
    model: &GenerationModel,
    device: &'static str,
    free: u64,
    previous: Option<Value>,
) -> Comfy {
    let listings = listing(model, false);
    Comfy::new(move |call, polls| match call.path.as_str() {
        "/system_stats" => json_body(json!({"system": {"ram_free": free},
            "devices": [{"type": device, "vram_free": free}]})),
        "/history" => json_body(match &previous {
            Some(graph) => json!({"old-1": {"prompt": [1, "old-1", graph, {}, []], "outputs": {}}}),
            None => json!({}),
        }),
        "/free" => json_body(json!({})),
        "/prompt" => json_body(json!({"prompt_id": "abc-123", "node_errors": {}})),
        "/history/abc-123" if polls == 0 => json_body(json!({})),
        "/history/abc-123" => json_body(json!({"abc-123": completed("28", "v.webp")})),
        "/view" => Ok((200, WEBP.to_vec())),
        path => json_body(listings[path].clone()),
    })
}

/// Live testing on a 36 GB Apple M4 Max: Wan sampled ~4x slower (and swapped) beside a resident
/// FLUX even though ComfyUI reported more free memory than Wan's weights. Unified memory therefore
/// always unloads on a workflow switch; discrete GPUs only when free memory is below the weights.
#[tokio::test]
async fn previous_workflow_weights_are_unloaded_on_switch_when_memory_is_shared_or_short() {
    let flux = json!({"30": {"class_type": "CheckpointLoaderSimple",
        "inputs": {"ckpt_name": "rigspark/comfyui/Comfy-Org/flux1-schnell@x/flux1-schnell-fp8.safetensors"}}});
    let (video, _) = fixture("video");
    let ours = |model: &GenerationModel| {
        let names: serde_json::Map<String, Value> = listing(model, false)
            .values()
            .enumerate()
            .map(|(index, list)| {
                (
                    index.to_string(),
                    json!({"class_type": "UNETLoader", "inputs": {"unet_name": list[1]}}),
                )
            })
            .collect();
        Value::Object(names)
    };
    let need = video.total_bytes();
    for (device, free, previous, expect_free) in [
        ("cuda", need / 2, Some(flux.clone()), true),
        ("cuda", need * 4, Some(flux.clone()), false),
        ("mps", need * 4, Some(flux.clone()), true),
        ("mps", need / 2, Some(ours(&video)), false),
        ("mps", need / 2, None, false),
    ] {
        let (model, hub) = fixture("video");
        let comfy = under_pressure(&model, device, free, previous.clone());
        let directory = comfy_dir();
        let out = tempfile::tempdir().unwrap();
        let messages = std::sync::Arc::new(Mutex::new(Vec::new()));
        let sink = messages.clone();
        let mut ui = client(&comfy, &hub);
        ui.events = Some(std::sync::Arc::new(move |line: String| {
            sink.lock().unwrap().push(line)
        }));
        ui.generate(
            &request(&model, directory.path(), out.path(), None),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        let calls = comfy.calls();
        let freed = calls.iter().find(|call| call.path == "/free");
        assert_eq!(
            freed.is_some(),
            expect_free,
            "{device} free={free} previous={previous:?}"
        );
        if let Some(call) = freed {
            assert_eq!(
                call.body,
                Some(json!({"unload_models": true, "free_memory": true}))
            );
            let free_index = calls.iter().position(|call| call.path == "/free").unwrap();
            let prompt_index = calls
                .iter()
                .position(|call| call.path == "/prompt")
                .unwrap();
            assert!(free_index < prompt_index, "unload must precede submission");
            assert!(
                messages
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|line| line.contains("Freeing ComfyUI memory"))
            );
        }
        if device != "mps" && free >= need {
            assert!(
                calls.iter().all(|call| call.path != "/history"),
                "discrete GPUs with room never read history"
            );
        } else {
            let history = calls.iter().find(|call| call.path == "/history").unwrap();
            assert_eq!(history.query.as_deref(), Some("max_items=1"));
        }
    }
}

#[test]
fn wan22_and_qwen_image_graphs_follow_the_official_examples_and_stay_local() {
    use rigspark_core::generation::Workflow;
    let catalog = GenerationCatalog::bundled().unwrap();
    let build = |workflow: Workflow, devices: &[String]| {
        let mut model = catalog.resolve("video").unwrap().clone();
        model.workflow = Some(workflow);
        let names = model
            .files
            .iter()
            .map(|file| (file.role, file.file.clone()))
            .collect();
        let graph = workflow_graph(&model, &names, devices);
        ensure_local_only(&graph).unwrap();
        graph
    };
    let wan = build(Workflow::Wan22Ti2v, &[]);
    assert_eq!(wan["55"]["class_type"], "Wan22ImageToVideoLatent");
    assert_eq!(wan["55"]["inputs"]["width"], 1280);
    assert_eq!(wan["55"]["inputs"]["height"], 704);
    assert_eq!(wan["3"]["inputs"]["cfg"], 5.0);
    assert_eq!(wan["3"]["inputs"]["sampler_name"], "uni_pc");
    assert_eq!(wan["28"]["class_type"], "SaveAnimatedWEBP");
    assert_eq!(
        build(Workflow::Wan22Ti2v, &["mps".to_owned()])["3"]["inputs"]["sampler_name"],
        "euler",
        "uni_pc diverges on Apple MPS"
    );
    let qwen = build(Workflow::QwenImage, &[]);
    assert_eq!(qwen["66"]["class_type"], "ModelSamplingAuraFlow");
    assert_eq!(qwen["66"]["inputs"]["shift"], 3.1);
    assert_eq!(qwen["58"]["inputs"]["width"], 1328);
    assert_eq!(qwen["38"]["inputs"]["type"], "qwen_image");
    assert_eq!(qwen["3"]["inputs"]["steps"], 20);
    assert_eq!(qwen["60"]["class_type"], "SaveImage");

    let mut fit_only = catalog.resolve("video").unwrap().clone();
    fit_only.workflow = None;
    let names = fit_only
        .files
        .iter()
        .map(|file| (file.role, file.file.clone()))
        .collect();
    let refusal = workflow(&fit_only, &names, "prompt", 1, &[])
        .unwrap_err()
        .to_string();
    assert!(refusal.contains("workflow coming"), "{refusal}");
}

fn workflow_graph(
    model: &rigspark_core::generation::GenerationModel,
    names: &std::collections::HashMap<rigspark_core::generation::FileRole, String>,
    devices: &[String],
) -> serde_json::Value {
    workflow(model, names, "a fox on a volcano", 7, devices).unwrap()
}
