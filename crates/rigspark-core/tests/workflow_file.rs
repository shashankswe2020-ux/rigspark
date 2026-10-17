use rigspark_core::{
    generation::{FileRole, GenerationKind, WorkflowId},
    workflow::{WorkflowFile, registry},
};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

const ALL: [&str; 6] = [
    "flux-checkpoint",
    "flux-split",
    "wan-t2v",
    "wan22-ti2v",
    "qwen-image",
    "ming-design",
];

fn id(value: &str) -> WorkflowId {
    WorkflowId::new(value).unwrap()
}
fn get(value: &str) -> &'static WorkflowFile {
    registry().unwrap().get(&id(value)).unwrap()
}

fn names(prefix: &str) -> HashMap<FileRole, String> {
    [
        FileRole::Checkpoint,
        FileRole::Diffusion,
        FileRole::TextEncoder,
        FileRole::Clip,
        FileRole::Vae,
    ]
    .into_iter()
    .map(|role| (role, format!("{prefix}/{}", role.folder())))
    .collect()
}

fn wan() -> Value {
    serde_json::to_value(get("wan-t2v")).unwrap()
}

type Mutation = (&'static str, fn(&mut Value));

fn parse(value: Value) -> Result<WorkflowFile, String> {
    WorkflowFile::parse(&value.to_string()).map_err(|error| error.to_string())
}

#[test]
fn every_built_in_workflow_is_bundled_verified_with_its_kind_and_roles() {
    use FileRole::*;
    let expected: [(&str, GenerationKind, &[FileRole]); 6] = [
        ("flux-checkpoint", GenerationKind::Image, &[Checkpoint]),
        (
            "flux-split",
            GenerationKind::Image,
            &[Diffusion, TextEncoder, Clip, Vae],
        ),
        (
            "wan-t2v",
            GenerationKind::Video,
            &[Diffusion, TextEncoder, Vae],
        ),
        (
            "wan22-ti2v",
            GenerationKind::Video,
            &[Diffusion, TextEncoder, Vae],
        ),
        (
            "qwen-image",
            GenerationKind::Image,
            &[Diffusion, TextEncoder, Vae],
        ),
        (
            "ming-design",
            GenerationKind::Image,
            &[Diffusion, TextEncoder, Vae],
        ),
    ];
    for (name, kind, roles) in expected {
        let file = get(name);
        assert_eq!(file.id, id(name));
        assert_eq!(file.kind, kind, "{name}");
        let found: HashSet<_> = file.models.iter().map(|slot| slot.role).collect();
        assert_eq!(
            found,
            roles.iter().copied().collect::<HashSet<_>>(),
            "{name}"
        );
        assert!(file.verified(), "{name}");
    }
    assert_eq!(
        registry()
            .unwrap()
            .files()
            .iter()
            .filter(|file| file.verified())
            .count(),
        ALL.len()
    );
}

#[test]
fn apply_substitutes_only_the_declared_bindings() {
    let file = get("wan-t2v");
    let applied = file
        .apply("a fox in snow", 7, &names("pinned"), &[])
        .unwrap();
    let graph = &applied.graph;
    assert_eq!(graph["6"]["inputs"]["text"], "a fox in snow");
    assert_eq!(graph["3"]["inputs"]["seed"], 7);
    assert_eq!(graph["3"]["inputs"]["sampler_name"], "uni_pc");
    assert_eq!(
        graph["37"]["inputs"]["unet_name"],
        "pinned/diffusion_models"
    );
    assert_eq!(graph["38"]["inputs"]["clip_name"], "pinned/text_encoders");
    assert_eq!(graph["39"]["inputs"]["vae_name"], "pinned/vae");
    assert!(
        graph["7"]["inputs"]["text"]
            .as_str()
            .unwrap()
            .starts_with("色调艳丽")
    );
    assert!(applied.notes.is_empty());
    assert_eq!(file.output_node(), "28");
}

#[test]
fn device_overrides_apply_only_to_matching_devices_and_are_reported() {
    let file = get("wan22-ti2v");
    let applied = file
        .apply("p", 1, &names("n"), &["cuda:0".into(), "mps".into()])
        .unwrap();
    assert_eq!(applied.graph["3"]["inputs"]["sampler_name"], "euler");
    assert_eq!(
        applied.notes,
        ["ComfyUI reports Apple MPS: using the euler sampler (uni_pc diverges on MPS)"]
    );
    let other = file.apply("p", 1, &names("n"), &["cuda:0".into()]).unwrap();
    assert_eq!(other.graph["3"]["inputs"]["sampler_name"], "uni_pc");
}

#[test]
fn apply_fails_closed_without_every_installed_weight_name() {
    let file = get("flux-split");
    let mut partial = names("n");
    partial.remove(&FileRole::Clip);
    assert!(file.apply("p", 1, &partial, &[]).is_err());
}

#[test]
fn malformed_files_are_rejected() {
    let cases: [Mutation; 9] = [
        ("unknown field", |v| v["extra"] = json!(true)),
        ("dangling prompt node", |v| {
            v["bindings"]["prompt"][0]["node"] = json!("999")
        }),
        ("dangling prompt input", |v| {
            v["bindings"]["prompt"][0]["input"] = json!("nope")
        }),
        ("missing seed binding", |v| {
            v["bindings"]["seed"] = json!([])
        }),
        ("output not a save node", |v| {
            v["bindings"]["output"]["node"] = json!("3")
        }),
        ("output format mismatch", |v| {
            v["bindings"]["output"]["format"] = json!("png")
        }),
        ("dangling model slot", |v| {
            v["models"][0]["node"] = json!("999")
        }),
        ("dangling override", |v| {
            v["overrides"][0]["input"] = json!("nope")
        }),
        ("empty graph", |v| v["graph"] = json!({})),
    ];
    assert!(parse(wan()).is_ok());
    for (name, mutate) in cases {
        let mut value = wan();
        mutate(&mut value);
        assert!(parse(value).is_err(), "{name} must be rejected");
    }
}

#[test]
fn files_with_duplicate_model_roles_or_unknown_status_are_rejected() {
    let mut value = wan();
    value["models"][1]["role"] = value["models"][0]["role"].clone();
    assert!(parse(value).is_err());
    let mut value = wan();
    value["status"] = json!("trusted");
    assert!(parse(value).is_err());
    let mut value = wan();
    value["kind"] = json!("image");
    assert!(parse(value).is_err(), "kind must match the output format");
    assert_eq!(parse(wan()).unwrap().kind, GenerationKind::Video);
}

fn weight(role: &str, folder: &str, file: &str, digest: char) -> Value {
    json!({"role": role, "folder": folder, "repo": "Comfy-Org/Example",
        "revision": "a".repeat(40), "file": file, "sha256": digest.to_string().repeat(64), "bytes": 1024})
}

/// A harvested candidate: a two-stage graph with pinned weights and repeated diffusion slots.
fn candidate() -> Value {
    json!({
        "id": "video-example-t2v",
        "kind": "video",
        "status": "candidate",
        "source": "https://github.com/Comfy-Org/workflow_templates/blob/99e3d43745926b78466d99f937c0cd2bb622423a/templates/video_example.json",
        "provenance": {
            "repository": "Comfy-Org/workflow_templates",
            "commit": "99e3d43745926b78466d99f937c0cd2bb622423a",
            "template": "video_example",
            "minComfyuiVersion": "0.3.60"
        },
        "deviceClasses": ["cpu", "mps"],
        "graph": {
            "1": {"class_type": "UNETLoader", "inputs": {"unet_name": "high.safetensors", "weight_dtype": "default"}},
            "2": {"class_type": "UNETLoader", "inputs": {"unet_name": "low.safetensors", "weight_dtype": "default"}},
            "3": {"class_type": "KSamplerAdvanced", "inputs": {"noise_seed": 1, "model": ["1", 0]}},
            "4": {"class_type": "KSamplerAdvanced", "inputs": {"noise_seed": 0, "model": ["2", 0]}},
            "5": {"class_type": "CLIPTextEncode", "inputs": {"text": "a fox"}},
            "6": {"class_type": "SaveVideo", "inputs": {"video": ["4", 0], "format": "auto", "format.codec": "auto"}}
        },
        "bindings": {
            "prompt": [{"node": "5", "input": "text"}],
            "seed": [{"node": "3", "input": "noise_seed"}, {"node": "4", "input": "noise_seed"}],
            "output": {"node": "6", "format": "mp4"}
        },
        "models": [
            {"node": "1", "input": "unet_name", "role": "diffusion",
             "weight": weight("diffusion", "diffusion_models", "split_files/high.safetensors", 'b')},
            {"node": "2", "input": "unet_name", "role": "diffusion",
             "weight": weight("diffusion", "diffusion_models", "split_files/low.safetensors", 'c')}
        ]
    })
}

#[test]
fn harvested_candidates_with_pinned_repeated_slots_and_mp4_output_parse() {
    let file = parse(candidate()).unwrap();
    assert!(!file.verified());
    assert_eq!(file.device_classes.len(), 2);
    assert_eq!(file.bindings.seed.len(), 2);
    let pinned: Vec<rigspark_core::generation::GenerationFile> = file
        .models
        .iter()
        .map(|slot| slot.weight.clone().unwrap())
        .collect();
    assert!(file.accepts_files(&pinned));
    let mut other = pinned.clone();
    other[1].sha256 = "d".repeat(64);
    assert!(
        !file.accepts_files(&other),
        "pinned slots accept only their exact weights"
    );
}

#[test]
fn candidate_files_fail_closed_on_missing_or_inconsistent_pins() {
    let cases: [Mutation; 7] = [
        ("candidate without provenance", |v| {
            v.as_object_mut().unwrap().remove("provenance");
        }),
        ("short provenance commit", |v| {
            v["provenance"]["commit"] = json!("99e3d43")
        }),
        ("repeated role without pins", |v| {
            v["models"][0].as_object_mut().unwrap().remove("weight");
        }),
        ("pin role mismatch", |v| {
            v["models"][0]["weight"]["role"] = json!("vae")
        }),
        ("pin folder mismatch", |v| {
            v["models"][0]["weight"]["folder"] = json!("vae")
        }),
        ("mp4 without SaveVideo", |v| {
            v["graph"]["6"]["class_type"] = json!("SaveAnimatedWEBP")
        }),
        ("unknown device class", |v| {
            v["deviceClasses"] = json!(["tpu"])
        }),
    ];
    for (name, mutate) in cases {
        let mut value = candidate();
        mutate(&mut value);
        assert!(parse(value).is_err(), "{name} must be rejected");
    }
}

#[test]
fn repeated_role_candidates_cannot_be_applied_until_per_file_names_exist() {
    let file = parse(candidate()).unwrap();
    assert!(file.apply("p", 1, &names("n"), &[]).is_err());
}

#[test]
fn the_bundled_registry_is_stored_in_canonical_form() {
    // Harvests rewrite the file through `to_json`; keeping it canonical means a harvest
    // diff contains only the workflows it adds.
    let registry =
        rigspark_core::workflow::Registry::parse(rigspark_core::workflow::WORKFLOWS_JSON).unwrap();
    if std::env::var_os("CANONICALIZE_WORKFLOWS").is_some() {
        std::fs::write(
            concat!(env!("CARGO_MANIFEST_DIR"), "/data/workflows.json"),
            registry.to_json(),
        )
        .unwrap();
    }
    assert_eq!(registry.to_json(), rigspark_core::workflow::WORKFLOWS_JSON);
}
