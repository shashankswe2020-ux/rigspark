use crate::{
    acquire::{Acquisition, Artifact, DownloadTransport, SizePolicy, hash_file},
    adapters::{BackendAdapter, BackendError, ServeRequest, model_id},
    application::events::{LifecycleObserver, LifecycleScope, LifecycleStage, observe},
    command::{CommandRunner, OllamaCommandContext},
    ollama_installed::verify_catalog_manifest,
};
use rigspark_core::catalog::{GgufSource, MlxSource};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub async fn with_ollama_daemon<T>(
    adapter: &dyn BackendAdapter,
    endpoint: &str,
    cancel: &CancellationToken,
    operation: impl std::future::Future<Output = Result<T, BackendError>>,
) -> Result<T, BackendError> {
    if adapter.name() != "ollama" || cancel.is_cancelled() {
        return Err(BackendError(
            "invalid or cancelled acquisition daemon request".into(),
        ));
    }
    let request = ServeRequest {
        draft_model_path: None,
        model_id: "llmup-acquisition".into(),
        endpoint: endpoint.into(),
        model_path: None,
        context: None,
        cache: None,
    };
    let handle = adapter.serve(&request, cancel).await?;
    let result = tokio::select! { biased; _ = cancel.cancelled() => Err(BackendError("pull cancelled".into())), result = operation => result };
    if handle.owned_by_us {
        let cleanup = tokio::time::timeout(
            Duration::from_secs(15),
            adapter.stop(&handle, &CancellationToken::new()),
        )
        .await;
        let failure = match cleanup {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(error.0),
            Err(_) => Some("cleanup timed out".into()),
        };
        if let Some(failure) = failure {
            return Err(BackendError(format!(
                "{}; acquisition daemon cleanup failed (pid {:?}): {failure}",
                result
                    .as_ref()
                    .err()
                    .map(|error| error.0.as_str())
                    .unwrap_or("pull completed"),
                handle.pid
            )));
        }
    }
    result
}

