use rigspark_core::{
    admission::{
        AdmissionInput, GgufError, LicenseError, RegistryConfig, active_params, build_entry,
        kv_bytes_per_token, license_id, observation, parse_gguf, parse_library, parse_tags,
        select_variants,
    },
    catalog::{Catalog, EntryProvenance},
    registry_collector::parse_layer,
    sizing::Architecture,
};
use serde_json::{Value, json};

const LIBRARY: &str = include_str!("../fixtures/admission/library-newest.html");

fn registry() -> Value {
    serde_json::from_str(include_str!("../fixtures/admission/registry.json")).unwrap()
}

/// Minimal GGUF v3 writer: metadata pairs then tensor infos (name, dims), no tensor data.
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
    /// Pads the tensor table with one tensor so the total element count equals `total`.
    fn total(mut self, total: f64) -> Self {
        let current: u64 = self
            .tensors
            .iter()
            .map(|(_, dims)| dims.iter().product::<u64>())
            .sum();
        self.tensors
            .push(("output.weight".into(), vec![total as u64 - current]));
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
                Value::Bool(flag) => {
                    out.extend(7u32.to_le_bytes());
                    out.push(u8::from(*flag));
                }
                Value::Number(number) if number.is_f64() => {
                    out.extend(6u32.to_le_bytes());
                    out.extend((number.as_f64().unwrap() as f32).to_le_bytes());
                }
                Value::Number(number) => {
                    out.extend(4u32.to_le_bytes());
                    out.extend((number.as_u64().unwrap() as u32).to_le_bytes());
                }
                Value::Array(items) => {
                    out.extend(9u32.to_le_bytes());
                    let strings = items.first().is_some_and(Value::is_string);
                    out.extend(if strings { 8u32 } else { 4u32 }.to_le_bytes());
                    out.extend((items.len() as u64).to_le_bytes());
                    for item in items {
                        match item {
                            Value::String(text) => string(out, text),
                            _ => out.extend((item.as_u64().unwrap() as u32).to_le_bytes()),
                        }
                    }
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
            out.extend(12u32.to_le_bytes()); // Q4_K
            out.extend(0u64.to_le_bytes());
        }
        out
    }
}

fn dense(arch: &str) -> Gguf {
    Gguf::default()
        .kv("general.architecture", json!(arch))
        .kv("general.license", json!("apache-2.0"))
        .kv("tokenizer.ggml.tokens", json!(["a", "b", "c"]))
        .kv(&format!("{arch}.block_count"), json!(64))
        .kv(&format!("{arch}.context_length"), json!(131072))
        .kv(&format!("{arch}.embedding_length"), json!(4096))
        .kv(&format!("{arch}.attention.head_count"), json!(32))
        .kv(&format!("{arch}.attention.head_count_kv"), json!(8))
        .tensor("token_embd.weight", &[4096, 100_000])
        .tensor("blk.0.attn_q.weight", &[4096, 4096])
}

#[test]
fn library_listing_yields_repositories_newest_first_with_sourced_capabilities() {
    let entries = parse_library(LIBRARY).unwrap();
    assert_eq!(entries.len(), 16);
    let names: Vec<_> = entries
        .iter()
        .map(|entry| entry.repository.as_str())
        .collect();
    assert_eq!(
        &names[..3],
        ["mistral-large-4", "embeddinggemma-2", "clef-flash"]
    );
    let qwen = entries
        .iter()
        .find(|entry| entry.repository == "qwen3.8")
        .unwrap();
    assert_eq!(
        qwen.capabilities,
        ["vision", "reasoning", "tools"],
        "catalog order"
    );
    assert!(
        entries
            .iter()
            .all(|entry| !entry.capabilities.contains(&"cloud".to_string()))
    );
    assert!(
        parse_library("<ul></ul>").is_err(),
        "an empty listing is a crawl failure"
    );
    assert!(parse_library(&"x".repeat(4 * 1024 * 1024 + 1)).is_err());
}

