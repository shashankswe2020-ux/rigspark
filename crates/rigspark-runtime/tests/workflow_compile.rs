//! Converting official ComfyUI templates (UI format) into API-format graphs.
//! Fixtures: Comfy-Org/workflow_templates@99e3d43745926b78466d99f937c0cd2bb622423a and
//! `/object_info` captured from ComfyUI 0.38.0 (f1072eb0350638a3390ddb6afbcaa8c6b237c6fd).
use rigspark_runtime::workflow_compile::{Compiled, MediaFormat, ObjectInfo, Target, compile};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn info() -> ObjectInfo {
    ObjectInfo::parse(include_str!("fixtures/comfyui-0.38.0-object-info.json")).unwrap()
}
fn template(name: &str) -> Value {
    let raw = match name {
        "wan" => include_str!("fixtures/templates/text_to_video_wan.json"),
        "qwen" => include_str!("fixtures/templates/image_qwen_image.json"),
        "ming" => include_str!("fixtures/templates/image_ming_image_01_design_t2i.json"),
        "wan22" => include_str!("fixtures/templates/video_wan2_2_14B_t2v.json"),
        _ => unreachable!(),
    };
    serde_json::from_str(raw).unwrap()
}
fn build(name: &str, value: &Value) -> Result<Compiled, String> {
    compile(name, &value.to_string(), &info()).map_err(|error| error.to_string())
}
fn ok(name: &str) -> Compiled {
    build(name, &template(name)).unwrap()
}
fn target(node: &str, input: &str) -> Target {
    Target {
        node: node.into(),
        input: input.into(),
    }
}
fn ids(compiled: &Compiled) -> BTreeSet<&str> {
    compiled.graph.keys().map(String::as_str).collect()
}
fn shipped(id: &str) -> Value {
    let registry: Value = serde_json::from_str(rigspark_core::workflow::WORKFLOWS_JSON).unwrap();
    registry["workflows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["id"] == id)
        .unwrap()["graph"]
        .clone()
}
fn models(compiled: &Compiled) -> Vec<(&str, &str)> {
    let mut found: Vec<_> = compiled
        .models
        .iter()
        .map(|slot| (slot.directory.as_str(), slot.name.as_str()))
        .collect();
    found.sort();
    found
}

#[test]
fn wan_text_to_video_maps_positional_widgets_and_skips_control_after_generate() {
    let compiled = ok("wan");
    assert_eq!(
        ids(&compiled),
        BTreeSet::from(["3", "37", "38", "39", "40", "48", "49", "50", "6", "7", "8"])
    );
    let graph = &compiled.graph;
    assert_eq!(
        graph["3"],
        json!({"class_type": "KSampler", "inputs": {
            "seed": 82628696717253u64, "steps": 30, "cfg": 6, "sampler_name": "uni_pc",
            "scheduler": "simple", "denoise": 1, "model": ["48", 0], "positive": ["6", 0],
            "negative": ["7", 0], "latent_image": ["40", 0]}})
    );
    assert_eq!(
        graph["40"]["inputs"],
        json!({"width": 832, "height": 480, "length": 33, "batch_size": 1})
    );
    // Optional widgets added after the template was saved are omitted, so ComfyUI defaults apply.
    assert_eq!(
        graph["49"]["inputs"],
        json!({"images": ["8", 0], "fps": 16})
    );
    assert_eq!(
        graph["50"]["inputs"],
        json!({"video": ["49", 0], "filename_prefix": "video/ComfyUI", "format": "auto", "format.codec": "auto"})
    );
    assert_eq!(compiled.prompt, target("6", "text"));
    assert_eq!(compiled.seeds, [target("3", "seed")]);
    assert_eq!(compiled.output.node, "50");
    assert_eq!(compiled.output.format, MediaFormat::Mp4);
    assert_eq!(
        models(&compiled),
        [
            ("diffusion_models", "wan2.1_t2v_1.3B_fp16.safetensors"),
            ("text_encoders", "umt5_xxl_fp8_e4m3fn_scaled.safetensors"),
            ("vae", "wan_2.1_vae.safetensors"),
        ]
    );
    // Same nodes as the shipped graph; documented differences are the output chain
    // (MP4 here, animated WEBP shipped), the 33-frame length (49 shipped), and the seed.
    let ours = shipped("wan-t2v");
    for node in ["6", "7", "8", "48"] {
        assert_eq!(
            graph[node]["class_type"], ours[node]["class_type"],
            "{node}"
        );
    }
    assert_eq!(graph["7"]["inputs"]["text"], ours["7"]["inputs"]["text"]);
    assert_eq!(
        graph["48"]["inputs"]["model"],
        ours["48"]["inputs"]["model"]
    );
    assert_eq!(
        graph["48"]["inputs"]["shift"].as_f64(),
        ours["48"]["inputs"]["shift"].as_f64()
    );
    for input in [
        "steps",
        "sampler_name",
        "scheduler",
        "model",
        "positive",
        "negative",
        "latent_image",
    ] {
        assert_eq!(
            graph["3"]["inputs"][input], ours["3"]["inputs"][input],
            "{input}"
        );
    }
}

#[test]
fn qwen_subgraph_is_flattened_and_constant_turbo_switches_are_folded_and_pruned() {
    let compiled = ok("qwen");
    let graph = &compiled.graph;
    for gone in ["76:73", "76:78", "76:79", "76:81", "76:82", "76:83"] {
        assert!(!graph.contains_key(gone), "{gone} must be folded or pruned");
    }
    assert!(graph.keys().all(|id| id == "60" || id.starts_with("76:")));
    let sampler = &graph["76:3"]["inputs"];
    assert_eq!(sampler["seed"], 50347169638278u64);
    assert_eq!(sampler["steps"], json!(["76:84", 0]));
    assert_eq!(sampler["cfg"], json!(["76:85", 0]));
    assert_eq!(sampler["model"], json!(["76:66", 0]));
    assert_eq!(graph["76:84"]["inputs"], json!({"value": 20}));
    assert_eq!(graph["76:85"]["inputs"], json!({"value": 4}));
    assert_eq!(graph["60"]["inputs"]["images"], json!(["76:8", 0]));
    assert_eq!(compiled.prompt, target("76:6", "text"));
    assert!(
        graph["76:6"]["inputs"]["text"]
            .as_str()
            .unwrap()
            .contains("Hong Kong")
    );
    assert_eq!(compiled.seeds, [target("76:3", "seed")]);
    assert_eq!(compiled.output.format, MediaFormat::Png);
    assert_eq!(
        models(&compiled),
        [
            ("diffusion_models", "qwen_image_fp8_e4m3fn.safetensors"),
            ("text_encoders", "qwen_2.5_vl_7b_fp8_scaled.safetensors"),
            ("vae", "qwen_image_vae.safetensors"),
        ],
        "the disabled Lightning LoRA must not be required"
    );
    // Matches the shipped graph except the template's CFG 4 (2.5 shipped from the
    // ComfyUI examples page) and its primitive-node indirection for steps/CFG.
    let ours = shipped("qwen-image");
    assert_eq!(
        graph["76:66"]["inputs"]["shift"]
            .as_f64()
            .unwrap()
            .to_string(),
        "3.1000000000000005"
    );
    assert_eq!(graph["76:58"]["inputs"], ours["58"]["inputs"]);
    assert_eq!(
        graph["76:38"]["inputs"]["type"],
        ours["38"]["inputs"]["type"]
    );
}

fn ming_instance(value: &mut Value) -> &mut Value {
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == 80)
        .unwrap()
}
/// Sets one of the Ming subgraph's exposed widgets (positional and named copies).
fn set_ming(value: &mut Value, index: usize, name: &str, setting: Value) {
    let instance = ming_instance(value);
    instance["widgets_values"][index] = setting.clone();
    instance["widgets_values_named"][name] = setting;
}

