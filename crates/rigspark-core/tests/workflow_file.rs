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
    assert_eq!(registry().unwrap().len(), ALL.len());
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