#[test]
fn tag_selection_prefers_plain_sizes_then_default_quant_and_excludes_cloud_and_tensor_formats() {
    let tags = |repo: &str, html: &str| select_variants(&parse_tags(html, repo).unwrap());
    assert_eq!(
        tags(
            "qwen3.8",
            include_str!("../fixtures/admission/tags-qwen3.8.html")
        ),
        ["27b"]
    );
    assert_eq!(
        tags(
            "granite4.2",
            include_str!("../fixtures/admission/tags-granite4.2.html")
        ),
        ["30b", "3b", "8b"]
    );
    assert_eq!(
        tags(
            "qwen3.8-flash-next",
            include_str!("../fixtures/admission/tags-qwen3.8-flash-next.html")
        ),
        ["125b-a6b-q4_K_M"]
    );
    assert!(
        tags(
            "mistral-large-4",
            include_str!("../fixtures/admission/tags-mistral-large-4.html")
        )
        .is_empty()
    );
    let other = parse_tags(
        include_str!("../fixtures/admission/tags-qwen3.8.html"),
        "other",
    )
    .unwrap();
    assert!(other.is_empty(), "links to other repositories are ignored");
}

#[test]
fn gguf_metadata_streams_past_tokenizer_arrays_and_reports_truncation() {
    let bytes = dense("granite").bytes();
    let parsed = parse_gguf(&bytes).unwrap();
    assert_eq!(parsed.architecture().unwrap(), "granite");
    assert_eq!(parsed.u64("granite.context_length"), Some(131072));
    assert_eq!(parsed.text("general.license"), Some("apache-2.0"));
    assert_eq!(parsed.total_elements(), 4096 * 100_000 + 4096 * 4096);
    assert!(matches!(
        parse_gguf(&bytes[..bytes.len() - 3]),
        Err(GgufError::Truncated)
    ));
    assert!(matches!(
        parse_gguf(b"GGML\x03\0\0\0"),
        Err(GgufError::Invalid(_))
    ));
    let mut huge = b"GGUF".to_vec();
    huge.extend(3u32.to_le_bytes());
    huge.extend(u64::MAX.to_le_bytes());
    huge.extend(0u64.to_le_bytes());
    assert!(
        matches!(parse_gguf(&huge), Err(GgufError::Invalid(_))),
        "absurd counts are rejected"
    );
    // Real files carry multi-KB chat templates; long values are skipped, not fatal.
    let templated = dense("granite").kv("tokenizer.chat_template", json!("x".repeat(64 * 1024)));
    let parsed_template = parse_gguf(&templated.bytes()).unwrap();
    assert_eq!(parsed_template.text("tokenizer.chat_template"), None);
    assert_eq!(parsed_template.u64("granite.context_length"), Some(131072));
    let long_key = Gguf::default().kv(&"k".repeat(2048), json!(1));
    assert!(
        matches!(parse_gguf(&long_key.bytes()), Err(GgufError::Invalid(_))),
        "keys stay capped"
    );
}

#[test]
fn kv_geometry_is_exact_for_plain_attention_and_unknown_for_hybrid_or_windowed() {
    let plain = parse_gguf(&dense("granite").bytes()).unwrap();
    // 64 layers × 8 KV heads × (128 + 128) head dims × 2 bytes (f16).
    assert_eq!(kv_bytes_per_token(&plain), Some(64.0 * 8.0 * 256.0 * 2.0));
    let explicit = parse_gguf(
        &dense("llama")
            .kv("llama.attention.key_length", json!(256))
            .kv("llama.attention.value_length", json!(128))
            .bytes(),
    )
    .unwrap();
    assert_eq!(
        kv_bytes_per_token(&explicit),
        Some(64.0 * 8.0 * 384.0 * 2.0)
    );
    for key in [
        "qwen35.full_attention_interval",
        "qwen35.ssm.state_size",
        "qwen35.attention.sliding_window",
        "qwen35.attention.kv_lora_rank",
    ] {
        let hybrid = parse_gguf(&dense("qwen35").kv(key, json!(4)).bytes()).unwrap();
        assert_eq!(kv_bytes_per_token(&hybrid), None, "{key}");
    }
}

