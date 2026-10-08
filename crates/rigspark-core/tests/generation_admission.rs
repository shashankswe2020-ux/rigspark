use rigspark_core::{
    catalog::EntryProvenance,
    generation::{FileRole, GenerationCatalog, GenerationKind, Workflow},
    generation_admission::{
        FitOnlyInput, HfModel, fit_only_entries, kind_from_pipeline, license_from_tags,
        parse_listing, safetensors_params,
    },
};
use serde_json::{Value, json};

fn models() -> Value {
    serde_json::from_str(include_str!("../fixtures/admission/hf-models.json")).unwrap()
}
fn model(repo: &str) -> HfModel {
    HfModel::parse(&models()[repo].to_string()).unwrap()
}

/// A safetensors header for tensors with the given shapes.
fn header(shapes: &[&[u64]]) -> Vec<u8> {
    let mut object = serde_json::Map::new();
    object.insert("__metadata__".into(), json!({"format":"pt"}));
    for (index, shape) in shapes.iter().enumerate() {
        object.insert(
            format!("t{index}"),
            json!({"dtype":"BF16","shape":shape,"data_offsets":[0,0]}),
        );
    }
    let json = serde_json::to_vec(&Value::Object(object)).unwrap();
    let mut out = (json.len() as u64).to_le_bytes().to_vec();
    out.extend(json);
    out
}

#[test]
fn listing_and_model_metadata_parse_from_recorded_hugging_face_responses() {
    let repos = parse_listing(include_str!("../fixtures/admission/hf-listing.json")).unwrap();
    assert_eq!(repos[0], "Comfy-Org/Ming-Image", "newest first");
    assert!(repos.iter().all(|repo| repo.starts_with("Comfy-Org/")));
    let ming = model("Comfy-Org/Ming-Image");
    assert_eq!(ming.sha.len(), 40);
    assert_eq!(
        ming.base_model().as_deref(),
        Some("inclusionAI/Ming-Image-0.1-Design")
    );
    assert_eq!(license_from_tags(&ming.tags), Some("mit"));
    assert_eq!(
        license_from_tags(&model("Comfy-Org/Qwen-Image-2.1").tags),
        None,
        "license:other is not open"
    );
    assert!(
        parse_listing(r#"[{"id":"someone-else/model"}]"#)
            .unwrap()
            .is_empty()
    );
    assert!(HfModel::parse(&"x".repeat(5 * 1024 * 1024)).is_err());
}

#[test]
fn only_text_prompted_image_and_video_pipelines_are_generation_kinds() {
    assert_eq!(
        kind_from_pipeline(Some("text-to-image")),
        Some(GenerationKind::Image)
    );
    assert_eq!(
        kind_from_pipeline(Some("text-to-video")),
        Some(GenerationKind::Video)
    );
    for other in [
        "text-to-audio",
        "image-to-image",
        "image-text-to-video",
        "image-to-3d",
        "depth-estimation",
    ] {
        assert_eq!(kind_from_pipeline(Some(other)), None, "{other}");
    }
    assert_eq!(kind_from_pipeline(None), None);
}

#[test]
fn safetensors_headers_give_exact_parameter_counts() {
    assert_eq!(
        safetensors_params(&header(&[&[4096, 4096], &[10]])),
        Ok(4096 * 4096 + 10)
    );
    let full = header(&[&[2, 3]]);
    assert!(
        safetensors_params(&full[..full.len() - 1]).is_err(),
        "truncated header"
    );
    let mut huge = (u64::MAX).to_le_bytes().to_vec();
    huge.extend(b"{}");
    assert!(safetensors_params(&huge).is_err());
}

#[test]
fn fit_only_entries_use_each_diffusion_file_with_the_smallest_companions() {
    let ming = model("Comfy-Org/Ming-Image");
    let params = |path: &str| {
        path.contains("bf16")
            .then_some(6_150_000_000)
            .or(Some(3_100_000_000))
    };
    let entries = fit_only_entries(&FitOnlyInput {
        model: &ming,
        kind: GenerationKind::Image,
        license: "mit",
        params: &params,
        today: "2026-10-07",
    })
    .unwrap();
    assert_eq!(entries.len(), 4, "one entry per diffusion file");
    let first = &entries[0];
    assert_eq!(first.id, "ming-image:ming_image_0.1_design_bf16");
    assert_eq!(first.provenance, EntryProvenance::Auto);
    assert_eq!(first.workflow, None);
    assert_eq!(first.params, "6.2B");
    assert_eq!(first.added_at.as_deref(), Some("2026-10-07"));
    assert_eq!(first.source, "https://huggingface.co/Comfy-Org/Ming-Image");
    let encoder = first.file(FileRole::TextEncoder).unwrap();
    assert_eq!(
        encoder.file, "text_encoders/ming_image_0.1_ling_mini_2.0_w4a8.safetensors",
        "the smallest published text encoder gives a memory lower bound"
    );
    assert_eq!(encoder.revision, ming.sha);
    assert_eq!(encoder.sha256.len(), 64);
    assert!(first.file(FileRole::Vae).is_some());

    // Edit variants are not text-to-image models.
    let longcat = model("Comfy-Org/LongCat-Image");
    let entries = fit_only_entries(&FitOnlyInput {
        model: &longcat,
        kind: GenerationKind::Image,
        license: "apache-2.0",
        params: &|_| Some(6_270_000_000),
        today: "2026-10-07",
    })
    .unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["longcat-image:longcat_image_bf16"]
    );

    // Every entry validates inside a v2 generation catalog alongside the curated defaults.
    let mut catalog: Value = serde_json::from_str(rigspark_core::GENERATION_JSON).unwrap();
    catalog["schemaVersion"] = json!(2);
    catalog["models"].as_array_mut().unwrap().extend(
        entries
            .iter()
            .map(|entry| serde_json::to_value(entry).unwrap()),
    );
    let parsed = GenerationCatalog::parse(&catalog.to_string()).unwrap();
    assert!(
        parsed
            .resolve("longcat-image:longcat_image_bf16")
            .unwrap()
            .runnable()
            .is_err()
    );
    assert_eq!(
        parsed.resolve("image").unwrap().workflow,
        Some(Workflow::FluxCheckpoint)
    );

    // Unknown parameter counts are never guessed: the variant is skipped.
    let skipped = fit_only_entries(&FitOnlyInput {
        model: &longcat,
        kind: GenerationKind::Image,
        license: "apache-2.0",
        params: &|_| None,
        today: "2026-10-07",
    })
    .unwrap();
    assert!(skipped.is_empty());
}

