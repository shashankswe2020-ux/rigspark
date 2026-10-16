//! Bundled ComfyUI workflows (`data/workflows.json`): API-format graphs plus the only
//! values RigSpark substitutes at run time. Adding a workflow is a data change.

use crate::{
    catalog::require,
    generation::{FileRole, GenerationFile, GenerationKind, WorkflowId},
    sizing::ValidationError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

pub const WORKFLOW_SCHEMA_VERSION: u8 = 1;
const MAX_NODES: usize = 128;

pub const WORKFLOWS_JSON: &str = include_str!("../data/workflows.json");

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
    Mp4,
}

/// Hardware a candidate was validated against (ComfyUI `/system_stats` device types).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceClass {
    Cpu,
    Mps,
    Cuda,
}

/// Where a harvested workflow came from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    pub repository: String,
    pub commit: String,
    pub template: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_comfyui_version: Option<String>,
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
    /// The exact pinned weight a harvested template requires in this slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<GenerationFile>,
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
    pub id: WorkflowId,
    pub kind: GenerationKind,
    pub status: Status,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    /// Device classes the graph validated on; empty means not restricted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_classes: Vec<DeviceClass>,
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

    /// Whether a catalog entry's files fill exactly this workflow's model slots.
    pub fn accepts_files(&self, files: &[GenerationFile]) -> bool {
        if self.models.iter().all(|slot| slot.weight.is_some()) {
            let mut wanted: Vec<&GenerationFile> = self
                .models
                .iter()
                .filter_map(|slot| slot.weight.as_ref())
                .collect();
            let mut have: Vec<&GenerationFile> = files.iter().collect();
            wanted.sort_by(|left, right| left.sha256.cmp(&right.sha256));
            have.sort_by(|left, right| left.sha256.cmp(&right.sha256));
            return wanted == have;
        }
        let mut wanted: Vec<FileRole> = self.models.iter().map(|slot| slot.role).collect();
        let mut have: Vec<FileRole> = files.iter().map(|file| file.role).collect();
        wanted.sort_by_key(|role| role.folder());
        have.sort_by_key(|role| role.folder());
        wanted.len() == have.len() && wanted.iter().zip(&have).all(|(left, right)| left == right)
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
            bindings.prompt.len() == 1 && (1..=8).contains(&bindings.seed.len()),
            "workflow needs exactly one prompt and 1..=8 seed bindings",
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
        let expected: &[&str] = match (self.kind, bindings.output.format) {
            (GenerationKind::Image, OutputFormat::Png) => &["SaveImage", "SaveImageAdvanced"],
            (GenerationKind::Video, OutputFormat::Webp) => &["SaveAnimatedWEBP"],
            (GenerationKind::Video, OutputFormat::Mp4) => &["SaveVideo"],
            _ => return Err(invalid("output format does not match the workflow kind")),
        };
        require(
            save.is_some_and(|class| expected.contains(&class)),
            "output binding must target the save node",
        )?;
        match (&self.provenance, self.status) {
            (Some(provenance), _) => require(
                provenance.repository == "Comfy-Org/workflow_templates"
                    && provenance.commit.len() == 40
                    && provenance
                        .commit
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit())
                    && !provenance.template.is_empty()
                    && provenance.template.len() <= 128
                    && provenance
                        .template
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)),
                "invalid workflow provenance",
            )?,
            (None, Status::Candidate) => {
                return Err(invalid("workflow candidates need template provenance"));
            }
            (None, Status::Verified) => {}
        }
        let pinned = self.models.iter().all(|slot| slot.weight.is_some());
        let mut roles = HashSet::new();
        for slot in &self.models {
            let target = Target {
                node: slot.node.clone(),
                input: slot.input.clone(),
            };
            require(
                self.input(&target).is_some(),
                "model slot targets a missing input",
            )?;
            require(
                roles.insert(slot.role) || pinned,
                "repeated model roles need pinned weights",
            )?;
            if let Some(weight) = &slot.weight {
                require(
                    weight.role == slot.role && weight.folder == slot.role.folder(),
                    "pinned weight does not match its slot role",
                )?;
            }
        }
        require(
            !self.models.is_empty() && self.models.len() <= 16,
            "workflow needs 1..=16 model slots",
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
        let roles: HashSet<FileRole> = self.models.iter().map(|slot| slot.role).collect();
        require(
            roles.len() == self.models.len(),
            "workflows with repeated model roles cannot run yet",
        )?;
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

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Document {
    schema_version: u8,
    workflows: Vec<WorkflowFile>,
}

pub struct Registry(Vec<WorkflowFile>);

impl Registry {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        let document: Document =
            serde_json::from_str(raw).map_err(|error| invalid(error.to_string()))?;
        require(
            document.schema_version == WORKFLOW_SCHEMA_VERSION,
            "unsupported workflow registry",
        )?;
        Self::new(document.workflows)
    }
    pub fn new(files: Vec<WorkflowFile>) -> Result<Self, ValidationError> {
        require(files.len() <= 512, "too many workflows")?;
        let mut ids = HashSet::new();
        for file in &files {
            file.validate()?;
            require(ids.insert(file.id.clone()), "duplicate workflow id")?;
        }
        Ok(Self(files))
    }
    /// Canonical document text; the bundled file must already be in this form.
    pub fn to_json(&self) -> String {
        let document = Document {
            schema_version: WORKFLOW_SCHEMA_VERSION,
            workflows: self.0.clone(),
        };
        format!(
            "{}\n",
            serde_json::to_string_pretty(&document).expect("workflow documents serialize")
        )
    }
    pub fn get(&self, id: &WorkflowId) -> Option<&WorkflowFile> {
        self.0.iter().find(|file| &file.id == id)
    }
    pub fn files(&self) -> &[WorkflowFile] {
        &self.0
    }
    pub fn len(&self) -> usize {
        self.0.len()
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The bundled workflows, parsed and validated once.
pub fn registry() -> Result<&'static Registry, ValidationError> {
    static REGISTRY: OnceLock<Result<Registry, ValidationError>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| Registry::parse(WORKFLOWS_JSON))
        .as_ref()
        .map_err(|error| ValidationError(error.0.clone()))
}
