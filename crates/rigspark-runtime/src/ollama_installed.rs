use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum OllamaError {
    #[error("invalid Ollama input or response: {0}")]
    Invalid(&'static str),
    #[error("installed model integrity verification failed: {0}")]
    Integrity(&'static str),
    #[error("Ollama request failed: {0}")]
    Request(String),
}
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledModel {
    pub id: String,
    pub digest: String,
    pub size_bytes: u64,
    pub quant: Option<String>,
    pub context_length: Option<u32>,
    pub kv_bytes_per_token: Option<u64>,
    pub capabilities: Vec<String>,
}
pub fn size_installed(
    model: &InstalledModel,
    hardware: &rigspark_core::sizing::Hardware,
    context: Option<u32>,
) -> Result<Value, OllamaError> {
    crate::hardware::validate_hardware(hardware).map_err(|_| OllamaError::Invalid("hardware"))?;
    if context.is_some_and(|value| !(1..=10000000).contains(&value)) {
        return Err(OllamaError::Invalid("context"));
    }
    let (kind, capacity) = rigspark_core::sizing::memory_capacity(hardware);
    let usable = capacity * 0.85;
    let weights_fit = model.size_bytes as f64 <= usable;
    let required = context
        .zip(model.kv_bytes_per_token)
        .map(|(context, kv)| model.size_bytes as f64 + kv as f64 * f64::from(context));
    let over_cap = context
        .zip(model.context_length)
        .is_some_and(|(context, cap)| context > cap);
    let fit = if over_cap || !weights_fit || required.is_some_and(|required| required > usable) {
        "no"
    } else if required.is_none() {
        "unknown"
    } else {
        "yes"
    };
    let mut value =
        serde_json::to_value(model).map_err(|_| OllamaError::Invalid("installed model"))?;
    for (key, entry) in [
        ("context", serde_json::json!(context)),
        ("fit", serde_json::json!(fit)),
        ("weightsFit", serde_json::json!(weights_fit)),
        ("requiredBytes", serde_json::json!(required)),
        ("usableBytes", serde_json::json!(usable)),
        ("memoryKind", serde_json::json!(kind)),
        ("evidence", serde_json::json!("local-runtime-metadata")),
        ("throughput", serde_json::json!("unknown")),
    ] {
        value[key] = entry;
    }
    Ok(value)
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
pub(crate) fn model_path(root: &Path, id: &str) -> Result<PathBuf, OllamaError> {
    let mut segments: Vec<_> = id.split('/').collect();
    if segments.len() > 3 {
        return Err(OllamaError::Invalid("installed model path"));
    }
    let last = segments.pop().ok_or(OllamaError::Invalid("model id"))?;
    let parts: Vec<_> = last.split(':').collect();
    if parts.len() > 2 {
        return Err(OllamaError::Invalid("model tag"));
    }
    let name = parts[0];
    let tag = parts.get(1).copied().unwrap_or("latest");
    let namespace = segments.pop().unwrap_or("library");
    let registry = segments.pop().unwrap_or("registry.ollama.ai");
    let safe = |value: &str| {
        !value.is_empty()
            && value.as_bytes()[0].is_ascii_alphanumeric()
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            })
    };
    if ![registry, namespace, name, tag].into_iter().all(safe) {
        return Err(OllamaError::Invalid("installed model path"));
    }
    Ok(root
        .join("manifests")
        .join(registry)
        .join(namespace)
        .join(name)
        .join(tag))
}
pub fn parse_inventory(value: Value) -> Result<Vec<InstalledModel>, OllamaError> {
    #[derive(Deserialize)]
    struct Details {
        quantization_level: Option<String>,
    }
    #[derive(Deserialize)]
    struct Entry {
        name: String,
        digest: String,
        size: u64,
        remote_host: Option<String>,
        remote_model: Option<String>,
        details: Option<Details>,
    }
    #[derive(Deserialize)]
    struct Inventory {
        models: Vec<Entry>,
    }
    let inventory: Inventory =
        serde_json::from_value(value).map_err(|_| OllamaError::Invalid("inventory"))?;
    if inventory.models.len() > 10000 {
        return Err(OllamaError::Invalid("inventory exceeds model limit"));
    }
    let mut result = Vec::new();
    for entry in inventory.models {
        if entry.name.is_empty()
            || entry.name.len() > 256
            || !digest(&entry.digest)
            || entry.size == 0
            || entry.size > 9007199254740991
            || entry
                .details
                .as_ref()
                .and_then(|details| details.quantization_level.as_ref())
                .is_some_and(|value| value.len() > 100)
        {
            return Err(OllamaError::Invalid("inventory entry"));
        }
        if entry
            .remote_host
            .as_ref()
            .is_some_and(|value| !value.is_empty())
            || entry
                .remote_model
                .as_ref()
                .is_some_and(|value| !value.is_empty())
        {
            continue;
        }
        model_path(Path::new(""), &entry.name)?;
        result.push(InstalledModel {
            id: entry.name,
            digest: entry.digest,
            size_bytes: entry.size,
            quant: entry.details.and_then(|details| details.quantization_level),
            context_length: None,
            kv_bytes_per_token: None,
            capabilities: Vec::new(),
        });
    }
    Ok(result)
}
#[derive(Deserialize)]
pub struct Metadata {
    #[serde(default)]
    pub model_info: BTreeMap<String, Value>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub parameters: Option<String>,
    pub remote_host: Option<String>,
}
pub fn parse_metadata(value: Value) -> Result<Metadata, OllamaError> {
    let metadata: Metadata =
        serde_json::from_value(value).map_err(|_| OllamaError::Invalid("model metadata"))?;
    if metadata
        .remote_host
        .as_ref()
        .is_some_and(|value| !value.is_empty())
        || metadata.capabilities.len() > 100
        || metadata.capabilities.iter().any(|value| value.len() > 100)
        || metadata
            .parameters
            .as_ref()
            .is_some_and(|value| value.len() > 100000)
    {
        return Err(OllamaError::Invalid("local model metadata"));
    }
    Ok(metadata)
}
pub fn inspect_metadata(
    mut model: InstalledModel,
    value: Value,
) -> Result<InstalledModel, OllamaError> {
    let metadata = parse_metadata(value)?;
    let family = metadata
        .model_info
        .get("general.architecture")
        .and_then(Value::as_str)
        .unwrap_or("");
    let number = |key: &str, maximum: u64| {
        metadata
            .model_info
            .get(&format!("{family}.{key}"))
            .and_then(Value::as_u64)
            .filter(|value| *value > 0 && *value <= maximum)
    };
    model.context_length = number("context_length", 10000000).map(|value| value as u32);
    if ["llama", "qwen2"].contains(&family) {
        model.kv_bytes_per_token = (|| {
            let layers = number("block_count", 100000)?;
            let heads = number("attention.head_count_kv", 100000)?;
            let key = number("attention.key_length", 100000)?;
            let value = number("attention.value_length", 100000)?;
            layers
                .checked_mul(heads)?
                .checked_mul(key + value)?
                .checked_mul(2)
        })();
    }
    model.capabilities = metadata.capabilities;
    Ok(model)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Layer {
    digest: String,
    size: u64,
    media_type: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u8,
    config: Layer,
    layers: Vec<Layer>,
}
fn contained(root: &Path, path: &Path) -> Result<(), OllamaError> {
    let canonical = path
        .canonicalize()
        .map_err(|_| OllamaError::Integrity("missing local content"))?;
    if !canonical.starts_with(root) {
        return Err(OllamaError::Integrity("local path escaped model root"));
    }
    Ok(())
}
pub async fn verify_manifest(
    root: &Path,
    id: &str,
    expected_manifest: &str,
    expected_weight: Option<&str>,
    expected_bytes: Option<u64>,
    cancel: &CancellationToken,
) -> Result<(), OllamaError> {
    verify_catalog_manifest(
        root,
        id,
        expected_manifest,
        expected_weight,
        expected_bytes,
        &[],
        cancel,
    )
    .await
}

const EMBEDDED_MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

/// Digests that identify a local Ollama manifest, the primary one first. In Ollama 0.40+'s
/// content-addressed store that is the manifest blob's digest; in the older store it is the
/// manifest file's SHA-256, followed by any embedded upstream manifest whose blob verifies.
pub fn manifest_digests(root: &Path, id: &str) -> Result<Vec<String>, OllamaError> {
    Ok(read_manifest(root, id)?.1)
}

/// The local manifest for `id` and the digests that identify it.
fn read_manifest(root: &Path, id: &str) -> Result<(String, Vec<String>), OllamaError> {
    let root = root
        .canonicalize()
        .map_err(|_| OllamaError::Integrity("model root unavailable"))?;
    if let Some(found) = read_v2_manifest(&root, id)? {
        return Ok(found);
    }
    let path = model_path(&root, id)?;
    contained(&root, &path)?;
    let raw = crate::state::secure_read(&path, 4 * 1024 * 1024, false)
        .map_err(|_| OllamaError::Integrity("manifest unavailable"))?;
    let digests = identities(&root, &raw);
    Ok((raw, digests))
}

/// Ollama 0.40+ links `manifests-v2/<registry>/<namespace>/<name>/<tag>` to the manifest's
/// content-addressed blob. Only a link to exactly `blobs/sha256-<hex>` whose content hashes
/// to `<hex>` is accepted.
fn read_v2_manifest(root: &Path, id: &str) -> Result<Option<(String, Vec<String>)>, OllamaError> {
    let v1 = model_path(root, id)?;
    let relative = v1
        .strip_prefix(root.join("manifests"))
        .map_err(|_| OllamaError::Invalid("installed model path"))?;
    let mut parts: Vec<_> = relative.components().map(|part| part.as_os_str()).collect();
    if parts
        .first()
        .is_some_and(|registry| *registry == "registry.ollama.ai")
    {
        parts[0] = std::ffi::OsStr::new("ollama.com");
    }
    let link = parts
        .iter()
        .fold(root.join("manifests-v2"), |path, part| path.join(part));
    let target = match std::fs::read_link(&link) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(OllamaError::Integrity("manifest link unreadable")),
    };
    let target = target.to_string_lossy();
    let hex = target
        .strip_prefix("../../../../blobs/sha256-")
        .filter(|hex| digest(hex))
        .ok_or(OllamaError::Integrity("manifest link escapes blobs"))?;
    let blob = root.join("blobs").join(format!("sha256-{hex}"));
    contained(root, &blob)?;
    let raw = crate::state::secure_read(&blob, 4 * 1024 * 1024, false)
        .map_err(|_| OllamaError::Integrity("manifest unavailable"))?;
    if format!("{:x}", Sha256::digest(raw.as_bytes())) != hex {
        return Err(OllamaError::Integrity(
            "manifest does not match its address",
        ));
    }
    Ok(Some((raw, vec![hex.to_owned()])))
}

fn identities(root: &Path, raw: &str) -> Vec<String> {
    let mut digests = vec![format!("{:x}", Sha256::digest(raw.as_bytes()))];
    let Ok(manifest) = serde_json::from_str::<Manifest>(raw) else {
        return digests;
    };
    for layer in manifest
        .layers
        .iter()
        .filter(|layer| layer.media_type == EMBEDDED_MANIFEST && layer.size <= 64 * 1024)
    {
        let Some(hex) = layer
            .digest
            .strip_prefix("sha256:")
            .filter(|hex| digest(hex))
        else {
            continue;
        };
        let blob = root.join("blobs").join(format!("sha256-{hex}"));
        let verified = contained(root, &blob).is_ok()
            && crate::state::secure_read(&blob, 64 * 1024, false)
                .is_ok_and(|text| format!("{:x}", Sha256::digest(text.as_bytes())) == hex);
        if verified && !digests.iter().any(|known| known == hex) {
            digests.push(hex.to_owned());
        }
    }
    digests
}

pub async fn verify_catalog_manifest(
    root: &Path,
    id: &str,
    expected_manifest: &str,
    expected_weight: Option<&str>,
    expected_bytes: Option<u64>,
    projectors: &[rigspark_core::sizing::ProjectorArtifact],
    cancel: &CancellationToken,
) -> Result<(), OllamaError> {
    if !digest(expected_manifest)
        || expected_weight.is_some_and(|value| !digest(value))
        || expected_bytes.is_some_and(|value| value == 0 || value > 9007199254740991)
    {
        return Err(OllamaError::Invalid("expected integrity"));
    }
    if cancel.is_cancelled() {
        return Err(OllamaError::Request("cancelled".into()));
    }
    let root = root
        .canonicalize()
        .map_err(|_| OllamaError::Integrity("model root unavailable"))?;
    let (raw, digests) = read_manifest(&root, id)?;
    if !digests.iter().any(|known| known == expected_manifest) {
        return Err(OllamaError::Integrity("manifest digest mismatch"));
    }
    let manifest: Manifest =
        serde_json::from_str(&raw).map_err(|_| OllamaError::Invalid("manifest"))?;
    if manifest.schema_version != 2 || manifest.layers.is_empty() || manifest.layers.len() > 10000 {
        return Err(OllamaError::Invalid("manifest layers"));
    }
    let weights: Vec<_> = manifest
        .layers
        .iter()
        .filter(|layer| layer.media_type == "application/vnd.ollama.image.model")
        .collect();
    if !projectors.is_empty() {
        let actual: Vec<_> = manifest
            .layers
            .iter()
            .filter(|layer| layer.media_type == "application/vnd.ollama.image.projector")
            .collect();
        let mut digests = std::collections::HashSet::new();
        if projectors.len() > 16
            || actual.len() != projectors.len()
            || projectors.iter().any(|expected| {
                !digest(&expected.sha256)
                    || expected.bytes == 0
                    || !digests.insert(&expected.sha256)
                    || !actual.iter().any(|layer| {
                        layer.digest == format!("sha256:{}", expected.sha256)
                            && layer.size == expected.bytes
                    })
            })
        {
            return Err(OllamaError::Integrity("catalog projector mismatch"));
        }
    }
    if weights.is_empty() {
        return Err(OllamaError::Integrity("manifest has no model weights"));
    }
    if expected_weight.is_some_and(|expected| {
        !weights
            .iter()
            .any(|layer| layer.digest == format!("sha256:{expected}"))
    }) {
        return Err(OllamaError::Integrity("catalog digest mismatch"));
    }
    let total = manifest
        .layers
        .iter()
        .filter(|layer| {
            layer.media_type == "application/vnd.ollama.image.model"
                || layer.media_type == "application/vnd.ollama.image.projector"
        })
        .try_fold(0u64, |total, layer| total.checked_add(layer.size))
        .ok_or(OllamaError::Invalid("weight size overflow"))?;
    if expected_bytes.is_some_and(|expected| total < expected.div_ceil(2)) {
        return Err(OllamaError::Integrity("catalog size floor"));
    }
    for layer in std::iter::once(&manifest.config).chain(&manifest.layers) {
        let sha = layer
            .digest
            .strip_prefix("sha256:")
            .filter(|value| digest(value))
            .ok_or(OllamaError::Invalid("blob digest"))?;
        if layer.size > 9007199254740991 {
            return Err(OllamaError::Invalid("blob size"));
        }
        let blob = root.join("blobs").join(format!("sha256-{sha}"));
        contained(&root, &blob)?;
        let (actual, bytes) = crate::acquire::hash_file(&blob, cancel)
            .await
            .map_err(|_| OllamaError::Integrity("blob unavailable or cancelled"))?;
        if actual != sha || bytes != layer.size {
            return Err(OllamaError::Integrity("blob digest or size mismatch"));
        }
    }
    if read_manifest(&root, id)?.0 != raw {
        return Err(OllamaError::Integrity(
            "manifest changed during verification",
        ));
    }
    Ok(())
}

#[async_trait::async_trait]
pub trait InstalledVerifier: Send + Sync {
    async fn verify(
        &self,
        id: &str,
        digest: &str,
        cancel: &CancellationToken,
    ) -> Result<(), OllamaError>;
}
pub struct LocalVerifier {
    pub root: PathBuf,
}
#[async_trait::async_trait]
impl InstalledVerifier for LocalVerifier {
    async fn verify(
        &self,
        id: &str,
        digest: &str,
        cancel: &CancellationToken,
    ) -> Result<(), OllamaError> {
        verify_manifest(&self.root, id, digest, None, None, cancel).await
    }
}
pub struct InstalledModels<'runtime> {
    transport: &'runtime dyn crate::http::Transport,
    verifier: &'runtime dyn InstalledVerifier,
}
impl<'runtime> InstalledModels<'runtime> {
    pub fn new(
        transport: &'runtime dyn crate::http::Transport,
        verifier: &'runtime dyn InstalledVerifier,
    ) -> Self {
        Self {
            transport,
            verifier,
        }
    }
    async fn request(
        &self,
        endpoint: &str,
        path: &str,
        body: Option<Value>,
        cancel: &CancellationToken,
    ) -> Result<Value, OllamaError> {
        let request = crate::http::Request::new(endpoint, path, body, None)
            .map_err(|error| OllamaError::Request(error.to_string()))?;
        crate::http::read_json(self.transport, request, cancel, 4 * 1024 * 1024)
            .await
            .map_err(|error| OllamaError::Request(error.to_string()))
    }
    pub async fn list(
        &self,
        endpoint: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<InstalledModel>, OllamaError> {
        parse_inventory(self.request(endpoint, "/api/tags", None, cancel).await?)
    }
    pub async fn inspect(
        &self,
        endpoint: &str,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<InstalledModel, OllamaError> {
        model_path(Path::new(""), id)?;
        let selected = exact(self.list(endpoint, cancel).await?, id)?;
        inspect_metadata(
            selected,
            self.request(
                endpoint,
                "/api/show",
                Some(serde_json::json!({"model":id})),
                cancel,
            )
            .await?,
        )
    }
    pub async fn activate(
        &self,
        endpoint: &str,
        model: &InstalledModel,
        context: Option<u32>,
        cancel: &CancellationToken,
    ) -> Result<String, OllamaError> {
        self.activate_with_id(
            endpoint,
            model,
            context,
            &uuid::Uuid::new_v4().to_string(),
            cancel,
        )
        .await
    }
    pub async fn activate_with_id(
        &self,
        endpoint: &str,
        model: &InstalledModel,
        context: Option<u32>,
        unique: &str,
        cancel: &CancellationToken,
    ) -> Result<String, OllamaError> {
        crate::state::loopback(endpoint).map_err(|_| OllamaError::Invalid("endpoint"))?;
        model_path(Path::new(""), &model.id)?;
        if !digest(&model.digest)
            || context.is_some_and(|context| !(1..=10000000).contains(&context))
        {
            return Err(OllamaError::Invalid("activation metadata"));
        }
        let variant = context.map(|context| format!("llmup-context-{unique}:{context}"));
        if let Some(variant) = &variant {
            model_path(Path::new(""), variant)?;
            if variant.len() > 256 {
                return Err(OllamaError::Invalid("variant name"));
            }
        }
        self.verifier
            .verify(&model.id, &model.digest, cancel)
            .await?;
        if exact(self.list(endpoint, cancel).await?, &model.id)?.digest != model.digest {
            return Err(OllamaError::Integrity("source manifest changed; retry"));
        }
        let Some(context) = context else {
            return Ok(model.id.clone());
        };
        let variant = variant.ok_or(OllamaError::Invalid("variant name"))?;
        let created=self.request(endpoint,"/api/create",Some(serde_json::json!({"model":variant,"from":model.id,"parameters":{"num_ctx":context},"stream":false})),cancel).await?;
        if created.get("status").and_then(Value::as_str) != Some("success") {
            return Err(OllamaError::Request(
                "context variant creation failed".into(),
            ));
        }
        let metadata = parse_metadata(
            self.request(
                endpoint,
                "/api/show",
                Some(serde_json::json!({"model":variant})),
                cancel,
            )
            .await?,
        )?;
        if !metadata.parameters.as_ref().is_some_and(|parameters| {
            parameters.lines().any(|line| {
                let mut parts = line.split_whitespace();
                parts.next() == Some("num_ctx")
                    && parts.next().and_then(|value| value.parse::<u32>().ok()) == Some(context)
            })
        }) {
            return Err(OllamaError::Integrity(
                "runtime did not retain requested context",
            ));
        }
        let installed = exact(self.list(endpoint, cancel).await?, &variant)?;
        self.verifier
            .verify(&installed.id, &installed.digest, cancel)
            .await?;
        if exact(self.list(endpoint, cancel).await?, &model.id)?.digest != model.digest {
            return Err(OllamaError::Integrity(
                "source changed during context activation",
            ));
        }
        Ok(variant)
    }
}
fn exact(models: Vec<InstalledModel>, id: &str) -> Result<InstalledModel, OllamaError> {
    let mut matching = models.into_iter().filter(|model| model.id == id);
    let selected = matching
        .next()
        .ok_or(OllamaError::Invalid("model not installed locally"))?;
    if matching.next().is_some() {
        return Err(OllamaError::Invalid("ambiguous installed model"));
    }
    Ok(selected)
}
