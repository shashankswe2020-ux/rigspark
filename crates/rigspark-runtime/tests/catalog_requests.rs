use rigspark_core::{MODELS_JSON, catalog::Catalog, catalog_request::Repository};
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