#[test]
fn moe_active_parameters_come_from_expert_tensors() {
    let moe = Gguf::default()
        .kv("general.architecture", json!("qwen3moe"))
        .kv("qwen3moe.expert_count", json!(128))
        .kv("qwen3moe.expert_used_count", json!(8))
        .tensor("token_embd.weight", &[1000, 1000])
        .tensor("blk.0.ffn_gate_exps.weight", &[100, 100, 128])
        .tensor("blk.0.ffn_down_exps.weight", &[100, 100, 128])
        .tensor("blk.0.ffn_gate_shexp.weight", &[100, 100]);
    let parsed = parse_gguf(&moe.bytes()).unwrap();
    let experts = 2.0 * 100.0 * 100.0 * 128.0;
    let expected = 1_000_000.0 + 10_000.0 + experts * 8.0 / 128.0;
    assert_eq!(active_params(&parsed), Some(expected));
    assert_eq!(
        active_params(&parse_gguf(&dense("llama").bytes()).unwrap()),
        None
    );
}

#[test]
fn licenses_need_a_recognised_open_source_and_agreement() {
    let apache = "\n\n                                 Apache License\n                           Version 2.0, January 2004";
    assert_eq!(
        license_id(Some("apache-2.0"), None, Some(apache)),
        Ok("apache-2.0")
    );
    assert_eq!(license_id(None, None, Some(apache)), Ok("apache-2.0"));
    assert_eq!(
        license_id(
            None,
            None,
            Some(
                "Apache License 2.0\n\nThis model fuses two artifacts, both released under Apache-2.0"
            )
        ),
        Ok("apache-2.0")
    );
    assert_eq!(license_id(Some("apache-2.0"), None, None), Ok("apache-2.0"));
    assert_eq!(
        license_id(Some("mit"), None, Some(apache)),
        Err(LicenseError::Conflict)
    );
    assert_eq!(
        license_id(
            Some("other"),
            Some("qwen-community-1.0"),
            Some("Qwen Community License 1.0\n")
        ),
        Err(LicenseError::Unrecognised)
    );
    assert_eq!(license_id(None, None, None), Err(LicenseError::Missing));
    assert_eq!(
        license_id(
            Some("apache-2.0"),
            None,
            Some("Custom terms with extra restrictions")
        ),
        Err(LicenseError::Unrecognised),
        "an unknown license text can add restrictions, so it is never overridden"
    );
}

fn input(reference: &str, gguf: &[u8]) -> (Value, Vec<u8>) {
    (registry()[reference].clone(), gguf.to_vec())
}