fn entries_for(repo: &str) -> Vec<rigspark_core::generation::GenerationModel> {
    let model = model(repo);
    let recipe = rigspark_core::generation_admission::recipe(&model.id).unwrap();
    fit_only_entries(&FitOnlyInput {
        model: &model,
        kind: recipe.kind,
        license: "apache-2.0",
        params: &|_| Some(5_000_000_000),
        today: "2026-10-07",
    })
    .unwrap()
}

#[test]
fn official_recipes_promote_exactly_the_documented_files_to_runnable() {
    let wan = entries_for("Comfy-Org/Wan_2.2_ComfyUI_Repackaged");
    let ti2v = wan
        .iter()
        .find(|entry| entry.id == "wan_2.2:wan2.2_ti2v_5b_fp16")
        .unwrap();
    assert_eq!(ti2v.kind, GenerationKind::Video);
    assert_eq!(ti2v.workflow, Some(Workflow::Wan22Ti2v));
    assert_eq!(
        ti2v.workflow_source.as_deref(),
        Some("https://comfyanonymous.github.io/ComfyUI_examples/wan22/")
    );
    let names: Vec<&str> = ti2v
        .files
        .iter()
        .map(|file| file.file.rsplit('/').next().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "wan2.2_ti2v_5B_fp16.safetensors",
            "umt5_xxl_fp8_e4m3fn_scaled.safetensors",
            "wan2.2_vae.safetensors"
        ],
        "the official example's text encoder and VAE, not the smallest ones"
    );
    for excluded in ["animate", "fun_", "_i2v_", "s2v", "vace", "camera"] {
        assert!(
            !wan.iter().any(|entry| entry.id.contains(excluded)),
            "{excluded}"
        );
    }
    assert!(
        wan.iter()
            .filter(|entry| entry.id != ti2v.id)
            .all(|entry| entry.workflow.is_none()),
        "other Wan files stay fit-only"
    );

    let qwen = entries_for("Comfy-Org/Qwen-Image_ComfyUI");
    let runnable: Vec<&str> = qwen
        .iter()
        .filter(|entry| entry.workflow == Some(Workflow::QwenImage))
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(
        runnable,
        [
            "qwen-image:qwen_image_bf16",
            "qwen-image:qwen_image_fp8_e4m3fn"
        ]
    );
    let fp8 = qwen
        .iter()
        .find(|entry| entry.id == "qwen-image:qwen_image_fp8_e4m3fn")
        .unwrap();
    assert!(
        fp8.files
            .iter()
            .any(|file| file.file.ends_with("qwen_2.5_vl_7b_fp8_scaled.safetensors"))
    );
    assert!(
        fp8.files
            .iter()
            .any(|file| file.file.ends_with("qwen_image_vae.safetensors"))
    );

    let mut catalog: Value = serde_json::from_str(rigspark_core::GENERATION_JSON).unwrap();
    catalog["schemaVersion"] = json!(2);
    catalog["models"].as_array_mut().unwrap().extend(
        wan.iter()
            .chain(qwen.iter())
            .map(|entry| serde_json::to_value(entry).unwrap()),
    );
    let parsed = GenerationCatalog::parse(&catalog.to_string()).unwrap();
    assert_eq!(
        parsed
            .resolve("wan_2.2:wan2.2_ti2v_5b_fp16")
            .unwrap()
            .runnable()
            .unwrap(),
        Workflow::Wan22Ti2v
    );
    assert!(rigspark_core::generation_admission::recipe("Comfy-Org/Ming-Image").is_none());
}
