//! Bundled ComfyUI workflow files: an API-format graph plus the only values RigSpark
//! substitutes at run time. Adding a workflow is a data change, not new graph code.

use crate::{
    catalog::require,
    generation::{FileRole, GenerationKind, Workflow},
    sizing::ValidationError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

pub const WORKFLOW_SCHEMA_VERSION: u8 = 1;
const MAX_NODES: usize = 64;

const BUNDLED: [&str; 6] = [
    include_str!("../data/workflows/flux-checkpoint.json"),
    include_str!("../data/workflows/flux-split.json"),
    include_str!("../data/workflows/wan-t2v.json"),
    include_str!("../data/workflows/wan22-ti2v.json"),
    include_str!("../data/workflows/qwen-image.json"),
    include_str!("../data/workflows/ming-design.json"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Candidate,
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Target {
    pub node: String,
    pub input: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Png,
    Webp,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Output {
    pub node: String,
    pub format: OutputFormat,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Bindings {
    pub prompt: Vec<Target>,
    pub seed: Vec<Target>,
    pub output: Output,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSlot {
    pub node: String,
    pub input: String,
    pub role: FileRole,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Override {
    /// ComfyUI `/system_stats` device type this applies to, e.g. `mps`.
    pub device: String,
    pub node: String,
    pub input: String,
    pub value: Value,
    pub message: String,
}

/// A native node class whose presence proves the running ComfyUI implements the model.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Marker {
    pub class: String,
    pub module: String,
    pub output: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preflight {
    pub markers: Vec<Marker>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkflowFile {
    pub schema_version: u8,
    pub id: Workflow,
    pub kind: GenerationKind,
    pub status: Status,
    pub source: String,
    pub graph: Map<String, Value>,
    pub bindings: Bindings,
    pub models: Vec<ModelSlot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<Preflight>,
}

/// A graph ready to submit, plus user-facing notes about device-specific changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub graph: Value,
    pub notes: Vec<String>,
}

fn invalid(message: impl Into<String>) -> ValidationError {
    ValidationError(message.into())
}

impl WorkflowFile {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        let file: Self = serde_json::from_str(raw).map_err(|error| invalid(error.to_string()))?;
        file.validate()?;
        Ok(file)
    }

    pub fn verified(&self) -> bool {
        self.status == Status::Verified
    }

    pub fn output_node(&self) -> &str {
        &self.bindings.output.node
    }

    fn input(&self, target: &Target) -> Option<&Value> {
        self.graph
            .get(&target.node)?
            .get("inputs")?
            .get(&target.input)
    }

    fn validate(&self) -> Result<(), ValidationError> {
        require(
            self.schema_version == WORKFLOW_SCHEMA_VERSION,
            "unsupported workflow schema version",
        )?;
        require(self.kind == self.id.kind(), "workflow kind mismatch")?;
        require(
            self.source.starts_with("https://") && self.source.len() <= 2048,
            "workflow source must be HTTPS",
        )?;
        require(
            !self.graph.is_empty() && self.graph.len() <= MAX_NODES,
            "workflow graph must have 1..=64 nodes",
        )?;
        for node in self.graph.values() {
            require(
                node.get("class_type").is_some_and(Value::is_string)
                    && node.get("inputs").is_some_and(Value::is_object)
                    && node.as_object().is_some_and(|node| node.len() == 2),
                "workflow nodes need only class_type and inputs",
            )?;
        }
        let bindings = &self.bindings;
        require(
            bindings.prompt.len() == 1 && bindings.seed.len() == 1,
            "workflow needs exactly one prompt and one seed binding",
        )?;
        for target in bindings.prompt.iter().chain(&bindings.seed) {
            require(
                self.input(target).is_some(),
                "binding targets a missing input",
            )?;
        }
        let save = self
            .graph
            .get(self.output_node())
            .and_then(|node| node["class_type"].as_str());
        let expected = match (self.kind, bindings.output.format) {
            (GenerationKind::Image, OutputFormat::Png) => "SaveImage",
            (GenerationKind::Video, OutputFormat::Webp) => "SaveAnimatedWEBP",
            _ => return Err(invalid("output format does not match the workflow kind")),
        };
        require(
            save == Some(expected),
            "output binding must target the save node",
        )?;
        let mut roles = HashSet::new();
        for slot in &self.models {
            require(
                self.input(&Target {
                    node: slot.node.clone(),
                    input: slot.input.clone(),
                })
                .is_some(),
                "model slot targets a missing input",
            )?;
            require(roles.insert(slot.role), "duplicate model role")?;
        }
        require(
            roles == self.id.roles().iter().copied().collect(),
            "model slots must match the workflow's file roles",
        )?;
        for change in &self.overrides {
            require(
                !change.device.is_empty()
                    && !change.message.is_empty()
                    && self
                        .input(&Target {
                            node: change.node.clone(),
                            input: change.input.clone(),
                        })
                        .is_some(),
                "override targets a missing input",
            )?;
        }
        if let Some(preflight) = &self.preflight {
            require(
                !preflight.markers.is_empty()
                    && preflight.markers.iter().all(|marker| {
                        !marker.class.is_empty()
                            && marker.module.starts_with("comfy_extras.")
                            && !marker.output.is_empty()
                    }),
                "preflight markers must name native comfy_extras nodes",
            )?;
        }
        Ok(())
    }

    /// Builds the API-format graph for one run. Only declared bindings change.
    pub fn apply(
        &self,
        prompt: &str,
        seed: u64,
        names: &HashMap<FileRole, String>,
        devices: &[String],
    ) -> Result<Applied, ValidationError> {
        let mut graph = Value::Object(self.graph.clone());
        let mut set = |node: &str, input: &str, value: Value| {
            graph[node]["inputs"][input] = value;
        };
        for target in &self.bindings.prompt {
            set(&target.node, &target.input, Value::from(prompt));
        }
        for target in &self.bindings.seed {
            set(&target.node, &target.input, Value::from(seed));
        }
        for slot in &self.models {
            let name = names
                .get(&slot.role)
                .ok_or_else(|| invalid("missing installed weight name"))?;
            set(&slot.node, &slot.input, Value::from(name.as_str()));
        }
        let mut notes = Vec::new();
        for change in &self.overrides {
            if devices.iter().any(|device| device == &change.device) {
                set(&change.node, &change.input, change.value.clone());
                if !notes.contains(&change.message) {
                    notes.push(change.message.clone());
                }
            }
        }
        Ok(Applied { graph, notes })
    }
}

pub struct Registry(Vec<WorkflowFile>);

impl Registry {
    pub fn get(&self, id: Workflow) -> &WorkflowFile {
        self.0
            .iter()
            .find(|file| file.id == id)
            .expect("registry validated one file per workflow")
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn load() -> Result<Registry, ValidationError> {
    let files = BUNDLED
        .iter()
        .map(|raw| WorkflowFile::parse(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let ids: HashSet<_> = files.iter().map(|file| file.id).collect();
    require(ids.len() == files.len(), "duplicate bundled workflow id")?;
    Ok(Registry(files))
}

/// The bundled workflows, parsed and validated once.
pub fn registry() -> Result<&'static Registry, ValidationError> {
    static REGISTRY: OnceLock<Result<Registry, ValidationError>> = OnceLock::new();
    REGISTRY
        .get_or_init(load)
        .as_ref()
        .map_err(|error| ValidationError(error.0.clone()))
}
