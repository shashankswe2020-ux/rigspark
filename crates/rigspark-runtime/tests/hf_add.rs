use rigspark_core::{catalog::EntryProvenance, catalog_request::Repository};
use rigspark_runtime::{
    admission::{RecordedAdmissionTransport, allowed_request},
    hf_add::{HfAddError, entry, inspect, parse_reference, search, search_url},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

const NOW: &str = "2026-10-30T00:00:00Z";
const REPOSITORY: &str = "owner/Tiny-1B-GGUF";
const REVISION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn header() -> Vec<u8> {
    fn string(out: &mut Vec<u8>, text: &str) {
        out.extend((text.len() as u64).to_le_bytes());
        out.extend(text.as_bytes());
    }
    let kv: [(&str, Value); 3] = [
        ("general.architecture", json!("tiny")),
        ("general.license", json!("apache-2.0")),
        ("tiny.context_length", json!(32768)),
    ];
    let mut out = b"GGUF".to_vec();
    out.extend(3u32.to_le_bytes());
    out.extend(1u64.to_le_bytes());
    out.extend((kv.len() as u64).to_le_bytes());
    for (key, value) in &kv {
        string(&mut out, key);
        match value {
            Value::String(text) => {
                out.extend(8u32.to_le_bytes());
                string(&mut out, text);
            }
            Value::Number(number) => {
                out.extend(4u32.to_le_bytes());
                out.extend((number.as_u64().unwrap() as u32).to_le_bytes());
            }
            _ => unreachable!(),
        }
    }
    string(&mut out, "output.weight");
    out.extend(1u32.to_le_bytes());
    out.extend(1_000_000_000u64.to_le_bytes());
    out.extend(12u32.to_le_bytes());
    out.extend(0u64.to_le_bytes());
    out
}

fn sha(fill: char) -> String {
    fill.to_string().repeat(64)
}

fn transport(license: &str) -> RecordedAdmissionTransport {
    let siblings = json!([
        {"rfilename": "Tiny-1B-Q4_K_M.gguf", "lfs": {"sha256": sha('b'), "size": 700_000_000}},
        {"rfilename": "Tiny-1B-Q8_0.gguf", "lfs": {"sha256": sha('c'), "size": 1_100_000_000}},
        {"rfilename": "Tiny-1B-IQ2_XS.gguf", "lfs": {"sha256": sha('d'), "size": 400_000_000}},
        {"rfilename": "mmproj-Tiny-F16.gguf", "lfs": {"sha256": sha('e'), "size": 90_000_000}},
        {"rfilename": "Tiny-1B-BF16-00001-of-00002.gguf", "lfs": {"sha256": sha('f'), "size": 1_000_000_000}},
        {"rfilename": "README.md"}
    ]);
    let metadata = json!({
        "id": REPOSITORY,
        "sha": REVISION,
        "tags": ["gguf", "text-generation", "base_model:owner/Tiny-1B", format!("license:{license}")],
        "gated": false,
        "private": false,
        "siblings": siblings,
        "pipeline_tag": "text-generation"
    });
    let mut responses = BTreeMap::from([(
        format!("https://huggingface.co/api/models/{REPOSITORY}?blobs=true"),
        (200, serde_json::to_vec(&metadata).unwrap()),
    )]);
    responses.insert(
        format!("https://huggingface.co/{REPOSITORY}/resolve/{REVISION}/Tiny-1B-IQ2_XS.gguf"),
        (206, header()),
    );
    responses.insert(
        search_url("tiny").unwrap().to_string(),
        (
            200,
            serde_json::to_vec(&json!([
                {"id": "owner/Tiny-1B-GGUF", "downloads": 1200, "likes": 3, "tags": ["gguf", "license:apache-2.0"]},
                {"id": "x/../escape", "downloads": 1},
                {"id": "other/Tiny-Chat-GGUF", "downloads": 50, "likes": 1, "tags": ["gguf"]}
            ]))
            .unwrap(),
        ),
    );
    RecordedAdmissionTransport::new(responses).unwrap()
}

fn repository() -> Repository {
    parse_reference(REPOSITORY).unwrap().0
}

#[test]
fn references_accept_urls_repo_ids_and_quant_suffixes() {
    for (raw, quant) in [
        ("owner/Tiny-1B-GGUF", None),
        ("https://huggingface.co/owner/Tiny-1B-GGUF", None),
        ("hf.co/owner/Tiny-1B-GGUF:Q8_0", Some("Q8_0")),
        ("owner/Tiny-1B-GGUF:q4_k_m", Some("Q4_K_M")),
    ] {
        let (repository, selected) = parse_reference(raw).unwrap();
        assert_eq!(repository.as_str(), REPOSITORY, "{raw}");
        assert_eq!(selected.as_deref(), quant, "{raw}");
    }
    for bad in [
        "",
        "owner",
        "owner/repo/extra",
        "https://example.com/owner/repo",
        "owner/re po",
        "../x/y",
        "owner/repo:Q4;rm",
    ] {
        assert!(parse_reference(bad).is_err(), "{bad:?}");
    }
}

#[tokio::test]
async fn inspection_lists_every_single_file_quant_with_pinned_digests() {
    let inspection = inspect(
        &repository(),
        &transport("apache-2.0"),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(inspection.revision, REVISION);
    assert_eq!(inspection.license, "apache-2.0");
    let quants: Vec<&str> = inspection
        .choices
        .iter()
        .map(|c| c.quant.as_str())
        .collect();
    assert_eq!(
        quants,
        ["IQ2_XS", "Q4_K_M", "Q8_0"],
        "projectors and split shards are skipped"
    );
    assert!(
        inspection
            .choices
            .iter()
            .all(|choice| choice.sha256.len() == 64)
    );
}

#[tokio::test]
async fn unrecognized_licenses_are_refused() {
    let error = inspect(
        &repository(),
        &transport("other"),
        &CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, HfAddError::License), "{error}");
}

#[tokio::test]
async fn a_chosen_quant_becomes_a_pinned_local_entry_for_both_runtimes() {
    let cancel = CancellationToken::new();
    let source = transport("apache-2.0");
    let inspection = inspect(&repository(), &source, &cancel).await.unwrap();
    let model = entry(&inspection, "Q8_0", &source, NOW, &cancel)
        .await
        .unwrap();
    assert_eq!(model.id, "hf.co/owner/tiny-1b-gguf:q8_0");
    assert_eq!(model.provenance, EntryProvenance::Local);
    assert_eq!(model.added_at.as_deref(), Some("2026-10-30"));
    assert_eq!(model.context_length, 32768.0);
    assert_eq!(model.quantizations[0].name, "Q8_0");
    assert_eq!(
        model.quantizations[0].sha256.as_deref(),
        Some(sha('c').as_str())
    );
    let gguf = model.source.gguf.as_ref().unwrap();
    assert_eq!(
        (gguf.revision.as_str(), gguf.file.as_str()),
        (REVISION, "Tiny-1B-Q8_0.gguf")
    );
    assert_eq!(
        model.source.ollama.as_deref(),
        Some("hf.co/owner/tiny-1b-gguf:q8_0")
    );
    assert!(model.supports_backend("llamacpp") && model.supports_backend("ollama"));
    assert!(
        entry(&inspection, "Q3_K_S", &source, NOW, &cancel)
            .await
            .is_err(),
        "only listed quants"
    );
}

#[tokio::test]
async fn search_lists_safe_gguf_repositories() {
    let hits = search("tiny", &transport("apache-2.0"), &CancellationToken::new())
        .await
        .unwrap();
    let ids: Vec<&str> = hits.iter().map(|hit| hit.id.as_str()).collect();
    assert_eq!(ids, ["owner/Tiny-1B-GGUF", "other/Tiny-Chat-GGUF"]);
    assert_eq!(hits[0].downloads, 1200);
    for bad in ["", "a&b=c", &"x".repeat(81), "line\nbreak"] {
        assert!(search_url(bad).is_err(), "{bad:?}");
    }
    let url = search_url("qwen 3 coder").unwrap();
    assert!(allowed_request(&url), "{url}");
    let mut tampered = url.clone();
    tampered.set_query(Some("search=x&filter=gguf&sort=downloads&limit=1000"));
    assert!(!allowed_request(&tampered));
}

#[tokio::test]
async fn one_header_sizes_every_quant_and_the_best_fit_is_the_largest_that_fits() {
    use rigspark_core::sizing::{CpuArch, Hardware, Platform};
    use rigspark_runtime::hf_add::{best_fit, entries};
    let cancel = CancellationToken::new();
    let source = transport("apache-2.0");
    let inspection = inspect(&repository(), &source, &cancel).await.unwrap();
    let all = entries(&inspection, &source, NOW, &cancel).await.unwrap();
    let quants: Vec<&str> = all
        .iter()
        .map(|model| model.quantizations[0].name.as_str())
        .collect();
    assert_eq!(quants, ["IQ2_XS", "Q4_K_M", "Q8_0"]);
    let machine = |gib: f64| Hardware {
        arch: CpuArch::Arm64,
        platform: Platform::Darwin,
        total_ram_bytes: gib * 1073741824.0,
        free_ram_bytes: gib * 1073741824.0,
        free_disk_bytes: 500e9,
        gpu: vec![],
        unified_memory: Some(true),
    };
    let pick = |gib| best_fit(&all, &machine(gib)).map(|model| model.quantizations[0].name.clone());
    assert_eq!(pick(64.0).as_deref(), Some("Q8_0"));
    assert_eq!(pick(0.25), None, "nothing fits: no silent pick");
    let mut with_bf16 = all.clone();
    let mut bf16 = all[2].clone();
    bf16.quantizations[0].name = "BF16".into();
    bf16.quantizations[0].disk_bytes *= 2.0;
    with_bf16.push(bf16);
    assert_eq!(
        best_fit(&with_bf16, &machine(64.0)).map(|model| model.quantizations[0].name.as_str()),
        Some("Q8_0"),
        "unquantized files are a last resort"
    );
    // Grow memory until Q4_K_M first fits: Q8_0 must not fit yet, so Q4_K_M is chosen.
    let first = (1..=256)
        .map(|step| f64::from(step) / 16.0)
        .find(|gib| rigspark_runtime::hf_add::fits(&all[1], &machine(*gib)))
        .unwrap();
    assert!(!rigspark_runtime::hf_add::fits(&all[2], &machine(first)));
    assert_eq!(pick(first).as_deref(), Some("Q4_K_M"));
}
