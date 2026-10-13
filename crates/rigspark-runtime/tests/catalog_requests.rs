use rigspark_core::{
    GENERATION_JSON, MODELS_JSON,
    catalog::Catalog,
    catalog_request::Repository,
    generation::{FileRole, GenerationCatalog, GenerationKind},
};
use rigspark_runtime::{
    admission::RecordedAdmissionTransport,
    catalog_request::{RequestDisposition, evaluate_request},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

const NOW: &str = "2026-10-13T00:00:00Z";
const REPOSITORY: &str = "owner/Requested-1B-GGUF";
const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const FILE: &str = "requested-1b-q4_k_m.gguf";
const SHA256: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn generation() -> GenerationCatalog {
    GenerationCatalog::parse(GENERATION_JSON).unwrap()
}

#[derive(Default)]
struct Gguf {
    kv: Vec<(String, Value)>,
    tensors: Vec<(String, Vec<u64>)>,
}

impl Gguf {
    fn kv(mut self, key: &str, value: Value) -> Self {
        self.kv.push((key.into(), value));
        self
    }

    fn tensor(mut self, name: &str, dims: &[u64]) -> Self {
        self.tensors.push((name.into(), dims.to_vec()));
        self
    }

    fn bytes(&self) -> Vec<u8> {
        fn string(out: &mut Vec<u8>, text: &str) {
            out.extend((text.len() as u64).to_le_bytes());
            out.extend(text.as_bytes());
        }
        fn value(out: &mut Vec<u8>, value: &Value) {
            match value {
                Value::String(text) => {
                    out.extend(8u32.to_le_bytes());
                    string(out, text);
                }
                Value::Number(number) => {
                    out.extend(4u32.to_le_bytes());
                    out.extend((number.as_u64().unwrap() as u32).to_le_bytes());
                }
                _ => unreachable!(),
            }
        }
        let mut out = b"GGUF".to_vec();
        out.extend(3u32.to_le_bytes());
        out.extend((self.tensors.len() as u64).to_le_bytes());
        out.extend((self.kv.len() as u64).to_le_bytes());
        for (key, item) in &self.kv {
            string(&mut out, key);
            value(&mut out, item);
        }
        for (name, dims) in &self.tensors {
            string(&mut out, name);
            out.extend((dims.len() as u32).to_le_bytes());
            for dim in dims {
                out.extend(dim.to_le_bytes());
            }
            out.extend(12u32.to_le_bytes());
            out.extend(0u64.to_le_bytes());
        }
        out
    }
}

fn metadata(siblings: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "id": REPOSITORY,
        "sha": REVISION,
        "tags": [
            "gguf",
            "text-generation",
            "conversational",
            "base_model:owner/Requested-1B",
            "license:apache-2.0"
        ],
        "gated": false,
        "private": false,
        "siblings": siblings,
        "pipeline_tag": "text-generation"
    }))
    .unwrap()
}

fn transport(with_gguf: bool) -> RecordedAdmissionTransport {
    let siblings = if with_gguf {
        json!([{
            "rfilename": FILE,
            "lfs": {"sha256": SHA256, "size": 700_000_000}
        }])
    } else {
        json!([{"rfilename": "model.safetensors"}])
    };
    let mut responses = BTreeMap::from([(
        format!("https://huggingface.co/api/models/{REPOSITORY}?blobs=true"),
        (200, metadata(siblings)),
    )]);
    if with_gguf {
        let header = Gguf::default()
            .kv("general.architecture", json!("requested"))
            .kv("general.license", json!("apache-2.0"))
            .kv("requested.context_length", json!(32768))
            .tensor("output.weight", &[1_000_000_000])
            .bytes();
        responses.insert(
            format!("https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/{FILE}"),
            (206, header),
        );
    }
    RecordedAdmissionTransport::new(responses).unwrap()
}

