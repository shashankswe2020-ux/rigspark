use rigspark_runtime::ollama_installed::{inspect_metadata, parse_inventory, verify_manifest};
use serde_json::json;
use sha2::{Digest, Sha256};

#[test]
fn exact_local_inventory_and_honest_context_geometry() {
    let inventory=parse_inventory(json!({"models":[{"name":"local:tag","digest":"a".repeat(64),"size":100,"details":{"quantization_level":"Q4_K_M"}},{"name":"cloud:tag","digest":"b".repeat(64),"size":10,"remote_host":"https://example.com"}]})).unwrap();
    assert_eq!(inventory.len(), 1);
    let metadata=inspect_metadata(inventory[0].clone(),json!({"model_info":{"general.architecture":"llama","llama.context_length":8192,"llama.block_count":32,"llama.attention.head_count_kv":8,"llama.attention.key_length":128,"llama.attention.value_length":128},"capabilities":["completion"]})).unwrap();
    assert_eq!(metadata.context_length, Some(8192));
    assert_eq!(metadata.kv_bytes_per_token, Some(131072));
    assert_eq!(
        inspect_metadata(
            inventory[0].clone(),
            json!({"model_info":{"general.architecture":"unknown"}})
        )
        .unwrap()
        .kv_bytes_per_token,
        None
    );
    assert!(
        parse_inventory(json!({"models":[{"name":"../escape","digest":"a".repeat(64),"size":1}]}))
            .is_err()
    );
    assert!(parse_inventory(json!({"models":[{"name":"x","digest":"bad","size":1}]})).is_err());
}

#[test]
fn installed_sizing_keeps_unknown_geometry_and_enforces_context_caps() {
    let hardware:rigspark_core::sizing::Hardware=serde_json::from_value(json!({"arch":"arm64","platform":"darwin","totalRamBytes":17179869184u64,"freeRamBytes":12884901888u64,"freeDiskBytes":100000000000u64,"gpu":[{"vendor":"apple","vramBytes":0}]})).unwrap();
    let mut model = parse_inventory(
        json!({"models":[{"name":"test:latest","digest":"a".repeat(64),"size":1000000000u64}]}),
    )
    .unwrap()
    .remove(0);
    let sized =
        rigspark_runtime::ollama_installed::size_installed(&model, &hardware, Some(8192)).unwrap();
    assert_eq!(sized["fit"], "unknown");
    assert_eq!(sized["throughput"], "unknown");
    model.context_length = Some(4096);
    model.kv_bytes_per_token = Some(1024);
    assert_eq!(
        rigspark_runtime::ollama_installed::size_installed(&model, &hardware, Some(8192)).unwrap()
            ["fit"],
        "no"
    );
    assert_eq!(
        rigspark_runtime::ollama_installed::size_installed(&model, &hardware, Some(4096)).unwrap()
            ["fit"],
        "yes"
    );
}

#[test]
fn discrete_gpu_sizing_reports_weights_only_fit_honestly_and_rejects_large_kv() {
    const GIB: u64 = 1 << 30;
    let hardware: rigspark_core::sizing::Hardware = serde_json::from_value(json!({"arch":"x64","platform":"linux","totalRamBytes":32*GIB,"freeRamBytes":24*GIB,"freeDiskBytes":0,"gpu":[{"vendor":"nvidia","vramBytes":8*GIB}]})).unwrap();
    let mut model = parse_inventory(
        json!({"models":[{"name":"gemma4:e4b-it-qat","digest":"a".repeat(64),"size":3*GIB}]}),
    )
    .unwrap()
    .remove(0);
    model.context_length = Some(131_072);
    let size = |model: &rigspark_runtime::ollama_installed::InstalledModel, context| {
        rigspark_runtime::ollama_installed::size_installed(model, &hardware, Some(context)).unwrap()
    };
    let unknown = size(&model, 65_536);
    assert_eq!(
        (
            &unknown["fit"],
            &unknown["weightsFit"],
            &unknown["requiredBytes"],
            &unknown["context"],
            &unknown["memoryKind"]
        ),
        (
            &json!("unknown"),
            &json!(true),
            &json!(null),
            &json!(65_536),
            &json!("vram")
        )
    );
    assert_eq!(size(&model, 262_144)["fit"], "no");
    model.kv_bytes_per_token = Some(16_384);
    assert_eq!(size(&model, 65_536)["fit"], "yes");
    model.kv_bytes_per_token = Some(262_144);
    assert_eq!(size(&model, 65_536)["fit"], "no");
}

