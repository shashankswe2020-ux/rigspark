use rigspark_core::artificial_analysis::PublisherMapping;
use rigspark_runtime::{
    admission::{AdmissionTransport, Fetched, allowed_request},
    artificial_analysis::{PublisherResolutionError, resolve_publisher_artifact},
};
use serde_json::{Value, json};
use std::{io, sync::Mutex};
use tokio_util::sync::CancellationToken;
use url::Url;

fn mapping() -> PublisherMapping {
    PublisherMapping {
        creator_slug: "publisher".into(),
        release_slug: "example".into(),
        repo: "Publisher/Example".into(),
    }
}
fn metadata() -> Value {
    json!({
        "id":"Publisher/Example", "sha":"a".repeat(40), "private":false, "gated":false,
        "tags":["license:apache-2.0"],
        "siblings":[
            {"rfilename":"config.json"},
            {"rfilename":"model.safetensors","lfs":{"sha256":"b".repeat(64),"size":1000}},
            {"rfilename":"other-export.gguf","lfs":{"sha256":"c".repeat(64),"size":500}}
        ]
    })
}
struct Recorded {
    metadata: Value,
    config: Value,
    status: u16,
    config_status: u16,
    calls: Mutex<Vec<String>>,
}
impl Recorded {
    fn new() -> Self {
        Self {
            metadata: metadata(),
            config: json!({"model_type":"example", "max_position_embeddings":32768}),
            status: 200,
            config_status: 200,
            calls: Mutex::new(Vec::new()),
        }
    }
}
#[async_trait::async_trait]
impl AdmissionTransport for Recorded {
    async fn fetch(
        &self,
        url: &Url,
        accept: Option<&'static str>,
        range: Option<usize>,
        limit: usize,
    ) -> io::Result<Fetched> {
        assert!(allowed_request(url), "{}", url);
        assert!(accept.is_none() && range.is_none());
        self.calls.lock().unwrap().push(url.to_string());
        let value = if url.path().starts_with("/api/models/") {
            assert_eq!(limit, 4 * 1024 * 1024);
            &self.metadata
        } else {
            assert_eq!(limit, 64 * 1024);
            assert_eq!(
                url.path(),
                format!("/Publisher/Example/resolve/{}/config.json", "a".repeat(40))
            );
            &self.config
        };
        Ok(Fetched {
            status: if url.path().starts_with("/api/models/") {
                self.status
            } else {
                self.config_status
            },
            body: serde_json::to_vec(value).unwrap(),
        })
    }
}