#[tokio::test]
async fn pinned_q4_k_m_repository_produces_one_valid_catalog_entry() {
    let catalog = Catalog::parse(MODELS_JSON).unwrap();
    let repository = Repository::parse(&format!("https://huggingface.co/{REPOSITORY}")).unwrap();
    let outcome = evaluate_request(
        &catalog,
        &generation(),
        &repository,
        &transport(true),
        NOW,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.disposition, RequestDisposition::Eligible);
    assert_eq!(outcome.reason_code, "ELIGIBLE");
    let updated = outcome.catalog.unwrap();
    let entry = updated
        .models
        .iter()
        .find(|model| {
            model
                .source
                .gguf
                .as_ref()
                .is_some_and(|source| source.repo == REPOSITORY)
        })
        .unwrap();
    assert_eq!(entry.id, "owner/requested-1b:q4_k_m");
    assert_eq!(entry.family, "requested");
    assert_eq!(entry.params, "1B");
    assert_eq!(entry.context_length, 32768.0);
    assert_eq!(entry.capabilities, ["chat"]);
    assert_eq!(entry.source.hf.as_deref(), Some("owner/Requested-1B"));
    assert_eq!(entry.source.gguf.as_ref().unwrap().revision, REVISION);
    assert_eq!(entry.quantizations[0].name, "Q4_K_M");
    assert_eq!(entry.quantizations[0].sha256.as_deref(), Some(SHA256));
    assert!(outcome.observation.is_some());
}

#[tokio::test]
async fn repository_without_runnable_artifact_is_blocked_without_mutation() {
    let catalog = Catalog::parse(MODELS_JSON).unwrap();
    let repository = Repository::parse(&format!("https://huggingface.co/{REPOSITORY}")).unwrap();
    let outcome = evaluate_request(
        &catalog,
        &generation(),
        &repository,
        &transport(false),
        NOW,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        outcome.disposition,
        RequestDisposition::NeedsRunnableArtifact
    );
    assert_eq!(outcome.reason_code, "RUNNABLE_ARTIFACT_MISSING");
    assert!(outcome.catalog.is_none());
    assert!(outcome.observation.is_none());
}

fn ming_catalog() -> GenerationCatalog {
    let mut catalog = generation();
    catalog.models.retain(|model| model.family == "ming-image");
    assert!(!catalog.models.is_empty());
    catalog
}

#[tokio::test]
async fn shared_generation_encoders_do_not_establish_model_coverage() {
    let mut catalog = ming_catalog();
    let base = "inclusionAI/Ming-Image-0.1-Design";
    for model in &mut catalog.models {
        model.source = "https://huggingface.co/Other/Package".into();
        for file in &mut model.files {
            file.repo = if matches!(file.role, FileRole::Diffusion | FileRole::Checkpoint) {
                "Other/Package".into()
            } else {
                base.into()
            };
        }
    }
    let request = Repository::parse(&format!("https://huggingface.co/{base}")).unwrap();
    let outcome = evaluate_request(
        &Catalog::parse(MODELS_JSON).unwrap(),
        &catalog,
        &request,
        &ming_transport(base, false),
        NOW,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.disposition, RequestDisposition::NeedsMaintainer);
    assert!(outcome.model_ids.is_empty());
}

#[tokio::test]
async fn upstream_lookup_failure_is_not_a_success_shaped_availability_result() {
    let request =
        Repository::parse("https://huggingface.co/inclusionAI/Ming-Image-0.1-Design").unwrap();
    let error = evaluate_request(
        &Catalog::parse(MODELS_JSON).unwrap(),
        &ming_catalog(),
        &request,
        &RecordedAdmissionTransport::new(BTreeMap::new()).unwrap(),
        NOW,
        &CancellationToken::new(),
    )
    .await
    .err()
    .expect("unavailable metadata must return an error");
    assert!(error.to_string().contains("404"));
}

