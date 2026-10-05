use rigspark_core::{
    GENERATION_JSON,
    generation::{FitVerdict, GenerationCatalog, GenerationKind, catalog_text, fit},
    sizing::Hardware,
};
use serde_json::{Value, json};

const GIB: f64 = 1073741824.0;

fn hardware(value: Value) -> Hardware {
    serde_json::from_value(value).unwrap()
}
fn apple(ram_gib: f64) -> Hardware {
    hardware(
        json!({"arch":"arm64","platform":"darwin","totalRamBytes":ram_gib*GIB,
        "freeRamBytes":ram_gib*GIB,"freeDiskBytes":500.0*GIB,"gpu":[{"vendor":"apple","vramBytes":0}]}),
    )
}
fn nvidia(vram_gib: f64, free_ram_gib: f64) -> Hardware {
    hardware(
        json!({"arch":"x64","platform":"linux","totalRamBytes":free_ram_gib*GIB,
        "freeRamBytes":free_ram_gib*GIB,"freeDiskBytes":500.0*GIB,
        "gpu":[{"vendor":"nvidia","vramBytes":vram_gib*GIB}]}),
    )
}
fn bundled_with_leading_non_default_variant() -> Value {
    let mut value: Value = serde_json::from_str(GENERATION_JSON).unwrap();
    let mut variant = model_mut(&mut value, "flux1-schnell:fp8").clone();
    variant["id"] = json!("flux1-schnell:test-variant");
    variant["default"] = json!(false);
    value["models"].as_array_mut().unwrap().insert(0, variant);
    value
}
fn model_mut<'a>(value: &'a mut Value, id: &str) -> &'a mut Value {
    value["models"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|model| model["id"] == id)
        .unwrap()
}
fn rejects(mutate: impl FnOnce(&mut Value), needle: &str) {
    let mut value = bundled_with_leading_non_default_variant();
    mutate(&mut value);
    let error = GenerationCatalog::parse(&value.to_string()).unwrap_err();
    assert!(
        error.to_string().contains(needle),
        "expected `{needle}`, got `{error}`"
    );
}

#[test]
fn bundled_catalog_has_pinned_apache_image_and_video_models() {
    let catalog = GenerationCatalog::bundled().unwrap();
    let image = catalog.resolve("image").unwrap();
    let video = catalog.resolve("video").unwrap();
    assert_eq!(image.id, "flux1-schnell:fp8");
    assert_eq!(image.kind, GenerationKind::Image);
    assert_eq!(video.id, "wan2.1-t2v:1.3b");
    assert_eq!(video.kind, GenerationKind::Video);
    for model in &catalog.models {
        assert_eq!(model.license, "apache-2.0");
        assert!(model.open_weight);
        for file in &model.files {
            assert_eq!(file.revision.len(), 40);
            assert_eq!(file.sha256.len(), 64);
            assert!(file.bytes > 0);
        }
    }
    assert_eq!(video.total_bytes(), 2838303560 + 6735906897 + 253815318);
    assert_eq!(
        catalog.resolve("wan2.1-t2v:1.3b").unwrap().id,
        "wan2.1-t2v:1.3b"
    );
    assert!(catalog.resolve("audio").is_err());
    assert!(catalog.resolve("").is_err());
}