#[test]
fn builds_a_fully_sourced_auto_entry_and_matching_cited_observation() {
    let (record, bytes) = input("granite4.2:30b", &dense("granite").total(29.3e9).bytes());
    let tensors_match = parse_gguf(&bytes).unwrap();
    let layer = parse_layer(&record["manifest"].to_string())
        .unwrap()
        .unwrap();
    let config = RegistryConfig::parse(&record["config"].to_string()).unwrap();
    let entry = build_entry(&AdmissionInput {
        repository: "granite4.2",
        tag: "30b",
        capabilities: &["tools".into()],
        layer: &layer,
        config: &config,
        gguf: &tensors_match,
        license_head: record["licenseHead"].as_str(),
        today: "2026-10-07",
    })
    .unwrap();
    assert_eq!(entry.id, "granite4.2:30b");
    assert_eq!(entry.provenance, EntryProvenance::Auto);
    assert_eq!(entry.params, "29.3B");
    assert_eq!(entry.family, "granite");
    assert!(matches!(entry.architecture, Architecture::Dense));
    assert_eq!(entry.license, "apache-2.0");
    assert_eq!(entry.context_length, 131072.0);
    assert_eq!(entry.capabilities, ["chat", "tools"]);
    assert_eq!(entry.release_date, None);
    assert_eq!(entry.added_at.as_deref(), Some("2026-10-07"));
    assert_eq!(entry.benchmark_proxy, None);
    assert_eq!(entry.kv_bytes_per_token, Some(64.0 * 8.0 * 256.0 * 2.0));
    let quant = &entry.quantizations[0];
    assert_eq!(quant.name, "Q4_K_M");
    assert_eq!(quant.sha256.as_deref(), Some(layer.sha256.as_str()));
    assert_eq!(quant.disk_bytes, layer.disk_bytes);

    let observed = observation(&entry, "2026-10-07T03:17:00Z");
    assert_eq!(
        observed["sources"],
        json!([
            "https://registry.ollama.ai/v2/library/granite4.2/manifests/30b",
            "https://ollama.com/library/granite4.2:30b"
        ])
    );
    assert_eq!(observed["model"], serde_json::to_value(&entry).unwrap());
    let catalog =
        json!({"schemaVersion":3,"generatedAt":"2026-10-07T03:17:00.000Z","models":[entry]});
    Catalog::parse(&catalog.to_string()).unwrap();
}

#[test]
fn rejects_variants_whose_facts_cannot_be_sourced_or_disagree() {
    let build = |reference: &str, gguf: Gguf, scale: Option<f64>, caps: &[String]| {
        let record = registry()[reference].clone();
        let gguf = match scale {
            Some(total) => gguf.total(total),
            None => gguf,
        };
        let parsed = parse_gguf(&gguf.bytes()).unwrap();
        let layer = parse_layer(&record["manifest"].to_string())
            .unwrap()
            .unwrap();
        let config = RegistryConfig::parse(&record["config"].to_string()).unwrap();
        build_entry(&AdmissionInput {
            repository: reference.split(':').next().unwrap(),
            tag: reference.split(':').nth(1).unwrap(),
            capabilities: caps,
            layer: &layer,
            config: &config,
            gguf: &parsed,
            license_head: record["licenseHead"].as_str(),
            today: "2026-10-07",
        })
        .map_err(|rejection| rejection.reason())
    };
    // Tensor total far from Ollama's own count: the header is not this model.
    assert_eq!(
        build("granite4.2:30b", dense("granite"), None, &[]).unwrap_err(),
        "parameter count disagrees with tensor table"
    );
    // No license layer and no GGUF license.
    let unlicensed = Gguf {
        kv: dense("qwen35")
            .kv
            .into_iter()
            .filter(|(key, _)| key != "general.license")
            .collect(),
        ..dense("qwen35")
    };
    assert_eq!(
        build("ornith-1.5:35b", unlicensed, Some(35.5e9), &[]).unwrap_err(),
        "license missing"
    );
    // MoE without expert counts cannot source active parameters.
    let moe = dense("granite").kv("granite.expert_count", json!(64));
    assert_eq!(
        build("granite4.2:30b", moe, Some(29.3e9), &[]).unwrap_err(),
        "MoE active parameters not sourced"
    );
    // Missing context length.
    let no_context = Gguf {
        kv: dense("granite")
            .kv
            .into_iter()
            .filter(|(key, _)| !key.ends_with("context_length"))
            .collect(),
        ..dense("granite")
    };
    assert_eq!(
        build("granite4.2:30b", no_context, Some(29.3e9), &[]).unwrap_err(),
        "context length missing"
    );
    // Embedding-only models do not claim chat.
    let embedding = build(
        "granite4.2:30b",
        dense("granite"),
        Some(29.3e9),
        &["embedding".into()],
    )
    .unwrap();
    assert_eq!(embedding.capabilities, ["embedding"]);
}