#[tokio::test]
async fn verifies_every_local_blob_and_preserves_catalog_integrity() {
    let root = tempfile::tempdir().unwrap();
    let bytes = b"verified weights";
    let config = b"{}";
    let weight_digest = format!("{:x}", Sha256::digest(bytes));
    let config_digest = format!("{:x}", Sha256::digest(config));
    let manifest=json!({"schemaVersion":2,"config":{"digest":format!("sha256:{config_digest}"),"size":config.len(),"mediaType":"config"},"layers":[{"digest":format!("sha256:{weight_digest}"),"size":bytes.len(),"mediaType":"application/vnd.ollama.image.model"}]}).to_string();
    let digest = format!("{:x}", Sha256::digest(manifest.as_bytes()));
    let directory = root
        .path()
        .join("manifests/registry.ollama.ai/library/test");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::create_dir_all(root.path().join("blobs")).unwrap();
    std::fs::write(directory.join("latest"), manifest).unwrap();
    std::fs::write(
        root.path().join(format!("blobs/sha256-{weight_digest}")),
        bytes,
    )
    .unwrap();
    std::fs::write(
        root.path().join(format!("blobs/sha256-{config_digest}")),
        config,
    )
    .unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    verify_manifest(
        root.path(),
        "test:latest",
        &digest,
        Some(&weight_digest),
        Some(bytes.len() as u64),
        &cancel,
    )
    .await
    .unwrap();
    assert!(
        verify_manifest(
            root.path(),
            "test:latest",
            &digest,
            Some(&"a".repeat(64)),
            None,
            &cancel
        )
        .await
        .is_err()
    );
    assert!(
        verify_manifest(
            root.path(),
            "test:latest",
            &digest,
            None,
            Some(1000),
            &cancel
        )
        .await
        .is_err()
    );
    let actual = bytes.len() as u64;
    // Catalog sizes are estimates: larger pulls and pulls down to half the estimate are benign.
    for expected in [1, actual / 2, actual * 38 / 49 + 1, actual * 2] {
        verify_manifest(
            root.path(),
            "test:latest",
            &digest,
            None,
            Some(expected),
            &cancel,
        )
        .await
        .unwrap_or_else(|error| panic!("expected {expected}: {error}"));
    }
    assert!(
        verify_manifest(
            root.path(),
            "test:latest",
            &digest,
            None,
            Some(actual * 2 + 2),
            &cancel
        )
        .await
        .is_err()
    );
    std::fs::write(
        root.path().join(format!("blobs/sha256-{config_digest}")),
        b"!!",
    )
    .unwrap();
    assert!(
        verify_manifest(root.path(), "test:latest", &digest, None, None, &cancel)
            .await
            .is_err()
    );
}

struct QueueTransport {
    responses: std::sync::Mutex<std::collections::VecDeque<serde_json::Value>>,
    requests: std::sync::Mutex<Vec<(String, Option<serde_json::Value>)>>,
}

