use rigspark_core::{catalog::EntryProvenance, generation::GenerationCatalog};
use rigspark_runtime::{
    admission::{AdmissionTransport, Fetched, GENERATION_LISTING_URL, RecordedAdmissionTransport},
    generation_admission::{GenerationAdmissionState, admit_generation},
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

fn models() -> Value {
    serde_json::from_str(include_str!(
        "../../rigspark-core/fixtures/admission/hf-models.json"
    ))
    .unwrap()
}

fn header(elements: u64) -> Vec<u8> {
    let json = serde_json::to_vec(
        &json!({"__metadata__":{},"w":{"dtype":"BF16","shape":[elements],"data_offsets":[0,0]}}),
    )
    .unwrap();
    let mut out = (json.len() as u64).to_le_bytes().to_vec();
    out.extend(json);
    out
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

fn upstream(metadata_ok: bool) -> Counting {
    let recorded = models();
    let mut responses: BTreeMap<String, (u16, Vec<u8>)> = BTreeMap::new();
    let repos = [
        "Comfy-Org/Ming-Image",
        "Comfy-Org/LongCat-Image",
        "Comfy-Org/MiniMax-Music-3",
    ];
    responses.insert(
        GENERATION_LISTING_URL.into(),
        (
            200,
            serde_json::to_vec(&repos.iter().map(|id| json!({"id": id})).collect::<Vec<_>>())
                .unwrap(),
        ),
    );
    for repo in repos {
        let model = &recorded[repo];
        let status = if metadata_ok || repo != "Comfy-Org/Ming-Image" {
            200
        } else {
            503
        };
        responses.insert(
            format!("https://huggingface.co/api/models/{repo}?blobs=true"),
            (status, model.to_string().into_bytes()),
        );
        for sibling in model["siblings"].as_array().unwrap() {
            let file = sibling["rfilename"].as_str().unwrap();
            if file.contains("diffusion_models/") {
                let sha = model["sha"].as_str().unwrap();
                responses.insert(
                    format!("https://huggingface.co/{repo}/resolve/{sha}/{file}"),
                    (206, header(6_150_000_000)),
                );
            }
        }
    }
    for base in [
        "inclusionAI/Ming-Image-0.1-Design",
        "meituan-longcat/LongCat-Image",
        "MiniMaxAI/MiniMax-Music3",
    ] {
        responses.insert(
            format!("https://huggingface.co/api/models/{base}"),
            (200, recorded[base].to_string().into_bytes()),
        );
    }
    Counting {
        inner: RecordedAdmissionTransport::new(responses).unwrap(),
        requests: Arc::default(),
    }
}

#[tokio::test]
async fn admits_fit_only_image_entries_skips_other_pipelines_and_keeps_curated() {
    let catalog = GenerationCatalog::bundled().unwrap();
    let transport = upstream(true);
    let outcome = admit_generation(
        &catalog,
        &transport,
        NOW,
        GenerationAdmissionState::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(outcome.complete, "{:?}", outcome.failures);
    assert_eq!(outcome.added.len(), 5, "{:?}", outcome.added);
    assert!(
        outcome
            .added
            .iter()
            .all(|id| id.starts_with("ming-image:") || id.starts_with("longcat-image:"))
    );
    assert!(
        !outcome.added.iter().any(|id| id.contains("edit")),
        "edit variants are not text-to-image"
    );
    assert!(
        !outcome.added.iter().any(|id| id.contains("minimax")),
        "text-to-audio is not generation"
    );
    let result = outcome.catalog.unwrap();
    assert_eq!(result.schema_version, 2);
    for curated in &catalog.models {
        assert_eq!(
            result.resolve(&curated.id).unwrap(),
            curated,
            "curated entries are untouched"
        );
    }
    let ming = result
        .resolve("ming-image:ming_image_0.1_design_bf16")
        .unwrap();
    assert_eq!(ming.provenance, EntryProvenance::Auto);
    assert!(ming.runnable().is_err());
    assert_eq!(ming.params, "6.2B");
    assert_eq!(
        outcome.state.params.len(),
        5,
        "header reads are cached by digest"
    );
}

#[tokio::test]
async fn reruns_reuse_cached_headers_and_unreadable_repos_keep_their_entries() {
    let catalog = GenerationCatalog::bundled().unwrap();
    let first = admit_generation(
        &catalog,
        &upstream(true),
        NOW,
        GenerationAdmissionState::default(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    let admitted = first.catalog.unwrap();

    let transport = upstream(true);
    let second = admit_generation(
        &admitted,
        &transport,
        "2026-10-14T03:17:00Z",
        first.state.clone(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(second.added.is_empty() && second.removed.is_empty());
    let unchanged = second.catalog.unwrap();
    assert_eq!(unchanged.generated_at, admitted.generated_at);
    assert_eq!(
        unchanged
            .resolve("ming-image:ming_image_0.1_design_bf16")
            .unwrap()
            .added_at
            .as_deref(),
        Some("2026-10-07")
    );
    assert!(
        !transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|url| url.contains("/resolve/"))
    );

    let outage = admit_generation(
        &admitted,
        &upstream(false),
        NOW,
        first.state,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!outage.complete);
    assert!(
        outage.removed.is_empty(),
        "a 503 must not delete entries: {:?}",
        outage.removed
    );
}

#[tokio::test]
async fn a_failed_listing_or_cancellation_changes_nothing() {
    let catalog = GenerationCatalog::bundled().unwrap();
    let empty = RecordedAdmissionTransport::new(BTreeMap::new()).unwrap();
    assert!(
        admit_generation(
            &catalog,
            &empty,
            NOW,
            GenerationAdmissionState::default(),
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = admit_generation(
        &catalog,
        &upstream(true),
        NOW,
        GenerationAdmissionState::default(),
        &cancel,
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error.kind(), io::ErrorKind::Interrupted);
}