#[test]
fn official_ming_template_requires_an_attention_backend_this_comfyui_lacks() {
    // The template selects Comfy Kitchen INT8 attention (Nvidia/AMD only); this ComfyUI
    // only offers PyTorch attention, so ComfyUI would reject the unmodified template.
    let error = build("ming", &template("ming")).unwrap_err();
    assert!(error.contains("comfy kitchen attention"), "{error}");
}

#[test]
fn ming_is_compiled_faithfully_including_the_enabled_prompt_enhancer() {
    let mut value = template("ming");
    set_ming(&mut value, 10, "attention", json!("pytorch attention"));
    let compiled = build("ming", &value).unwrap();
    let graph = &compiled.graph;
    assert_eq!(graph["80:43"]["class_type"], "TextGenerate");
    assert_eq!(graph["80:43"]["inputs"]["sampling_mode"], "on");
    assert_eq!(graph["80:43"]["inputs"]["sampling_mode.temperature"], 0.7);
    assert_eq!(
        graph["80:43"]["inputs"]["sampling_mode.presence_penalty"],
        0
    );
    assert_eq!(compiled.prompt, target("80:77", "value"));
    assert!(
        graph["80:77"]["inputs"]["value"]
            .as_str()
            .unwrap()
            .starts_with("A retro 1950s")
    );
    assert_eq!(compiled.seeds, [target("80:26", "noise_seed")]);
    assert_eq!(graph["80:26"]["inputs"]["noise_seed"], 43);
    assert_eq!(graph["84"]["inputs"]["format"], "png");
    assert_eq!(graph["84"]["inputs"]["format.bit_depth"], "8-bit");
    assert_eq!(compiled.output.format, MediaFormat::Png);
    assert!(
        models(&compiled).contains(&("text_encoders", "qwen3.8_27b_w4a8.safetensors")),
        "the template's prompt enhancer needs the 27B encoder"
    );
    assert_eq!(compiled.models.len(), 4);
}