#[tokio::test]
async fn projector_pins_reject_substitution_and_missing_layers() {
    use rigspark_core::sizing::ProjectorArtifact;
    use rigspark_runtime::ollama_installed::verify_catalog_manifest;
    let root = tempfile::tempdir().unwrap();
    let directory = root
        .path()
        .join("manifests/registry.ollama.ai/library/test");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::create_dir(root.path().join("blobs")).unwrap();
    let make_layer = |bytes: &[u8], media_type: &str| {
        let sha = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(root.path().join(format!("blobs/sha256-{sha}")), bytes).unwrap();
        json!({"digest":format!("sha256:{sha}"), "size":bytes.len(), "mediaType":media_type})
    };
    let config = make_layer(b"{}", "config");
    let model = make_layer(b"model", "application/vnd.ollama.image.model");
    let projector = make_layer(b"projector", "application/vnd.ollama.image.projector");
    let pins = vec![ProjectorArtifact {
        bytes: 9,
        sha256: format!("{:x}", Sha256::digest(b"projector")),
    }];
    let cancel = tokio_util::sync::CancellationToken::new();
    for (layers, success) in [
        (json!([model.clone(), projector]), true),
        (json!([model.clone()]), false),
        (
            json!([
                model,
                make_layer(b"substitute", "application/vnd.ollama.image.projector")
            ]),
            false,
        ),
    ] {
        let raw = json!({"schemaVersion":2,"config":config,"layers":layers}).to_string();
        std::fs::write(directory.join("latest"), &raw).unwrap();
        let digest = format!("{:x}", Sha256::digest(raw.as_bytes()));
        assert_eq!(
            verify_catalog_manifest(
                root.path(),
                "test:latest",
                &digest,
                None,
                Some(14),
                &pins,
                &cancel
            )
            .await
            .is_ok(),
            success
        );
    }
}
#[async_trait::async_trait]
impl rigspark_runtime::http::Transport for QueueTransport {
    async fn send(
        &self,
        request: rigspark_runtime::http::Request,
    ) -> Result<rigspark_runtime::http::Response, rigspark_runtime::http::HttpError> {
        self.requests
            .lock()
            .unwrap()
            .push((request.url.path().into(), request.body().cloned()));
        let value = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected HTTP request");
        Ok(rigspark_runtime::http::Response {
            status: 200,
            body: Box::pin(std::io::Cursor::new(serde_json::to_vec(&value).unwrap())),
        })
    }
}
struct Verifier(std::sync::Mutex<Vec<String>>);
#[async_trait::async_trait]
impl rigspark_runtime::ollama_installed::InstalledVerifier for Verifier {
    async fn verify(
        &self,
        id: &str,
        _digest: &str,
        _cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<(), rigspark_runtime::ollama_installed::OllamaError> {
        self.0.lock().unwrap().push(id.into());
        Ok(())
    }
}
#[tokio::test]
async fn context_activation_verifies_variant_and_detects_source_drift() {
    use rigspark_runtime::ollama_installed::InstalledModels;
    let tag = |id: &str, digest: &str| json!({"name":id,"digest":digest,"size":100});
    let original = tag("test:latest", &"a".repeat(64));
    let variant = "llmup-context-fixed:8192";
    let model = parse_inventory(json!({"models":[original.clone()]}))
        .unwrap()
        .remove(0);
    for drift in [false, true] {
        let transport=QueueTransport { responses:std::sync::Mutex::new([
            json!({"models":[original.clone()]}),json!({"status":"success"}),json!({"parameters":"num_ctx 8192\n"}),json!({"models":[original.clone(),tag(variant,&"b".repeat(64))]}),json!({"models":[if drift {tag("test:latest",&"c".repeat(64))} else {original.clone()}]})
        ].into()),requests:std::sync::Mutex::new(Vec::new()) };
        let verifier = Verifier(std::sync::Mutex::new(Vec::new()));
        let service = InstalledModels::new(&transport, &verifier);
        let result = service
            .activate_with_id(
                "http://127.0.0.1:11435",
                &model,
                Some(8192),
                "fixed",
                &tokio_util::sync::CancellationToken::new(),
            )
            .await;
        if drift {
            assert!(result.is_err());
        } else {
            assert_eq!(result.unwrap(), variant);
        }
        assert_eq!(*verifier.0.lock().unwrap(), vec!["test:latest", variant]);
        let requests = transport.requests.lock().unwrap();
        assert_eq!(
            requests[1].1.as_ref().unwrap(),
            &json!({"model":variant,"from":"test:latest","parameters":{"num_ctx":8192},"stream":false})
        );
    }
}

#[test]
fn ollama_identity_accepts_the_file_digest_or_a_verified_embedded_manifest() {
    use rigspark_runtime::ollama_installed::manifest_digests;
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let hex = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let upstream = br#"{"schemaVersion":2,"layers":[]}"#;
    let upstream_digest = hex(upstream);
    std::fs::create_dir_all(root.path().join("blobs")).unwrap();
    std::fs::write(
        root.path()
            .join("blobs")
            .join(format!("sha256-{upstream_digest}")),
        upstream,
    )
    .unwrap();
    let manifest = serde_json::json!({
        "schemaVersion": 2,
        "config": {"mediaType": "application/vnd.docker.container.image.v1+json", "digest": format!("sha256:{}", "c".repeat(64)), "size": 1},
        "layers": [
            {"mediaType": "application/vnd.ollama.image.model", "digest": format!("sha256:{}", "a".repeat(64)), "size": 10},
            {"mediaType": "application/vnd.docker.distribution.manifest.v2+json", "digest": format!("sha256:{upstream_digest}"), "size": upstream.len()},
            {"mediaType": "application/vnd.docker.distribution.manifest.v2+json", "digest": format!("sha256:{}", "d".repeat(64)), "size": 5}
        ]
    })
    .to_string();
    let path = root.path().join("manifests/hf.co/owner/repo/q4_k_m");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &manifest).unwrap();
    let digests = manifest_digests(root.path(), "hf.co/owner/repo:q4_k_m").unwrap();
    assert_eq!(
        digests[0],
        hex(manifest.as_bytes()),
        "the file digest comes first"
    );
    assert!(
        digests.contains(&upstream_digest),
        "Ollama 0.40+ reports the embedded manifest"
    );
    assert!(
        !digests.contains(&"d".repeat(64)),
        "an embedded digest without a matching blob is never trusted"
    );
    assert_eq!(digests.len(), 2);
    std::fs::write(
        root.path()
            .join("blobs")
            .join(format!("sha256-{upstream_digest}")),
        b"tampered",
    )
    .unwrap();
    assert_eq!(
        manifest_digests(root.path(), "hf.co/owner/repo:q4_k_m")
            .unwrap()
            .len(),
        1,
        "a tampered embedded manifest is not an identity"
    );
}

