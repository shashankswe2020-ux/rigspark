use rigspark_core::catalog::{Catalog, EntryProvenance};
use rigspark_runtime::{
    admission::{
        AdmissionOptions, AdmissionState, AdmissionTransport, Fetched, LIBRARY_URL,
        RecordedAdmissionTransport, admit, allowed_request,
    },
    catalog_quality::evaluate,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;
use url::Url;

const NOW: &str = "2026-10-07T03:17:00Z";
const MANIFEST: &str = "application/vnd.docker.distribution.manifest.v2+json";

fn digest(seed: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(seed.as_bytes()))
}

/// GGUF v3 bytes for a plain-attention model whose tensor table sums to `total` elements.
fn gguf(arch: &str, license: Option<&str>, total: u64) -> Vec<u8> {
    let mut out = b"GGUF".to_vec();
    out.extend(3u32.to_le_bytes());
    let mut kv: Vec<(String, Result<String, u32>)> = vec![
        ("general.architecture".into(), Ok(arch.into())),
        (format!("{arch}.block_count"), Err(32)),
        (format!("{arch}.context_length"), Err(32768)),
        (format!("{arch}.embedding_length"), Err(4096)),
        (format!("{arch}.attention.head_count"), Err(32)),
        (format!("{arch}.attention.head_count_kv"), Err(8)),
    ];
    if let Some(license) = license {
        kv.push(("general.license".into(), Ok(license.into())));
    }
    out.extend(1u64.to_le_bytes());
    out.extend((kv.len() as u64).to_le_bytes());
    let string = |out: &mut Vec<u8>, text: &str| {
        out.extend((text.len() as u64).to_le_bytes());
        out.extend(text.as_bytes());
    };
    for (key, value) in &kv {
        string(&mut out, key);
        match value {
            Ok(text) => {
                out.extend(8u32.to_le_bytes());
                string(&mut out, text);
            }
            Err(number) => {
                out.extend(4u32.to_le_bytes());
                out.extend(number.to_le_bytes());
            }
        }
    }
    string(&mut out, "token_embd.weight");
    out.extend(1u32.to_le_bytes());
    out.extend(total.to_le_bytes());
    out.extend(12u32.to_le_bytes());
    out.extend(0u64.to_le_bytes());
    out
}

