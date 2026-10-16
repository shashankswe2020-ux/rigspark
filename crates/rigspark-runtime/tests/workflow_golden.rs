//! Data-driven graphs must equal the hand-written builders they replaced. The legacy
//! outputs were frozen from those builders before deletion (prompt and seed below).
use rigspark_core::{
    generation::{FileRole, WorkflowId},
    workflow::registry,
};
use serde_json::Value;
use std::collections::HashMap;

const PROMPT: &str = "a fox \"in\" snow\n";
const SEED: u64 = 0;

#[test]
fn bundled_workflow_files_reproduce_the_frozen_legacy_graphs_exactly() {
    let frozen: Value =
        serde_json::from_str(include_str!("fixtures/legacy-workflow-graphs.json")).unwrap();
    let names: HashMap<_, _> = [
        FileRole::Checkpoint,
        FileRole::Diffusion,
        FileRole::TextEncoder,
        FileRole::Clip,
        FileRole::Vae,
    ]
    .into_iter()
    .map(|role| {
        (
            role,
            format!(
                "rigspark/comfyui/Org/Repo@rev/{}.safetensors",
                role.folder()
            ),
        )
    })
    .collect();
    let mut checked = 0;
    for (id, devices) in frozen.as_object().unwrap() {
        let built_in = WorkflowId::new(id).unwrap();
        for (device, legacy) in devices.as_object().unwrap() {
            let devices: Vec<String> = (device != "none")
                .then(|| device.clone())
                .into_iter()
                .collect();
            let applied = registry()
                .unwrap()
                .get(&built_in)
                .unwrap()
                .apply(PROMPT, SEED, &names, &devices)
                .unwrap();
            assert_eq!(&applied.graph, legacy, "{id} on {device}");
            checked += 1;
        }
    }
    assert_eq!(checked, 18, "six workflows on three device classes");
}