#[tokio::test]
async fn publisher_resolution_pins_only_requested_weights_and_configuration() {
    let transport = Recorded::new();
    let resolved = resolve_publisher_artifact(
        &mapping(),
        &["model.safetensors".into()],
        &transport,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(resolved.source.files.len(), 1);
    assert_eq!(resolved.source.files[0].sha256, "b".repeat(64));
    assert_eq!(resolved.source.files[0].bytes, 1000.0);
    assert_eq!(resolved.source.revision, "a".repeat(40));
    assert_eq!(resolved.license, "apache-2.0");
    assert_eq!(resolved.model_type.as_deref(), Some("example"));
    assert_eq!(resolved.context_length, Some(32768));
    assert_eq!(transport.calls.lock().unwrap().len(), 2);
    assert!(resolved.config_url.unwrap().contains(&"a".repeat(40)));
}

#[tokio::test]
async fn publisher_resolution_rejects_identity_access_license_and_digest_failures() {
    let mutations = [
        ("id", json!("Impersonator/Example")),
        ("sha", json!("main")),
        ("private", json!(true)),
        ("gated", json!("auto")),
        ("gated", Value::Null),
        ("tags", json!([])),
        ("tags", json!(["license:closed"])),
        ("tags", json!(["license:mit", "license:apache-2.0"])),
        ("siblings", json!([{"rfilename":"model.safetensors"}])),
        (
            "siblings",
            json!([{"rfilename":"model.safetensors","lfs":{"sha256":"bad","size":1000}}]),
        ),
    ];
    for (field, value) in mutations {
        let mut transport = Recorded::new();
        transport.metadata[field] = value;
        assert!(
            resolve_publisher_artifact(
                &mapping(),
                &["model.safetensors".into()],
                &transport,
                &CancellationToken::new()
            )
            .await
            .is_err(),
            "{field}"
        );
        assert_eq!(transport.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn publisher_resolution_rejects_missing_duplicate_and_unsafe_file_selections() {
    for files in [
        vec![],
        vec!["missing.safetensors".into()],
        vec!["model.safetensors".into(), "model.safetensors".into()],
        vec!["../model.safetensors".into()],
        vec!["config.json".into()],
    ] {
        assert!(
            resolve_publisher_artifact(
                &mapping(),
                &files,
                &Recorded::new(),
                &CancellationToken::new()
            )
            .await
            .is_err()
        );
    }
    let mut transport = Recorded::new();
    transport.metadata["siblings"] = json!([
        metadata()["siblings"][1].clone(),
        metadata()["siblings"][1].clone()
    ]);
    assert!(
        resolve_publisher_artifact(
            &mapping(),
            &["model.safetensors".into()],
            &transport,
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn publisher_resolution_preserves_unknowns_and_rejects_invalid_config_facts() {
    let mut transport = Recorded::new();
    transport.config = json!({});
    let resolved = resolve_publisher_artifact(
        &mapping(),
        &["model.safetensors".into()],
        &transport,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(resolved.model_type.is_none() && resolved.context_length.is_none());
    for config in [
        json!({"model_type":"bad\nmodel"}),
        json!({"max_position_embeddings":0}),
        json!({"max_position_embeddings":-1}),
    ] {
        let mut transport = Recorded::new();
        transport.config = config;
        assert!(
            resolve_publisher_artifact(
                &mapping(),
                &["model.safetensors".into()],
                &transport,
                &CancellationToken::new()
            )
            .await
            .is_err()
        );
    }
}

#[tokio::test]
async fn publisher_resolution_cancels_before_fetch_and_surfaces_http_failures() {
    let transport = Recorded::new();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        resolve_publisher_artifact(
            &mapping(),
            &["model.safetensors".into()],
            &transport,
            &cancel
        )
        .await
        .is_err()
    );
    assert!(transport.calls.lock().unwrap().is_empty());
    let mut transport = Recorded::new();
    transport.status = 401;
    assert!(
        resolve_publisher_artifact(
            &mapping(),
            &["model.safetensors".into()],
            &transport,
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
}

#[test]
fn publisher_config_urls_require_an_exact_pinned_public_path() {
    assert!(allowed_request(
        &Url::parse(&format!(
            "https://huggingface.co/Publisher/Example/resolve/{}/config.json",
            "a".repeat(40)
        ))
        .unwrap()
    ));
    for url in [
        "http://huggingface.co/Publisher/Example/resolve/main/config.json",
        "https://huggingface.co/Publisher/Example/resolve/main/config.json",
        "https://untrusted.test/Publisher/Example/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/config.json",
        "https://huggingface.co/Publisher/Example/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/model.py",
        "https://user@huggingface.co/Publisher/Example/resolve/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/config.json",
    ] {
        assert!(!allowed_request(&Url::parse(url).unwrap()));
    }
}

#[tokio::test]
async fn publisher_resolution_checks_caps_even_when_an_injected_transport_does_not() {
    let mut transport = Recorded::new();
    transport.metadata["padding"] = json!("x".repeat(4 * 1024 * 1024));
    assert!(matches!(
        resolve_publisher_artifact(
            &mapping(),
            &["model.safetensors".into()],
            &transport,
            &CancellationToken::new()
        )
        .await,
        Err(PublisherResolutionError::Oversized(4_194_304))
    ));
    let mut transport = Recorded::new();
    transport.config["padding"] = json!("x".repeat(64 * 1024));
    assert!(matches!(
        resolve_publisher_artifact(
            &mapping(),
            &["model.safetensors".into()],
            &transport,
            &CancellationToken::new()
        )
        .await,
        Err(PublisherResolutionError::Oversized(65_536))
    ));
}

#[tokio::test]
async fn publisher_configuration_failures_are_not_converted_to_unknown_facts() {
    let mut transport = Recorded::new();
    transport.config_status = 404;
    assert!(matches!(
        resolve_publisher_artifact(
            &mapping(),
            &["model.safetensors".into()],
            &transport,
            &CancellationToken::new()
        )
        .await,
        Err(PublisherResolutionError::Http(404))
    ));
    transport.status = 206;
    assert!(matches!(
        resolve_publisher_artifact(
            &mapping(),
            &["model.safetensors".into()],
            &transport,
            &CancellationToken::new()
        )
        .await,
        Err(PublisherResolutionError::Http(206))
    ));
}

struct Pending(CancellationToken);
#[async_trait::async_trait]
impl AdmissionTransport for Pending {
    async fn fetch(
        &self,
        _: &Url,
        _: Option<&'static str>,
        _: Option<usize>,
        _: usize,
    ) -> io::Result<Fetched> {
        self.0.cancel();
        std::future::pending().await
    }
}

#[tokio::test]
async fn publisher_resolution_cancels_an_in_flight_request() {
    let cancel = CancellationToken::new();
    assert!(matches!(
        resolve_publisher_artifact(
            &mapping(),
            &["model.safetensors".into()],
            &Pending(cancel.clone()),
            &cancel
        )
        .await,
        Err(PublisherResolutionError::Cancelled)
    ));
}

#[tokio::test]
async fn publisher_resolution_rejects_unsafe_mapping_before_network_activity() {
    let mut mapping = mapping();
    mapping.repo = "Publisher/Example?redirect=bad".into();
    let transport = Recorded::new();
    assert!(
        resolve_publisher_artifact(
            &mapping,
            &["model.safetensors".into()],
            &transport,
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
    assert!(transport.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn publisher_weights_without_a_config_keep_unsourced_facts_unknown() {
    let mut transport = Recorded::new();
    transport.metadata["siblings"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    let resolved = resolve_publisher_artifact(
        &mapping(),
        &["other-export.gguf".into()],
        &transport,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(resolved.model_type.is_none() && resolved.context_length.is_none());
    assert!(resolved.config_url.is_none());
    assert_eq!(transport.calls.lock().unwrap().len(), 1);
}