#[derive(Clone)]
pub struct PullRequest {
    pub backend: String,
    pub model_id: String,
    pub expected_bytes: u64,
    pub expected_sha256: Option<String>,
    pub projectors: Vec<rigspark_core::sizing::ProjectorArtifact>,
    pub gguf: Option<GgufSource>,
    pub mlx: Option<MlxSource>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedModel {
    pub model_id: String,
    pub model_path: Option<PathBuf>,
    pub digest_verified: bool,
    pub local_manifest_digest: Option<String>,
}
pub struct PullService<'runtime> {
    pub acquisition: &'runtime Acquisition,
    pub download: &'runtime dyn DownloadTransport,
    pub commands: &'runtime dyn CommandRunner,
    pub ollama_models: &'runtime Path,
    pub studio_models: &'runtime Path,
}
impl PullService<'_> {
    pub async fn pull(
        &self,
        request: &PullRequest,
        binary: &Path,
        cancel: &CancellationToken,
    ) -> Result<PreparedModel, BackendError> {
        self.pull_at(request, binary, "http://127.0.0.1:11434", cancel)
            .await
    }
    pub async fn pull_at(
        &self,
        request: &PullRequest,
        binary: &Path,
        endpoint: &str,
        cancel: &CancellationToken,
    ) -> Result<PreparedModel, BackendError> {
        self.pull_at_observed(request, binary, endpoint, cancel, None)
            .await
    }
    pub async fn pull_at_observed(
        &self,
        request: &PullRequest,
        binary: &Path,
        endpoint: &str,
        cancel: &CancellationToken,
        observer: Option<&LifecycleObserver>,
    ) -> Result<PreparedModel, BackendError> {
        model_id(&request.model_id)?;
        let mut projector_digests = std::collections::HashSet::new();
        let projector_bytes = request
            .projectors
            .iter()
            .try_fold(0_u64, |total, projector| {
                if projector.bytes == 0
                    || projector.sha256.len() != 64
                    || !projector
                        .sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                    || !projector_digests.insert(&projector.sha256)
                {
                    return None;
                }
                total.checked_add(projector.bytes)
            });
        if request.projectors.len() > 16
            || projector_bytes.is_none_or(|total| total >= request.expected_bytes)
            || (!request.projectors.is_empty() && request.expected_sha256.is_none())
        {
            return Err(BackendError("invalid expected projector integrity".into()));
        }
        if request.expected_bytes == 0
            || request.expected_bytes > 9007199254740991
            || request.expected_sha256.as_ref().is_some_and(|value| {
                value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(BackendError("invalid expected model integrity".into()));
        }
        if cancel.is_cancelled() {
            return Err(BackendError("cancelled".into()));
        }
        let mut result = PreparedModel {
            model_id: request.model_id.clone(),
            model_path: None,
            digest_verified: false,
            local_manifest_digest: None,
        };
        match request.backend.as_str() {
            "ollama" => {
                let context = OllamaCommandContext::new(endpoint, self.ollama_models)?;
                observe(
                    observer,
                    LifecycleScope::Runtime,
                    LifecycleStage::Acquisition,
                    async {
                        self.commands
                            .run_ollama(
                                binary,
                                &["pull".into(), "--".into(), request.model_id.clone()],
                                &context,
                                cancel,
                                Duration::from_secs(1800),
                            )
                            .await
                            .map_err(BackendError)
                    },
                )
                .await?;
                let digest = observe(
                    observer,
                    LifecycleScope::Runtime,
                    LifecycleStage::Verification,
                    async {
                        let digest = crate::ollama_installed::manifest_digests(
                            self.ollama_models,
                            &request.model_id,
                        )
                        .map_err(|error| BackendError(error.to_string()))?
                        .into_iter()
                        .next()
                        .ok_or_else(|| BackendError("manifest digest unavailable".into()))?;
                        verify_catalog_manifest(
                            self.ollama_models,
                            &request.model_id,
                            &digest,
                            request.expected_sha256.as_deref(),
                            Some(request.expected_bytes),
                            &request.projectors,
                            cancel,
                        )
                        .await
                        .map_err(|error| BackendError(error.to_string()))?;
                        Ok(digest)
                    },
                )
                .await?;
                result.digest_verified = request.expected_sha256.is_some();
                result.local_manifest_digest = Some(digest);
            }
            "llamacpp" | "bitnet" => {
                let source = request.gguf.as_ref().ok_or_else(|| {
                    BackendError(format!("{} requires a pinned GGUF source", request.backend))
                })?;
                let ceiling = request
                    .expected_bytes
                    .checked_add(request.expected_bytes.div_ceil(2))
                    .ok_or_else(|| BackendError("model size overflow".into()))?
                    .max(
                        request
                            .expected_bytes
                            .checked_add(64 * 1024 * 1024)
                            .ok_or_else(|| BackendError("model size overflow".into()))?,
                    );
                let artifact = Artifact {
                    backend: request.backend.clone(),
                    repo: source.repo.clone(),
                    revision: source.revision.clone(),
                    file: source.file.clone(),
                    sha256: source.sha256.clone(),
                    bytes: ceiling,
                };
                let acquired = observe(
                    observer,
                    LifecycleScope::Runtime,
                    LifecycleStage::AcquisitionVerification,
                    async {
                        self.acquisition
                            .acquire_sized(&artifact, SizePolicy::Ceiling, self.download, cancel)
                            .await
                            .map_err(BackendError)
                    },
                )
                .await?;
                result.model_path = Some(acquired.path);
                result.digest_verified = true;
            }
            "mlx" => {
                let source = request.mlx.as_ref().ok_or_else(|| {
                    BackendError("MLX requires pinned repository manifest".into())
                })?;
                let files = repository_artifacts(source, &request.backend, request.expected_bytes)?;
                result.model_path = Some(
                    observe(
                        observer,
                        LifecycleScope::Runtime,
                        LifecycleStage::AcquisitionVerification,
                        async {
                            self.acquisition
                                .repository(&files, self.download, cancel)
                                .await
                                .map_err(BackendError)
                        },
                    )
                    .await?,
                );
                result.digest_verified = true;
            }
            "lmstudio" => {
                let raw = self
                    .commands
                    .run(
                        binary,
                        &[
                            "ls".into(),
                            "--json".into(),
                            "--llm".into(),
                            "--quiet".into(),
                        ],
                        cancel,
                        Duration::from_secs(5),
                    )
                    .await?;
                let models = studio_models(&raw)?;
                let mut matches = models.into_iter().filter(|model| {
                    let path = model.path.replace('\\', "/");
                    if let Some(source) = &request.gguf {
                        let suffix = format!("{}/{}", source.repo, source.file);
                        path == suffix || path.ends_with(&format!("/{suffix}"))
                    } else if let Some(source) = &request.mlx {
                        path == source.repo || path.ends_with(&format!("/{}", source.repo))
                    } else {
                        model.model_key.as_deref() == Some(request.model_id.as_str())
                            || path == request.model_id
                    }
                });
                let selected = matches
                    .next()
                    .ok_or_else(|| BackendError("model is not downloaded in LM Studio".into()))?;
                if matches.next().is_some() {
                    return Err(BackendError("ambiguous LM Studio model selection".into()));
                }
                let local = studio_path(self.studio_models, &selected.path)?;
                observe(
                    observer,
                    LifecycleScope::Runtime,
                    LifecycleStage::Verification,
                    async {
                        if let Some(source) = &request.gguf {
                            let (digest, bytes) = hash_file(&local, cancel).await?;
                            if !digest.eq_ignore_ascii_case(&source.sha256)
                                || bytes < request.expected_bytes.div_ceil(2)
                            {
                                return Err(BackendError(
                                    "delegated GGUF integrity mismatch".into(),
                                ));
                            }
                            result.digest_verified = true;
                        } else if let Some(source) = &request.mlx {
                            let files =
                                repository_artifacts(source, "mlx", request.expected_bytes)?;
                            for artifact in files {
                                let path = studio_path(&local, &artifact.file)?;
                                let (digest, bytes) = hash_file(&path, cancel).await?;
                                if !digest.eq_ignore_ascii_case(&artifact.sha256)
                                    || bytes != artifact.bytes
                                {
                                    return Err(BackendError(
                                        "delegated MLX integrity mismatch".into(),
                                    ));
                                }
                            }
                            crate::special_adapters::validate_mlx_directory(&local)?;
                            result.digest_verified = true;
                        } else {
                            return Err(BackendError(
                                "delegated model needs catalog integrity evidence".into(),
                            ));
                        }
                        Ok(())
                    },
                )
                .await?;
                result.model_path = Some(PathBuf::from(selected.path));
            }
            _ => return Err(BackendError("unknown backend".into())),
        }
        Ok(result)
    }
}
fn repository_artifacts(
    source: &MlxSource,
    backend: &str,
    expected: u64,
) -> Result<Vec<Artifact>, BackendError> {
    let mut total = 0u64;
    let mut artifacts = Vec::new();
    for file in &source.files {
        if !file.bytes.is_finite()
            || file.bytes.fract() != 0.0
            || file.bytes <= 0.0
            || file.bytes > 9007199254740991.0
        {
            return Err(BackendError("invalid repository file size".into()));
        }
        total = total
            .checked_add(file.bytes as u64)
            .ok_or_else(|| BackendError("repository size overflow".into()))?;
        let artifact = Artifact {
            backend: backend.into(),
            repo: source.repo.clone(),
            revision: source.revision.clone(),
            file: file.file.clone(),
            sha256: file.sha256.clone(),
            bytes: file.bytes as u64,
        };
        artifact.validate()?;
        artifacts.push(artifact);
    }
    if total != expected {
        return Err(BackendError("repository/catalog size mismatch".into()));
    }
    Ok(artifacts)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StudioModel {
    path: String,
    model_key: Option<String>,
}
fn studio_models(raw: &str) -> Result<Vec<StudioModel>, BackendError> {
    #[derive(Deserialize)]
    struct Group {
        model: serde_json::Value,
        variants: Vec<StudioModel>,
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Model(StudioModel),
        Group(Group),
    }
    let entries: Vec<Entry> = serde_json::from_str(raw)
        .map_err(|_| BackendError("invalid LM Studio inventory".into()))?;
    if entries.len() > 10000 {
        return Err(BackendError("LM Studio inventory exceeds limit".into()));
    }
    let mut models = Vec::new();
    for entry in entries {
        match entry {
            Entry::Model(model) => models.push(model),
            Entry::Group(group) => {
                let key = group
                    .model
                    .get("modelKey")
                    .and_then(serde_json::Value::as_str);
                for mut model in group.variants {
                    if model.model_key.is_none() {
                        model.model_key = key.map(str::to_owned);
                    }
                    models.push(model);
                }
            }
        }
    }
    if models.len() > 10000
        || models.iter().any(|model| {
            model.path.is_empty()
                || model.path.len() > 4096
                || model.path.chars().any(char::is_control)
                || model.model_key.as_ref().is_some_and(|key| {
                    key.is_empty() || key.len() > 1024 || key.chars().any(char::is_control)
                })
        })
    {
        return Err(BackendError("invalid delegated model path".into()));
    }
    Ok(models)
}
fn studio_path(root: &Path, value: &str) -> Result<PathBuf, BackendError> {
    let normalized = value.replace('\\', "/");
    if normalized.split('/').any(|part| part == "..") {
        return Err(BackendError("delegated path traversal".into()));
    }
    let root = root
        .canonicalize()
        .map_err(|_| BackendError("delegated root unavailable".into()))?;
    let path = Path::new(&normalized);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    let path = path
        .canonicalize()
        .map_err(|_| BackendError("delegated artifact unavailable".into()))?;
    if !path.starts_with(&root) || path == root {
        return Err(BackendError("delegated artifact escaped root".into()));
    }
    Ok(path)
}