struct Upstream {
    responses: BTreeMap<String, (u16, Vec<u8>)>,
    listing: Vec<(String, Vec<String>)>,
}
impl Upstream {
    fn new() -> Self {
        Self {
            responses: BTreeMap::new(),
            listing: Vec::new(),
        }
    }
    fn repo(&mut self, repo: &str, chips: &[&str], tags: &[&str]) -> &mut Self {
        self.listing.push((
            repo.into(),
            chips.iter().map(|chip| chip.to_string()).collect(),
        ));
        let links: String = tags
            .iter()
            .map(|tag| {
                format!(r#"<a href="/library/{repo}:{tag}" class="group-hover:underline"></a>"#)
            })
            .collect();
        self.responses.insert(
            format!("https://ollama.com/library/{repo}/tags"),
            (200, links.into_bytes()),
        );
        self
    }
    /// Publishes `repo:tag` with a GGUF model layer; returns the model digest.
    fn variant(
        &mut self,
        repo: &str,
        tag: &str,
        weights: &str,
        license_text: Option<&str>,
        gguf_license: Option<&str>,
    ) -> String {
        let model = digest(weights);
        let config = digest(&format!("{weights}-config"));
        let mut layers = vec![
            json!({"mediaType":"application/vnd.ollama.image.model","digest":format!("sha256:{model}"),"size":4_000_000_000_u64}),
        ];
        if let Some(text) = license_text {
            let license = digest(text);
            layers.push(json!({"mediaType":"application/vnd.ollama.image.license","digest":format!("sha256:{license}"),"size":text.len()}));
            self.responses.insert(
                format!("https://registry.ollama.ai/v2/library/{repo}/blobs/sha256:{license}"),
                (200, text.as_bytes().to_vec()),
            );
        }
        let manifest = json!({"schemaVersion":2,"mediaType":MANIFEST,"config":{"mediaType":"application/vnd.docker.container.image.v1+json","digest":format!("sha256:{config}"),"size":100},"layers":layers});
        self.responses.insert(
            format!("https://registry.ollama.ai/v2/library/{repo}/manifests/{tag}"),
            (200, manifest.to_string().into_bytes()),
        );
        self.responses.insert(
            format!("https://registry.ollama.ai/v2/library/{repo}/blobs/sha256:{config}"),
            (
                200,
                json!({"model_family":"granite","model_type":"8.0B","file_type":"Q4_K_M"})
                    .to_string()
                    .into_bytes(),
            ),
        );
        self.responses.insert(
            format!("https://registry.ollama.ai/v2/library/{repo}/blobs/sha256:{model}"),
            (200, gguf("granite", gguf_license, 8_000_000_000)),
        );
        model
    }
    fn transport(&self) -> Counting {
        let mut responses = self.responses.clone();
        let html: String = self
            .listing
            .iter()
            .map(|(repo, chips)| {
                let spans: String = chips
                    .iter()
                    .map(|chip| format!(r#"<span  class="inline-flex items-center rounded-md bg-indigo-50">{chip}</span>"#))
                    .collect();
                format!(r#"<li><a href="/library/{repo}" class="group w-full space-y-5"><div>{spans}</div></a></li>"#)
            })
            .collect();
        responses.insert(LIBRARY_URL.into(), (200, html.into_bytes()));
        Counting {
            inner: RecordedAdmissionTransport::new(responses).unwrap(),
            requests: Arc::default(),
        }
    }
}

struct Counting {
    inner: RecordedAdmissionTransport,
    requests: Arc<Mutex<Vec<String>>>,
}
#[async_trait::async_trait]
impl AdmissionTransport for Counting {
    async fn fetch(
        &self,
        url: &Url,
        accept: Option<&'static str>,
        range: Option<usize>,
        limit: usize,
    ) -> io::Result<Fetched> {
        self.requests.lock().unwrap().push(url.to_string());
        self.inner.fetch(url, accept, range, limit).await
    }
}

const APACHE: &str = "                                 Apache License\n                           Version 2.0, January 2004\n";

fn curated() -> Catalog {
    let mut value: Value =
        serde_json::from_str(include_str!("../../rigspark-core/data/models.json")).unwrap();
    let mistral = value["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == "mistral:7b")
        .unwrap()
        .clone();
    value["models"] = json!([mistral]);
    Catalog::parse(&value.to_string()).unwrap()
}

fn options(max_new: usize, state: AdmissionState) -> AdmissionOptions {
    AdmissionOptions {
        now: NOW.into(),
        max_new_variants: max_new,
        state,
    }
}

#[tokio::test]
async fn admits_sourced_variants_rejects_unsourced_and_never_touches_curated_entries() {
    let mut upstream = Upstream::new();
    let curated_digest = curated().models[0].quantizations[0].sha256.clone().unwrap();
    upstream
        .repo("granite9", &["tools"], &["8b", "8b-q8_0", "cloud"])
        .repo("closedmodel", &[], &["8b"])
        .repo("mistral", &[], &["7b", "7.0b"])
        .repo("cloudonly", &["cloud"], &["cloud"]);
    upstream.variant(
        "granite9",
        "8b",
        "granite9-8b",
        Some(APACHE),
        Some("apache-2.0"),
    );
    upstream.variant(
        "closedmodel",
        "8b",
        "closed-8b",
        Some("Proprietary terms"),
        None,
    );
    // Same weights as the curated mistral:7b under a different tag: curated wins.
    upstream.responses.insert(
        "https://registry.ollama.ai/v2/library/mistral/manifests/7.0b".into(),
        (200, json!({"config":{"mediaType":"x","digest":format!("sha256:{}", digest("c"))},"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":format!("sha256:{curated_digest}"),"size":4_372_811_712_u64}]}).to_string().into_bytes()),
    );
    let transport = upstream.transport();
    let catalog = curated();
    let outcome = admit(
        &catalog,
        &transport,
        &options(10, AdmissionState::default()),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(outcome.added, ["granite9:8b"]);
    assert_eq!(
        outcome.rejected.get("closedmodel:8b").map(String::as_str),
        Some("license not recognised as open")
    );
    assert!(outcome.complete, "{:?}", outcome.failures);
    let result = outcome.catalog.as_ref().unwrap();
    assert_eq!(result.models.len(), 2);
    let mistral = result
        .models
        .iter()
        .find(|model| model.id == "mistral:7b")
        .unwrap();
    assert_eq!(
        serde_json::to_value(mistral).unwrap(),
        serde_json::to_value(&catalog.models[0]).unwrap(),
        "curated entry is untouched"
    );
    let granite = result
        .models
        .iter()
        .find(|model| model.id == "granite9:8b")
        .unwrap();
    assert_eq!(granite.provenance, EntryProvenance::Auto);
    assert_eq!(granite.capabilities, ["chat", "tools"]);
    assert_eq!(granite.added_at.as_deref(), Some("2026-10-07"));
    assert_eq!(
        outcome.scope["variants"],
        json!(["granite9:8b", "mistral:7b"])
    );
    assert!(
        !transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|url| url.contains("/manifests/cloud")),
        "cloud tags are never fetched"
    );

    // Generated evidence verifies every auto entry in the real publication gate.
    let evidence =
        json!({"policyVersion":1,"scopes":[outcome.scope],"observations":outcome.observations});
    let report = evaluate(
        &serde_json::to_string(result).unwrap(),
        &evidence.to_string(),
        NOW,
    )
    .unwrap();
    let report = serde_json::to_value(report).unwrap();
    let granite_entry = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "granite9:8b")
        .unwrap();
    assert_eq!(granite_entry["verified"], true, "{granite_entry}");
    assert_eq!(report["coveragePercent"], 100.0);
}

#[tokio::test]
async fn budget_defers_new_variants_and_marks_the_run_incomplete() {
    let mut upstream = Upstream::new();
    upstream
        .repo("alpha", &[], &["8b"])
        .repo("beta", &[], &["8b"]);
    upstream.variant("alpha", "8b", "alpha", Some(APACHE), None);
    upstream.variant("beta", "8b", "beta", Some(APACHE), None);
    let outcome = admit(
        &curated(),
        &upstream.transport(),
        &options(1, AdmissionState::default()),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.added, ["alpha:8b"], "newest first");
    assert_eq!(outcome.deferred, ["beta:8b"]);
    assert!(!outcome.complete);
    assert_eq!(outcome.scope["complete"], false);
    assert!(
        outcome.scope["variants"]
            .as_array()
            .unwrap()
            .contains(&json!("beta:8b")),
        "deferred variants count as missing coverage"
    );
}

#[tokio::test]
async fn reruns_are_idempotent_cache_rejections_and_remove_vanished_auto_entries() {
    let mut upstream = Upstream::new();
    upstream
        .repo("granite9", &[], &["8b"])
        .repo("closedmodel", &[], &["8b"]);
    upstream.variant("granite9", "8b", "granite9-8b", Some(APACHE), None);
    upstream.variant(
        "closedmodel",
        "8b",
        "closed-8b",
        Some("Proprietary terms"),
        None,
    );
    let first = admit(
        &curated(),
        &upstream.transport(),
        &options(10, AdmissionState::default()),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let catalog = first.catalog.clone().unwrap();

    let transport = upstream.transport();
    let second = admit(
        &catalog,
        &transport,
        &options(10, first.state.clone()),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(second.added.is_empty() && second.updated.is_empty() && second.removed.is_empty());
    assert_eq!(second.reverified, ["granite9:8b"]);
    assert_eq!(
        second.catalog.as_ref().unwrap().generated_at,
        catalog.generated_at,
        "no change, no new stamp"
    );
    let requests = transport.requests.lock().unwrap().clone();
    let model_blobs = |seed: &str| {
        requests
            .iter()
            .filter(|url| url.ends_with(&digest(seed)))
            .count()
    };
    assert_eq!(
        model_blobs("granite9-8b"),
        0,
        "unchanged digests are not re-downloaded"
    );
    assert_eq!(
        model_blobs("closed-8b"),
        0,
        "cached rejections are not re-downloaded"
    );
    assert!(second.rejected.contains_key("closedmodel:8b"));

    let mut vanished = Upstream::new();
    vanished.repo("closedmodel", &[], &["8b"]);
    vanished.variant(
        "closedmodel",
        "8b",
        "closed-8b",
        Some("Proprietary terms"),
        None,
    );
    let third = admit(
        &catalog,
        &vanished.transport(),
        &options(10, first.state),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(third.removed, ["granite9:8b"]);
    assert!(
        third
            .catalog
            .unwrap()
            .models
            .iter()
            .all(|model| model.id != "granite9:8b")
    );
}

#[tokio::test]
async fn transient_failures_are_reported_not_cached_and_listing_failure_changes_nothing() {
    let mut upstream = Upstream::new();
    upstream.repo("granite9", &[], &["8b"]);
    let weights = upstream.variant("granite9", "8b", "granite9-8b", Some(APACHE), None);
    upstream.responses.insert(
        format!("https://registry.ollama.ai/v2/library/granite9/blobs/sha256:{weights}"),
        (503, Vec::new()),
    );
    let outcome = admit(
        &curated(),
        &upstream.transport(),
        &options(10, AdmissionState::default()),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(outcome.added.is_empty());
    assert!(!outcome.complete);
    let admission_failures: Vec<_> = outcome
        .failures
        .iter()
        .filter(|failure| !failure.contains("(curated)"))
        .collect();
    assert_eq!(admission_failures.len(), 1, "{:?}", outcome.failures);
    assert!(
        outcome.state.rejections.is_empty(),
        "a 503 must not blacklist eligible weights"
    );

    let transport = Counting {
        inner: RecordedAdmissionTransport::new(BTreeMap::new()).unwrap(),
        requests: Arc::default(),
    };
    assert!(
        admit(
            &curated(),
            &transport,
            &options(10, AdmissionState::default()),
            &CancellationToken::new()
        )
        .await
        .is_err()
    );

    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = admit(
        &curated(),
        &upstream.transport(),
        &options(10, AdmissionState::default()),
        &cancel,
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
}

#[test]
fn only_official_listing_tags_manifests_and_blobs_are_allow_listed() {
    let allowed = |raw: &str| allowed_request(&Url::parse(raw).unwrap());
    assert!(allowed(LIBRARY_URL));
    assert!(allowed("https://ollama.com/library/qwen3.8/tags"));
    assert!(allowed(
        "https://registry.ollama.ai/v2/library/qwen3.8/manifests/27b"
    ));
    assert!(allowed(&format!(
        "https://registry.ollama.ai/v2/library/qwen3.8/blobs/sha256:{}",
        digest("x")
    )));
    for refused in [
        "http://ollama.com/library?sort=newest",
        "https://ollama.com/library?sort=popular",
        "https://ollama.com/library/../admin/tags",
        "https://ollama.com:8443/library/qwen/tags",
        "https://registry.ollama.ai/v2/someone/model/manifests/latest",
        "https://registry.ollama.ai/v2/library/qwen/blobs/sha256:short",
        "https://evil.example/library?sort=newest",
        "https://user@registry.ollama.ai/v2/library/qwen/manifests/latest",
    ] {
        assert!(!allowed(refused), "{refused}");
    }
}

/// Publishes the curated mistral:7b artifact upstream with the given GGUF context length.
fn curated_upstream(context_ok: bool) -> Upstream {
    let curated = curated();
    let quant = &curated.models[0].quantizations[0];
    let weights = quant.sha256.clone().unwrap();
    let config = digest("mistral-config");
    let mut upstream = Upstream::new();
    upstream.repo("mistral", &["tools"], &["7b"]);
    let registry = "https://registry.ollama.ai/v2/library/mistral";
    upstream.responses.insert(
        format!("{registry}/manifests/7b"),
        (200, json!({"config":{"mediaType":"x","digest":format!("sha256:{config}")},"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":format!("sha256:{weights}"),"size":quant.disk_bytes as u64}]}).to_string().into_bytes()),
    );
    upstream.responses.insert(
        format!("{registry}/blobs/sha256:{config}"),
        (
            200,
            json!({"model_family":"llama","model_type":"7.2B","file_type":"Q4_K_M"})
                .to_string()
                .into_bytes(),
        ),
    );
    let mut header = gguf("llama", Some("apache-2.0"), 7_200_000_000);
    if !context_ok {
        // Same shape, smaller context: the catalog's 32768 is contradicted upstream.
        let needle = 32768u32.to_le_bytes();
        let at = header
            .windows(4)
            .position(|window| window == needle)
            .unwrap();
        header[at..at + 4].copy_from_slice(&4096u32.to_le_bytes());
    }
    upstream
        .responses
        .insert(format!("{registry}/blobs/sha256:{weights}"), (200, header));
    upstream
}

#[tokio::test]
async fn curated_entries_are_reobserved_partially_without_being_modified() {
    let catalog = curated();
    let transport = curated_upstream(true).transport();
    let outcome = admit(
        &catalog,
        &transport,
        &options(10, AdmissionState::default()),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.curated_observed, ["mistral:7b"]);
    assert_eq!(
        serde_json::to_value(outcome.catalog.as_ref().unwrap()).unwrap()["models"],
        serde_json::to_value(&catalog).unwrap()["models"],
        "curated entries are never modified"
    );
    let observed = &outcome.curated_observations[0];
    assert!(observed.get("model").is_none());
    assert_eq!(
        observed["sources"],
        json!([
            "https://registry.ollama.ai/v2/library/mistral/manifests/7b",
            "https://ollama.com/library/mistral:7b"
        ]),
        "cites only the Ollama reads"
    );
    let evidence = json!({"policyVersion":1,"scopes":[outcome.scope],"observations":outcome.curated_observations});
    let report = serde_json::to_value(
        evaluate(
            &serde_json::to_string(outcome.catalog.as_ref().unwrap()).unwrap(),
            &evidence.to_string(),
            NOW,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        report["entries"][0]["verified"], true,
        "{}",
        report["entries"][0]
    );
    assert_eq!(
        report["entries"][0]["checkedFields"],
        json!([
            "architecture",
            "contextLength",
            "defaultQuantization",
            "kvBytesPerToken",
            "license"
        ])
    );
    assert!(
        report["passed"].as_bool().unwrap(),
        "{}",
        report["blockers"]
    );

    // The GGUF facts are cached by digest: the second run reads only manifests and configs.
    let again = curated_upstream(true).transport();
    let second = admit(
        &catalog,
        &again,
        &options(10, outcome.state.clone()),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(second.curated_observed, ["mistral:7b"]);
    let weights = catalog.models[0].quantizations[0].sha256.clone().unwrap();
    assert!(
        !again
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|url| url.ends_with(&weights))
    );
}

#[tokio::test]
async fn a_curated_fact_contradicted_upstream_blocks_publication() {
    let catalog = curated();
    let outcome = admit(
        &catalog,
        &curated_upstream(false).transport(),
        &options(10, AdmissionState::default()),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let evidence = json!({"policyVersion":1,"scopes":[outcome.scope],"observations":outcome.curated_observations});
    let report = evaluate(
        &serde_json::to_string(outcome.catalog.as_ref().unwrap()).unwrap(),
        &evidence.to_string(),
        NOW,
    )
    .unwrap();
    assert!(!report.passed);
    assert!(
        report
            .blockers
            .iter()
            .any(|blocker| blocker.starts_with("mistral:7b") && blocker.contains("contradict"))
    );
}