#[test]
fn ming_with_prompt_enhancement_disabled_prunes_the_27b_encoder() {
    let mut value = template("ming");
    set_ming(&mut value, 10, "attention", json!("pytorch attention"));
    set_ming(&mut value, 8, "switch", json!(false));
    let compiled = build("ming", &value).unwrap();
    assert!(!compiled.graph.contains_key("80:43"));
    assert!(!compiled.graph.contains_key("80:46"));
    assert_eq!(compiled.prompt, target("80:77", "value"));
    assert_eq!(
        models(&compiled),
        [
            (
                "diffusion_models",
                "ming_image_0.1_design_int8_convrot.safetensors"
            ),
            (
                "text_encoders",
                "ming_image_0.1_ling_mini_2.0_int8_convrot.safetensors"
            ),
            ("vae", "ming_image_vae_bf16.safetensors"),
        ]
    );
}

#[test]
fn wan22_two_stage_template_binds_every_sampler_seed_and_keeps_autogrow_links() {
    let compiled = ok("wan22");
    let graph = &compiled.graph;
    assert_eq!(
        compiled.seeds,
        [
            target("128:78", "noise_seed"),
            target("128:81", "noise_seed")
        ]
    );
    let math = &graph["128:127"]["inputs"];
    assert_eq!(math["expression"], "floor(a * b) + 1");
    assert!(math["values.a"].is_array() && math["values.b"].is_array());
    assert!(math.get("values.c").is_none());
    assert_eq!(compiled.output.format, MediaFormat::Mp4);
    assert_eq!(
        models(&compiled),
        [
            (
                "diffusion_models",
                "wan2.2_t2v_high_noise_14B_fp8_scaled.safetensors"
            ),
            (
                "diffusion_models",
                "wan2.2_t2v_low_noise_14B_fp8_scaled.safetensors"
            ),
            ("text_encoders", "umt5_xxl_fp8_e4m3fn_scaled.safetensors"),
            ("vae", "wan_2.1_vae.safetensors"),
        ],
        "turbo mode is off, so the Lightning LoRAs are pruned"
    );
}

