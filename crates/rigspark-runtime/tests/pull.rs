use rigspark_runtime::{
    acquire::{Acquisition, DownloadResponse, DownloadTransport},
    application::events::{LifecycleObserver, LifecycleScope, LifecycleStage, LifecycleStatus},
    command::{CommandRunner, OllamaCommandContext},
    pull::{PullRequest, PullService},
};
use std::{path::Path, time::Duration};
use tokio_util::sync::CancellationToken;
struct Commands;
#[async_trait::async_trait]
impl CommandRunner for Commands {
    async fn run(
        &self,
        _binary: &Path,
        _args: &[String],
        _cancel: &CancellationToken,
        _timeout: Duration,
    ) -> Result<String, String> {
        panic!("unexpected command")
    }
}
struct Download;
#[tokio::test]
async fn unsafe_ollama_context_is_rejected_before_commands_or_downloads() {
    let root = tempfile::tempdir().unwrap();
    let acquisition = Acquisition::new(root.path().join("cache")).unwrap();
    let service = PullService {
        acquisition: &acquisition,
        download: &Download,
        commands: &Commands,
        ollama_models: root.path(),
        studio_models: root.path(),
    };
    let request = PullRequest {
        backend: "ollama".into(),
        model_id: "test:latest".into(),
        expected_bytes: 8,
        expected_sha256: None,
        projectors: Vec::new(),
        gguf: None,
        mlx: None,
    };
    for endpoint in [
        "http://example.com:59125",
        "http://127.0.0.1:59125/?query=1",
        "http://127.0.0.1:59125/#fragment",
    ] {
        assert!(
            service
                .pull_at(
                    &request,
                    Path::new("/never/spawn"),
                    endpoint,
                    &CancellationToken::new()
                )
                .await
                .is_err()
        );
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        service
            .pull_at(
                &request,
                Path::new("/never/spawn"),
                "http://127.0.0.1:59125",
                &cancel
            )
            .await
            .is_err()
    );
}
struct PullCommands;
#[async_trait::async_trait]
impl CommandRunner for PullCommands {
    async fn run(
        &self,
        _binary: &Path,
        _args: &[String],
        _cancel: &CancellationToken,
        _timeout: Duration,
    ) -> Result<String, String> {
        panic!("Ollama pull must use explicit endpoint and store")
    }
    async fn run_ollama(
        &self,
        _binary: &Path,
        args: &[String],
        context: &OllamaCommandContext,
        _cancel: &CancellationToken,
        _timeout: Duration,
    ) -> Result<String, String> {
        assert_eq!(args, ["pull", "--", "test:latest"]);
        assert_eq!(
            context.environment()["OLLAMA_HOST"],
            "http://127.0.0.1:59125/"
        );
        assert!(Path::new(&context.environment()["OLLAMA_MODELS"]).is_absolute());
        Ok(String::new())
    }
}
#[tokio::test]
async fn ollama_pull_uses_discrete_argv_and_verifies_manifest_blobs() {
    use serde_json::json;
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let blobs = root.path().join("blobs");
    let manifests = root
        .path()
        .join("manifests/registry.ollama.ai/library/test");
    std::fs::create_dir_all(&blobs).unwrap();
    std::fs::create_dir_all(&manifests).unwrap();
    let weight = b"verified";
    let config = b"{}";
    let sha = format!("{:x}", Sha256::digest(weight));
    let config_sha = format!("{:x}", Sha256::digest(config));
    std::fs::write(blobs.join(format!("sha256-{sha}")), weight).unwrap();
    std::fs::write(blobs.join(format!("sha256-{config_sha}")), config).unwrap();
    let manifest=json!({"schemaVersion":2,"config":{"digest":format!("sha256:{config_sha}"),"size":2,"mediaType":"config"},"layers":[{"digest":format!("sha256:{sha}"),"size":8,"mediaType":"application/vnd.ollama.image.model"}]}).to_string();
    std::fs::write(manifests.join("latest"), manifest).unwrap();
    let acquisition = Acquisition::new(root.path().join("cache")).unwrap();
    let service = PullService {
        acquisition: &acquisition,
        download: &Download,
        commands: &PullCommands,
        ollama_models: root.path(),
        studio_models: root.path(),
    };
    let request = PullRequest {
        backend: "ollama".into(),
        model_id: "test:latest".into(),
        expected_bytes: 8,
        expected_sha256: Some(sha.clone()),
        projectors: Vec::new(),
        gguf: None,
        mlx: None,
    };
    let (observer, mut events) = LifecycleObserver::channel();
    let result = service
        .pull_at_observed(
            &request,
            Path::new("/fake/ollama"),
            "http://127.0.0.1:59125",
            &CancellationToken::new(),
            Some(&observer),
        )
        .await
        .unwrap();
    assert!(result.digest_verified);
    assert!(result.local_manifest_digest.is_some());
    for stage in [LifecycleStage::Acquisition, LifecycleStage::Verification] {
        for status in [LifecycleStatus::Started, LifecycleStatus::Completed] {
            let event = events.try_recv().unwrap();
            assert_eq!(event.scope, LifecycleScope::Runtime);
            assert_eq!((event.stage, event.status), (stage, status));
        }
    }
    assert!(events.try_recv().is_err());
    std::fs::write(blobs.join(format!("sha256-{sha}")), b"corrupt!").unwrap();
    assert!(
        service
            .pull_at_observed(
                &request,
                Path::new("/fake/ollama"),
                "http://127.0.0.1:59125",
                &CancellationToken::new(),
                Some(&observer),
            )
            .await
            .is_err()
    );
    for (stage, status) in [
        (LifecycleStage::Acquisition, LifecycleStatus::Started),
        (LifecycleStage::Acquisition, LifecycleStatus::Completed),
        (LifecycleStage::Verification, LifecycleStatus::Started),
        (LifecycleStage::Verification, LifecycleStatus::Failed),
    ] {
        let event = events.try_recv().unwrap();
        assert_eq!((event.stage, event.status), (stage, status));
    }
    assert!(events.try_recv().is_err());
    std::fs::write(blobs.join(format!("sha256-{sha}")), weight).unwrap();
    let projector_sha = format!("{:x}", Sha256::digest(b"projector"));
    std::fs::write(blobs.join(format!("sha256-{projector_sha}")), b"projector").unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(manifests.join("latest")).unwrap()).unwrap();
    manifest["layers"].as_array_mut().unwrap().push(json!({"digest":format!("sha256:{projector_sha}"),"size":9,"mediaType":"application/vnd.ollama.image.projector"}));
    std::fs::write(manifests.join("latest"), manifest.to_string()).unwrap();
    let mut request = request;
    request.expected_bytes = 17;
    request
        .projectors
        .push(rigspark_core::sizing::ProjectorArtifact {
            bytes: 9,
            sha256: projector_sha,
        });
    assert!(
        service
            .pull_at(
                &request,
                Path::new("/fake/ollama"),
                "http://127.0.0.1:59125",
                &CancellationToken::new()
            )
            .await
            .is_ok()
    );
    request.projectors[0].sha256 = "b".repeat(64);
    assert!(
        service
            .pull_at(
                &request,
                Path::new("/fake/ollama"),
                "http://127.0.0.1:59125",
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn invalid_projector_request_is_rejected_before_commands() {
    struct NoCommands;
    #[async_trait::async_trait]
    impl CommandRunner for NoCommands {
        async fn run(
            &self,
            _: &Path,
            _: &[String],
            _: &CancellationToken,
            _: Duration,
        ) -> Result<String, String> {
            panic!("unexpected command")
        }
        async fn run_ollama(
            &self,
            _: &Path,
            _: &[String],
            _: &OllamaCommandContext,
            _: &CancellationToken,
            _: Duration,
        ) -> Result<String, String> {
            panic!("unexpected Ollama command")
        }
    }
    let root = tempfile::tempdir().unwrap();
    let acquisition = Acquisition::new(root.path()).unwrap();
    let service = PullService {
        acquisition: &acquisition,
        download: &Download,
        commands: &NoCommands,
        ollama_models: root.path(),
        studio_models: root.path(),
    };
    let request = PullRequest {
        backend: "ollama".into(),
        model_id: "test:latest".into(),
        expected_bytes: 100,
        expected_sha256: Some("a".repeat(64)),
        projectors: vec![rigspark_core::sizing::ProjectorArtifact {
            bytes: 0,
            sha256: "bad".into(),
        }],
        gguf: None,
        mlx: None,
    };
    assert!(
        service
            .pull_at(
                &request,
                Path::new("/fake/ollama"),
                "http://127.0.0.1:59125",
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn runtime_events_ollama_pull_failure_never_starts_verification() {
    struct FailedPull;
    #[async_trait::async_trait]
    impl CommandRunner for FailedPull {
        async fn run(
            &self,
            _binary: &Path,
            _args: &[String],
            _cancel: &CancellationToken,
            _timeout: Duration,
        ) -> Result<String, String> {
            panic!("unexpected command")
        }
        async fn run_ollama(
            &self,
            _binary: &Path,
            _args: &[String],
            _context: &OllamaCommandContext,
            _cancel: &CancellationToken,
            _timeout: Duration,
        ) -> Result<String, String> {
            Err("mock pull failed".into())
        }
    }
    let root = tempfile::tempdir().unwrap();
    let acquisition = Acquisition::new(root.path()).unwrap();
    let service = PullService {
        acquisition: &acquisition,
        download: &Download,
        commands: &FailedPull,
        ollama_models: root.path(),
        studio_models: root.path(),
    };
    let request = PullRequest {
        backend: "ollama".into(),
        model_id: "test:latest".into(),
        expected_bytes: 8,
        expected_sha256: None,
        gguf: None,
        projectors: Vec::new(),
        mlx: None,
    };
    let (observer, mut events) = LifecycleObserver::channel();
    let error = service
        .pull_at_observed(
            &request,
            Path::new("/fake/ollama"),
            "http://127.0.0.1:59125",
            &CancellationToken::new(),
            Some(&observer),
        )
        .await
        .unwrap_err();
    assert_eq!(error.0, "mock pull failed");
    for status in [LifecycleStatus::Started, LifecycleStatus::Failed] {
        let event = events.try_recv().unwrap();
        assert_eq!(
            (event.stage, event.status),
            (LifecycleStage::Acquisition, status)
        );
    }
    assert!(events.try_recv().is_err());
}
#[async_trait::async_trait]
impl DownloadTransport for Download {
    async fn get(&self, _url: &str) -> Result<DownloadResponse, String> {
        panic!("unexpected download")
    }
}
#[tokio::test]
async fn self_managed_pull_requires_pinned_source_before_side_effects() {
    let root = tempfile::tempdir().unwrap();
    let acquisition = Acquisition::new(root.path()).unwrap();
    let service = PullService {
        acquisition: &acquisition,
        download: &Download,
        commands: &Commands,
        ollama_models: root.path(),
        studio_models: root.path(),
    };
    for backend in ["llamacpp", "mlx", "bitnet"] {
        let request = PullRequest {
            backend: backend.into(),
            model_id: "test:latest".into(),
            expected_bytes: 100,
            expected_sha256: None,
            projectors: Vec::new(),
            gguf: None,
            mlx: None,
        };
        assert!(
            service
                .pull(
                    &request,
                    Path::new("/fake/runtime"),
                    &CancellationToken::new()
                )
                .await
                .is_err()
        );
    }
}

struct Bytes;
#[async_trait::async_trait]
impl DownloadTransport for Bytes {
    async fn get(&self, _url: &str) -> Result<DownloadResponse, String> {
        Ok(DownloadResponse {
            status: 200,
            commit: Some("a".repeat(40)),
            length: Some(2),
            body: Box::pin(std::io::Cursor::new(b"{}".to_vec())),
        })
    }
}
#[tokio::test]
async fn self_managed_preparation_verifies_gguf_and_complete_mlx_snapshot() {
    use rigspark_core::catalog::{GgufSource, MlxFile, MlxSource};
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let acquisition = Acquisition::new(root.path().join("cache")).unwrap();
    let digest = format!("{:x}", Sha256::digest(b"{}"));
    let service = PullService {
        acquisition: &acquisition,
        download: &Bytes,
        commands: &Commands,
        ollama_models: root.path(),
        studio_models: root.path(),
    };
    let mut request = PullRequest {
        backend: "llamacpp".into(),
        model_id: "test:latest".into(),
        expected_bytes: 100,
        expected_sha256: None,
        projectors: Vec::new(),
        gguf: Some(GgufSource {
            repo: "owner/model".into(),
            revision: "a".repeat(40),
            file: "weights.gguf".into(),
            sha256: digest.clone(),
        }),
        mlx: None,
    };
    let prepared = service
        .pull(
            &request,
            Path::new("/fake/runtime"),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(prepared.digest_verified);
    assert_eq!(std::fs::read(prepared.model_path.unwrap()).unwrap(), b"{}");
    request.backend = "bitnet".into();
    let prepared = service
        .pull(
            &request,
            Path::new("/fake/bitnet"),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(prepared.digest_verified);
    let path = prepared.model_path.unwrap();
    assert!(path.starts_with(root.path().canonicalize().unwrap().join("cache/bitnet")));
    assert_eq!(std::fs::read(&path).unwrap(), b"{}");
    request.gguf.as_mut().unwrap().sha256 = "b".repeat(64);
    assert!(
        service
            .pull(
                &request,
                Path::new("/fake/bitnet"),
                &CancellationToken::new()
            )
            .await
            .is_err()
    );
    request.backend = "mlx".into();
    request.expected_bytes = 6;
    request.gguf = None;
    request.mlx = Some(MlxSource {
        repo: "owner/model".into(),
        revision: "a".repeat(40),
        files: [
            "config.json",
            "tokenizer_config.json",
            "weights.safetensors",
        ]
        .into_iter()
        .map(|file| MlxFile {
            file: file.into(),
            sha256: digest.clone(),
            bytes: 2.0,
        })
        .collect(),
    });
    let prepared = service
        .pull(
            &request,
            Path::new("/fake/runtime"),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(prepared.digest_verified);
    assert_eq!(
        std::fs::read_dir(prepared.model_path.unwrap())
            .unwrap()
            .count(),
        3
    );
}
struct StudioCommands;
#[async_trait::async_trait]
impl CommandRunner for StudioCommands {
    async fn run(
        &self,
        _binary: &Path,
        args: &[String],
        _cancel: &CancellationToken,
        _timeout: Duration,
    ) -> Result<String, String> {
        assert_eq!(args, ["ls", "--json", "--llm", "--quiet"]);
        Ok(r#"[{"model":{"modelKey":"test:latest"},"variants":[{"path":"owner/model/weights.gguf"}]}]"#.into())
    }
}
#[tokio::test]
async fn delegated_pull_checks_exact_local_digest_without_download() {
    use rigspark_core::catalog::GgufSource;
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("owner/model")).unwrap();
    std::fs::write(root.path().join("owner/model/weights.gguf"), b"verified").unwrap();
    let acquisition = Acquisition::new(root.path().join("cache")).unwrap();
    let service = PullService {
        acquisition: &acquisition,
        download: &Download,
        commands: &StudioCommands,
        ollama_models: root.path(),
        studio_models: root.path(),
    };
    let request = PullRequest {
        backend: "lmstudio".into(),
        model_id: "test:latest".into(),
        expected_bytes: 8,
        expected_sha256: None,
        projectors: Vec::new(),
        gguf: Some(GgufSource {
            repo: "owner/model".into(),
            revision: "a".repeat(40),
            file: "weights.gguf".into(),
            sha256: format!("{:x}", Sha256::digest(b"verified")),
        }),
        mlx: None,
    };
    let result = service
        .pull(&request, Path::new("/fake/lms"), &CancellationToken::new())
        .await
        .unwrap();
    assert!(result.digest_verified);
    std::fs::write(root.path().join("owner/model/weights.gguf"), b"corrupt!").unwrap();
    assert!(
        service
            .pull(&request, Path::new("/fake/lms"), &CancellationToken::new())
            .await
            .is_err()
    );
}