#[test]
fn validation_fails_closed_on_untrusted_dataset_fields() {
    rejects(|v| v["schemaVersion"] = json!(2), "unsupported");
    rejects(|v| v["models"] = json!([]), "unsupported");
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["extra"] = json!(1),
        "unknown field",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["kind"] = json!("audio"),
        "unknown variant",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["license"] = json!("flux-1-dev-non-commercial"),
        "license",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["openWeight"] = json!(false),
        "license",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["id"] = json!("Bad Id"),
        "invalid generation model id",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["id"] = json!("video"),
        "invalid generation model id",
    );
    rejects(
        |v| model_mut(v, "wan2.1-t2v:1.3b")["id"] = json!("flux1-schnell:fp8"),
        "duplicate",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["params"] = json!("twelve"),
        "parameter",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["releaseDate"] = json!("2024-13-01"),
        "date",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["source"] = json!("http://example.com"),
        "HTTPS",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["workflowSource"] = json!("file:///x"),
        "HTTPS",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["workflow"] = json!("wan-t2v"),
        "workflow roles",
    );
    rejects(
        |v| {
            model_mut(v, "wan2.1-t2v:1.3b")["files"]
                .as_array_mut()
                .unwrap()
                .pop()
                .map(drop)
                .unwrap()
        },
        "workflow roles",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["files"][0]["folder"] = json!("vae"),
        "folder",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["files"][0]["folder"] = json!("../custom_nodes"),
        "folder",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["files"][0]["revision"] = json!("main"),
        "revision",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["files"][0]["repo"] = json!("../x"),
        "repository",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["files"][0]["sha256"] = json!("00"),
        "SHA-256",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["files"][0]["file"] = json!("../evil.safetensors"),
        "unsafe",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["files"][0]["file"] = json!("payload.py"),
        "safetensors",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["files"][0]["bytes"] = json!(0),
        "size",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["default"] = json!(false),
        "exactly one default",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["kind"] = json!("video"),
        "workflow",
    );
    rejects(
        |v| {
            let mut twin = model_mut(v, "flux1-schnell:fp8").clone();
            twin["id"] = json!("flux1-schnell:twin");
            v["models"].as_array_mut().unwrap().push(twin);
        },
        "exactly one default",
    );
    rejects(
        |v| model_mut(v, "flux1-schnell:fp8")["id"] = json!(null),
        "null",
    );
}

#[test]
fn fit_is_memory_only_and_never_claims_speed() {
    let catalog = GenerationCatalog::bundled().unwrap();
    let flux = catalog.resolve("image").unwrap();
    let wan = catalog.resolve("video").unwrap();

    let roomy = fit(flux, &apple(64.0));
    assert_eq!(roomy.verdict, FitVerdict::Yes);
    assert_eq!(roomy.memory_kind, "ram");
    assert_eq!(roomy.total_bytes, 17236328572);

    assert_eq!(fit(flux, &apple(16.0)).verdict, FitVerdict::No);
    // Wan: 9.83 GB total, largest 6.74 GB. 12 GiB unified → budget ≈ 8.5 GiB.
    let staged = fit(wan, &apple(12.0));
    assert_eq!(staged.verdict, FitVerdict::Slow);
    assert!(staged.reason.contains("one stage at a time"));
    assert_eq!(fit(wan, &apple(16.0)).verdict, FitVerdict::Yes);

    // Discrete 8 GiB GPU cannot hold the FLUX checkpoint but 64 GiB system RAM can.
    let offload = fit(flux, &nvidia(8.0, 64.0));
    assert_eq!(offload.verdict, FitVerdict::Slow);
    assert_eq!(offload.memory_kind, "vram");
    assert!(offload.reason.contains("system RAM"));
    assert_eq!(fit(flux, &nvidia(8.0, 8.0)).verdict, FitVerdict::No);
    assert_eq!(fit(flux, &nvidia(24.0, 8.0)).verdict, FitVerdict::Yes);
}

#[test]
fn catalog_text_lists_both_kinds_with_unknown_speed() {
    let catalog = GenerationCatalog::bundled().unwrap();
    let text = catalog_text(&catalog, &apple(16.0));
    assert!(text.contains("flux1-schnell:fp8"));
    assert!(text.contains("wan2.1-t2v:1.3b"));
    assert!(text.contains("image"));
    assert!(text.contains("video"));
    assert!(text.contains("unknown"));
    assert!(text.contains("ComfyUI"));
    assert!(text.contains("rigspark generate image --prompt"));
    assert_eq!(text, catalog_text(&catalog, &apple(16.0)));
}