#[test]
fn bypassed_nodes_pass_their_matching_input_through() {
    let mut value = template("wan");
    for node in value["nodes"].as_array_mut().unwrap() {
        if node["id"] == 48 {
            node["mode"] = json!(4);
        }
    }
    let compiled = build("wan", &value).unwrap();
    assert!(!compiled.graph.contains_key("48"));
    assert_eq!(compiled.graph["3"]["inputs"]["model"], json!(["37", 0]));
}

fn rejected(name: &str, mutate: impl FnOnce(&mut Value), needle: &str) {
    let mut value = template(name);
    mutate(&mut value);
    let error = build(name, &value).unwrap_err();
    assert!(error.contains(needle), "expected `{needle}` in: {error}");
}
fn node_mut(value: &mut Value, id: u64) -> &mut Value {
    value["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|node| node["id"] == id)
        .unwrap()
}

#[test]
fn unknown_partner_or_custom_nodes_are_refused() {
    rejected(
        "wan",
        |v| node_mut(v, 48)["type"] = json!("OpenAIDalle3"),
        "unknown node class",
    );
}

#[test]
fn muted_required_sources_are_refused() {
    rejected(
        "wan",
        |v| node_mut(v, 48)["mode"] = json!(2),
        "missing required input",
    );
}

#[test]
fn surplus_widget_values_are_refused() {
    rejected(
        "wan",
        |v| {
            node_mut(v, 48)["widgets_values"]
                .as_array_mut()
                .unwrap()
                .push(json!(1))
        },
        "widget values",
    );
}

#[test]
fn model_files_missing_from_the_template_manifest_are_refused() {
    rejected(
        "wan",
        |v| node_mut(v, 39)["widgets_values"] = json!(["unlisted_vae.safetensors"]),
        "not listed",
    );
}

#[test]
fn templates_without_exactly_one_media_save_node_are_refused() {
    rejected(
        "wan",
        |v| {
            let mut copy = node_mut(v, 50).clone();
            copy["id"] = json!(999);
            v["nodes"].as_array_mut().unwrap().push(copy);
        },
        "exactly one media save node",
    );
}

#[test]
fn ambiguous_positive_prompts_are_refused() {
    rejected(
        "wan",
        |v| {
            // Point the positive conditioning at the negative encoder as well: no unique prompt.
            for link in v["links"].as_array_mut().unwrap() {
                if link[0] == 46 {
                    link[1] = json!(7);
                }
            }
        },
        "prompt",
    );
}

#[test]
fn oversized_templates_are_refused_before_parsing() {
    let huge = format!(
        "{{\"nodes\":[],\"links\":[],\"pad\":\"{}\"}}",
        "x".repeat(5 << 20)
    );
    let error = compile("huge", &huge, &info()).unwrap_err().to_string();
    assert!(error.contains("too large"), "{error}");
}

fn extra(name: &str) -> Value {
    serde_json::from_str(match name {
        "flux_dev" => include_str!("fixtures/templates/flux_dev_checkpoint_example.json"),
        "ernie" => include_str!("fixtures/templates/image_ernie_image.json"),
        _ => unreachable!(),
    })
    .unwrap()
}

#[test]
fn a_zeroed_negative_derived_from_the_positive_is_not_a_shared_prompt() {
    // FLUX dev feeds the negative through ConditioningZeroOut(positive); its text is ignored.
    let compiled = build("flux_dev", &extra("flux_dev")).unwrap();
    let encoder = &compiled.graph[&compiled.prompt.node];
    assert_eq!(encoder["class_type"], "CLIPTextEncode");
    assert_eq!(compiled.prompt.input, "text");
    assert!(
        compiled
            .graph
            .values()
            .any(|node| node["class_type"] == "ConditioningZeroOut")
    );
}

#[test]
fn display_only_values_on_widgetless_output_nodes_are_ignored() {
    // PreviewAny stores its last preview in widgets_values even though it has no widgets.
    let error = build("ernie", &extra("ernie")).err().unwrap_or_default();
    assert!(!error.contains("widget values"), "{error}");
}