fn ming_transport(base: &str, change_digest: bool) -> RecordedAdmissionTransport {
    let recorded: Value = serde_json::from_str(include_str!(
        "../../rigspark-core/fixtures/admission/hf-models.json"
    ))
    .unwrap();
    let mut package = recorded["Comfy-Org/Ming-Image"].clone();
    if change_digest {
        for sibling in package["siblings"].as_array_mut().unwrap() {
            if sibling["lfs"].is_object() {
                sibling["lfs"]["sha256"] = json!("b".repeat(64));
            }
        }
    }
    let revision = package["sha"].as_str().unwrap();
    RecordedAdmissionTransport::parse(&json!({
        format!("https://huggingface.co/api/models/{base}?blobs=true"): {
            "status": 200,
            "json": {
                "id": base, "sha": REVISION, "private": false, "gated": false,
                "pipeline_tag": "text-to-image", "tags": ["license:mit"], "siblings": []
            }
        },
        format!("https://huggingface.co/api/models/Comfy-Org/Ming-Image/revision/{revision}?blobs=true"): {
            "status": 200, "json": package
        }
    }).to_string()).unwrap()
}

#[tokio::test]
async fn issue_317_matches_pinned_ming_lineage_but_keeps_fit_only_request_open() {
    let catalog = ming_catalog();
    let base = "inclusionAI/Ming-Image-0.1-Design";
    let request = Repository::parse(&format!("https://huggingface.co/{base}")).unwrap();
    let outcome = evaluate_request(
        &Catalog::parse(MODELS_JSON).unwrap(),
        &catalog,
        &request,
        &ming_transport(base, false),
        NOW,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.disposition, RequestDisposition::NeedsWorkflow);
    assert_eq!(outcome.reason_code, "GENERATION_WORKFLOW_MISSING");
    assert_eq!(outcome.model_ids.len(), catalog.models.len());
    assert!(outcome.summary.contains("fit-only"));
    assert!(
        outcome
            .evidence_urls
            .iter()
            .any(|url| url.contains("/revision/"))
    );
    assert!(outcome.changed_files.is_empty());
    assert!(outcome.catalog.is_none());
    assert!(outcome.observation.is_none());
}

#[tokio::test]
async fn image_names_or_lineage_without_matching_pinned_weights_are_not_duplicates() {
    for (base, change_digest) in [
        ("unrelated/Ming-Image-0.1-Design", false),
        ("inclusionAI/Ming-Image-0.1-Design", true),
    ] {
        let request = Repository::parse(&format!("https://huggingface.co/{base}")).unwrap();
        let outcome = evaluate_request(
            &Catalog::parse(MODELS_JSON).unwrap(),
            &ming_catalog(),
            &request,
            &ming_transport(base, change_digest),
            NOW,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.disposition, RequestDisposition::NeedsMaintainer);
        assert_eq!(outcome.reason_code, "GENERATION_ADMISSION_REVIEW_REQUIRED");
        assert!(outcome.model_ids.is_empty());
        assert!(outcome.changed_files.is_empty());
    }
}

#[tokio::test]
async fn direct_generation_requests_distinguish_runnable_and_fit_only_without_network() {
    let catalog = generation();
    let offline = RecordedAdmissionTransport::new(BTreeMap::new()).unwrap();
    for kind in [GenerationKind::Image, GenerationKind::Video] {
        let runnable = catalog
            .models
            .iter()
            .find(|model| model.kind == kind && model.runnable().is_ok())
            .unwrap();
        let request = Repository::parse(&runnable.source).unwrap();
        let outcome = evaluate_request(
            &Catalog::parse(MODELS_JSON).unwrap(),
            &catalog,
            &request,
            &offline,
            NOW,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.disposition, RequestDisposition::AlreadyAvailable);
        assert!(outcome.model_ids.contains(&runnable.id));
        assert!(outcome.changed_files.is_empty());
    }
    let request = Repository::parse("https://huggingface.co/Comfy-Org/Ming-Image").unwrap();
    let outcome = evaluate_request(
        &Catalog::parse(MODELS_JSON).unwrap(),
        &catalog,
        &request,
        &offline,
        NOW,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(outcome.disposition, RequestDisposition::NeedsWorkflow);
}