#[cfg(unix)]
#[test]
fn ollama_v2_content_addressed_manifests_are_read_and_verified() {
    use rigspark_runtime::ollama_installed::manifest_digests;
    use sha2::{Digest, Sha256};
    let root = tempfile::tempdir().unwrap();
    let manifest = br#"{"schemaVersion":2,"config":{"mediaType":"x","digest":"sha256:aa","size":1},"layers":[]}"#;
    let hex = format!("{:x}", Sha256::digest(manifest));
    std::fs::create_dir_all(root.path().join("blobs")).unwrap();
    std::fs::write(
        root.path().join("blobs").join(format!("sha256-{hex}")),
        manifest,
    )
    .unwrap();
    let link = |id_path: &str, target: &str| {
        let path = root.path().join("manifests-v2").join(id_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        std::os::unix::fs::symlink(target, &path).unwrap();
    };
    let target = format!("../../../../blobs/sha256-{hex}");
    link("ollama.com/library/llmup-context-x/8192", &target);
    link("hf.co/owner/repo/q4_k_m", &target);
    assert_eq!(
        manifest_digests(root.path(), "llmup-context-x:8192").unwrap(),
        std::slice::from_ref(&hex),
        "library models live under ollama.com in the v2 store"
    );
    assert_eq!(
        manifest_digests(root.path(), "hf.co/owner/repo:q4_k_m").unwrap(),
        std::slice::from_ref(&hex)
    );
    std::fs::write(
        root.path().join("blobs").join(format!("sha256-{hex}")),
        b"tampered",
    )
    .unwrap();
    assert!(
        manifest_digests(root.path(), "llmup-context-x:8192").is_err(),
        "content must match its address"
    );
    std::fs::write(
        root.path().join("blobs").join(format!("sha256-{hex}")),
        manifest,
    )
    .unwrap();
    link("hf.co/owner/repo/q4_k_m", "../../../../../outside");
    assert!(
        manifest_digests(root.path(), "hf.co/owner/repo:q4_k_m").is_err(),
        "links only point into blobs/"
    );
}
