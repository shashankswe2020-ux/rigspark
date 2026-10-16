//! Compiles official ComfyUI templates (frontend "UI" format) into API-format graphs
//! RigSpark can submit, using a running ComfyUI's `/object_info` as the schema.
//!
//! The compiler is faithful to the template: it never invents values. It flattens
//! subgraphs, applies bypass/mute/reroute semantics, maps positional widget values to
//! named inputs (including V3 dynamic combos), folds switches whose condition is a
//! constant, prunes everything that does not feed the single media save node, and
//! detects the prompt, seed and model bindings. Anything ambiguous fails closed.

use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt,
    rc::Rc,
};

pub use rigspark_core::workflow::Target;

pub const MAX_TEMPLATE_BYTES: usize = 4 << 20;
pub const MAX_OBJECT_INFO_BYTES: usize = 32 << 20;
const MAX_NODES: usize = 512;
const MAX_DEPTH: usize = 4;
const MAX_STEPS: usize = 100_000;
const SAVE_NODES: [&str; 4] = [
    "SaveImage",
    "SaveImageAdvanced",
    "SaveAnimatedWEBP",
    "SaveVideo",
];
const CONTROLS: [&str; 4] = ["fixed", "increment", "decrement", "randomize"];
const MODEL_EXTENSIONS: [&str; 7] = [
    ".safetensors",
    ".sft",
    ".ckpt",
    ".pt",
    ".pth",
    ".bin",
    ".gguf",
];
const PRIMITIVES: [&str; 5] = [
    "PrimitiveInt",
    "PrimitiveFloat",
    "PrimitiveBoolean",
    "PrimitiveString",
    "PrimitiveStringMultiline",
];
/// STRING inputs followed upstream when locating the user's prompt text.
const TEXT_PATH: [&str; 5] = ["value", "source", "prompt", "text", "string"];

#[derive(Debug, thiserror::Error)]
#[error("{template}: {reason}")]
pub struct CompileError {
    pub template: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaFormat {
    Png,
    Webp,
    Mp4,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledOutput {
    pub node: String,
    pub class: String,
    pub format: MediaFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSlot {
    pub node: String,
    pub input: String,
    pub name: String,
    pub directory: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    pub template: String,
    pub graph: Map<String, Value>,
    pub prompt: Target,
    pub seeds: Vec<Target>,
    pub output: CompiledOutput,
    pub models: Vec<ModelSlot>,
    /// Every node class the graph uses; the runtime preflight contract.
    pub classes: BTreeSet<String>,
}

// ---------------------------------------------------------------------------
// `/object_info` schema, parsed with document order preserved for nested inputs.

/// A JSON object whose key order is preserved (serde_json's `Map` sorts keys here).
#[derive(Debug, Clone)]
pub struct Ordered<T>(pub Vec<(String, T)>);

impl<T> Default for Ordered<T> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<T> Ordered<T> {
    fn get(&self, name: &str) -> Option<&T> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Ordered<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Collect<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for Collect<T> {
            type Value = Ordered<T>;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some((key, value)) = map.next_entry::<String, T>()? {
                    entries.push((key, value));
                }
                Ok(Ordered(entries))
            }
        }
        deserializer.deserialize_map(Collect(std::marker::PhantomData))
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Inputs {
    #[serde(default)]
    pub required: Ordered<Spec>,
    #[serde(default)]
    pub optional: Ordered<Spec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DynamicOption {
    pub key: String,
    #[serde(default)]
    pub inputs: Inputs,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Choices {
    Dynamic(Vec<DynamicOption>),
    Plain(Vec<Value>),
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AutogrowTemplate {
    #[serde(default)]
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecOptions {
    pub default: Option<Value>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub options: Option<Choices>,
    #[serde(rename = "control_after_generate")]
    pub control_after_generate: Option<Value>,
    #[serde(default)]
    pub force_input: bool,
    #[serde(default)]
    pub multiselect: bool,
    pub template: Option<AutogrowTemplate>,
}

/// One `/object_info` input specification: `[type, options?]`.
#[derive(Debug, Clone)]
pub struct Spec {
    /// The type name, or `None` for a legacy inline choice list.
    pub kind: Option<String>,
    pub legacy_choices: Vec<Value>,
    pub options: SpecOptions,
}

impl<'de> Deserialize<'de> for Spec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Head {
            Name(String),
            Choices(Vec<Value>),
        }
        struct Pair;
        impl<'de> Visitor<'de> for Pair {
            type Value = Spec;
            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("[type, options?]")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Spec, A::Error> {
                let head: Head = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::custom("input spec has no type"))?;
                let options: SpecOptions = seq.next_element()?.unwrap_or_default();
                if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(serde::de::Error::custom("input spec has extra elements"));
                }
                Ok(match head {
                    Head::Name(kind) => Spec {
                        kind: Some(kind),
                        legacy_choices: Vec::new(),
                        options,
                    },
                    Head::Choices(choices) => Spec {
                        kind: None,
                        legacy_choices: choices,
                        options,
                    },
                })
            }
        }
        deserializer.deserialize_seq(Pair)
    }
}

impl Spec {
    fn name(&self) -> &str {
        self.kind.as_deref().unwrap_or("COMBO")
    }
    fn dynamic(&self) -> bool {
        self.name() == "COMFY_DYNAMICCOMBO_V3"
    }
    fn autogrow(&self) -> bool {
        self.name() == "COMFY_AUTOGROW_V3"
    }
    fn widget(&self) -> bool {
        !self.options.force_input
            && matches!(
                self.name(),
                "INT" | "FLOAT" | "STRING" | "BOOLEAN" | "COMBO" | "COMFY_DYNAMICCOMBO_V3"
            )
    }
    fn control(&self) -> bool {
        self.options
            .control_after_generate
            .as_ref()
            .is_some_and(|value| !matches!(value, Value::Bool(false) | Value::Null))
    }
    fn choices(&self) -> Option<&[Value]> {
        if self.kind.is_none() {
            return Some(&self.legacy_choices);
        }
        match &self.options.options {
            Some(Choices::Plain(choices)) if self.name() == "COMBO" => Some(choices),
            _ => None,
        }
    }
    fn option(&self, key: &str) -> Option<&DynamicOption> {
        match &self.options.options {
            Some(Choices::Dynamic(options)) => options.iter().find(|option| option.key == key),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct InputOrder {
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub optional: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeSchema {
    #[serde(default)]
    pub input: Inputs,
    pub input_order: Option<InputOrder>,
    #[serde(default)]
    pub output: Vec<Value>,
    #[serde(default)]
    pub output_node: bool,
    #[serde(default)]
    pub python_module: String,
    #[serde(default)]
    pub api_node: bool,
}

impl NodeSchema {
    fn output_type(&self, slot: usize) -> Option<String> {
        self.output.get(slot).map(|kind| match kind {
            Value::String(kind) => kind.clone(),
            _ => "COMBO".into(),
        })
    }
    /// Inputs in declaration order: required first, then optional.
    fn ordered(&self) -> Vec<(&str, &Spec, bool)> {
        let mut result = Vec::new();
        match &self.input_order {
            Some(order) => {
                for name in &order.required {
                    if let Some(spec) = self.input.required.get(name) {
                        result.push((name.as_str(), spec, true));
                    }
                }
                for name in &order.optional {
                    if let Some(spec) = self.input.optional.get(name) {
                        result.push((name.as_str(), spec, false));
                    }
                }
            }
            None => {
                result.extend(
                    self.input
                        .required
                        .0
                        .iter()
                        .map(|(name, spec)| (name.as_str(), spec, true)),
                );
                result.extend(
                    self.input
                        .optional
                        .0
                        .iter()
                        .map(|(name, spec)| (name.as_str(), spec, false)),
                );
            }
        }
        result
    }
}

/// Node schemas from a ComfyUI `/object_info` response.
#[derive(Debug, Clone, Default)]
pub struct ObjectInfo(pub BTreeMap<String, NodeSchema>);

impl ObjectInfo {
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.len() > MAX_OBJECT_INFO_BYTES {
            return Err("object_info is too large".into());
        }
        let ordered: Ordered<NodeSchema> =
            serde_json::from_str(raw).map_err(|error| error.to_string())?;
        Ok(Self(ordered.0.into_iter().collect()))
    }
    fn get(&self, class: &str) -> Option<&NodeSchema> {
        self.0.get(class)
    }
}

// ---------------------------------------------------------------------------
// Template (UI format) model.

#[derive(Debug, Deserialize)]
struct Ui {
    #[serde(default)]
    nodes: Vec<UiNode>,
    #[serde(default)]
    links: Vec<UiLink>,
    #[serde(default)]
    definitions: Option<Definitions>,
}

#[derive(Debug, Default, Deserialize)]
struct Definitions {
    #[serde(default)]
    subgraphs: Vec<Subgraph>,
}

#[derive(Debug, Deserialize)]
struct Port {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Subgraph {
    id: String,
    #[serde(default)]
    inputs: Vec<Port>,
    #[serde(default)]
    nodes: Vec<UiNode>,
    #[serde(default)]
    links: Vec<UiLink>,
}

#[derive(Debug, Deserialize)]
struct UiInput {
    name: String,
    #[serde(default)]
    link: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct TemplateModel {
    name: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    directory: String,
}

#[derive(Debug, Default, Deserialize)]
struct Properties {
    #[serde(default)]
    models: Vec<TemplateModel>,
}

#[derive(Debug, Deserialize)]
struct UiNode {
    id: Value,
    #[serde(rename = "type")]
    class: String,
    #[serde(default)]
    mode: u8,
    #[serde(default)]
    inputs: Vec<UiInput>,
    #[serde(default)]
    widgets_values: Option<Value>,
    #[serde(default)]
    widgets_values_named: Option<Map<String, Value>>,
    #[serde(default)]
    properties: Option<Properties>,
}

impl UiNode {
    fn key(&self) -> String {
        match &self.id {
            Value::String(id) => id.clone(),
            other => other.to_string(),
        }
    }
    fn widgets(&self) -> Option<&Vec<Value>> {
        match &self.widgets_values {
            None | Some(Value::Null) => None,
            Some(Value::Array(values)) => Some(values),
            Some(_) => None,
        }
    }
}

#[derive(Debug, Clone)]
struct UiLink {
    id: i64,
    origin: i64,
    origin_slot: usize,
    target: i64,
    target_slot: usize,
}

impl<'de> Deserialize<'de> for UiLink {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        let int = |value: &Value| value.as_i64();
        let parsed = match &value {
            Value::Array(items) if items.len() >= 5 => (
                int(&items[0]),
                int(&items[1]),
                int(&items[2]),
                int(&items[3]),
                int(&items[4]),
            ),
            Value::Object(fields) => (
                fields.get("id").and_then(int),
                fields.get("origin_id").and_then(int),
                fields.get("origin_slot").and_then(int),
                fields.get("target_id").and_then(int),
                fields.get("target_slot").and_then(int),
            ),
            _ => (None, None, None, None, None),
        };
        match parsed {
            (Some(id), Some(origin), Some(origin_slot), Some(target), Some(target_slot))
                if origin_slot >= 0 && target_slot >= 0 =>
            {
                Ok(UiLink {
                    id,
                    origin,
                    origin_slot: origin_slot as usize,
                    target,
                    target_slot: target_slot as usize,
                })
            }
            _ => Err(serde::de::Error::custom("malformed link")),
        }
    }
}

// ---------------------------------------------------------------------------
// Flattening.

const SUBGRAPH_INPUT: i64 = -10;
const SUBGRAPH_OUTPUT: i64 = -20;

#[derive(Debug, Clone, PartialEq)]
enum Source {
    Link(String, usize),
    Const(Value),
    /// A subgraph input with no external value: the inner widget keeps its own value.
    Unbound,
}

#[derive(Debug)]
struct Flat {
    id: String,
    class: String,
    mode: u8,
    widgets: Vec<Value>,
    sources: Vec<(String, Source)>,
}

struct Scope<'a> {
    prefix: String,
    nodes: &'a [UiNode],
    links: &'a [UiLink],
    parent: Option<(Rc<Scope<'a>>, &'a UiNode, &'a Subgraph)>,
    depth: usize,
}

impl<'a> Scope<'a> {
    fn child(self: &Rc<Self>, instance: &'a UiNode, subgraph: &'a Subgraph) -> Rc<Self> {
        Rc::new(Scope {
            prefix: format!("{}{}:", self.prefix, instance.key()),
            nodes: &subgraph.nodes,
            links: &subgraph.links,
            parent: Some((Rc::clone(self), instance, subgraph)),
            depth: self.depth + 1,
        })
    }
}

struct Flattener<'a> {
    template: &'a str,
    subgraphs: HashMap<&'a str, &'a Subgraph>,
    flat: Vec<Flat>,
    manifest: BTreeMap<String, (String, String)>,
    steps: usize,
}

impl<'a> Flattener<'a> {
    fn fail(&self, reason: impl Into<String>) -> CompileError {
        CompileError {
            template: self.template.into(),
            reason: reason.into(),
        }
    }
    fn tick(&mut self) -> Result<(), CompileError> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(self.fail("template is too complex"));
        }
        Ok(())
    }
    fn subgraph(
        &self,
        node: &UiNode,
        scope: &Scope<'a>,
    ) -> Result<Option<&'a Subgraph>, CompileError> {
        match self.subgraphs.get(node.class.as_str()).copied() {
            Some(_) if scope.depth >= MAX_DEPTH => {
                Err(self.fail("subgraphs are nested too deeply"))
            }
            found => Ok(found),
        }
    }
    fn resolve_link(&mut self, scope: &Rc<Scope<'a>>, link: i64) -> Result<Source, CompileError> {
        let edge = scope
            .links
            .iter()
            .find(|edge| edge.id == link)
            .cloned()
            .ok_or_else(|| self.fail(format!("link {link} is missing")))?;
        self.resolve_origin(scope, edge.origin, edge.origin_slot)
    }
    fn resolve_origin(
        &mut self,
        scope: &Rc<Scope<'a>>,
        origin: i64,
        slot: usize,
    ) -> Result<Source, CompileError> {
        self.tick()?;
        if origin == SUBGRAPH_INPUT {
            let (outer, instance, subgraph) = scope
                .parent
                .clone()
                .ok_or_else(|| self.fail("subgraph input used outside a subgraph"))?;
            let port = subgraph
                .inputs
                .get(slot)
                .ok_or_else(|| self.fail("subgraph input slot is missing"))?;
            if let Some(link) = instance
                .inputs
                .iter()
                .find(|input| input.name == port.name)
                .and_then(|input| input.link)
            {
                return self.resolve_link(&outer, link);
            }
            let named = instance
                .widgets_values_named
                .as_ref()
                .and_then(|named| named.get(&port.name));
            let positional = instance
                .widgets()
                .filter(|values| values.len() == subgraph.inputs.len())
                .and_then(|values| values.get(slot));
            return Ok(named
                .or(positional)
                .map_or(Source::Unbound, |value| Source::Const(value.clone())));
        }
        let node = scope
            .nodes
            .iter()
            .find(|node| node.id.as_i64() == Some(origin) || node.key() == origin.to_string())
            .ok_or_else(|| self.fail(format!("link origin {origin} is missing")))?;
        if let Some(subgraph) = self.subgraph(node, scope)? {
            let inner = scope.child(node, subgraph);
            let edge = subgraph
                .links
                .iter()
                .find(|link| link.target == SUBGRAPH_OUTPUT && link.target_slot == slot)
                .cloned()
                .ok_or_else(|| self.fail("subgraph output is not connected"))?;
            return self.resolve_origin(&inner, edge.origin, edge.origin_slot);
        }
        Ok(Source::Link(
            format!("{}{}", scope.prefix, node.key()),
            slot,
        ))
    }
    fn expand(&mut self, scope: &Rc<Scope<'a>>) -> Result<(), CompileError> {
        for node in scope.nodes {
            self.tick()?;
            if let Some(properties) = &node.properties {
                for model in &properties.models {
                    self.manifest.insert(
                        model.name.clone(),
                        (model.directory.clone(), model.url.clone()),
                    );
                }
            }
            if let Some(subgraph) = self.subgraph(node, scope)? {
                self.expand(&scope.child(node, subgraph))?;
                continue;
            }
            let mut sources = Vec::new();
            for input in &node.inputs {
                if let Some(link) = input.link {
                    sources.push((input.name.clone(), self.resolve_link(scope, link)?));
                }
            }
            self.flat.push(Flat {
                id: format!("{}{}", scope.prefix, node.key()),
                class: node.class.clone(),
                mode: node.mode,
                widgets: node.widgets().cloned().unwrap_or_default(),
                sources,
            });
            if self.flat.len() > MAX_NODES {
                return Err(self.fail("template has too many nodes"));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Compilation.

#[derive(Debug)]
struct Node {
    class: String,
    inputs: Map<String, Value>,
    specs: HashMap<String, Spec>,
    error: Option<String>,
}

fn widget_walk(
    entries: &[(&str, &Spec, bool)],
    prefix: &str,
    values: &mut std::slice::Iter<'_, Value>,
    inputs: &mut Map<String, Value>,
    specs: &mut HashMap<String, Spec>,
) -> Result<(), String> {
    for (name, spec, required) in entries {
        let full = format!("{prefix}{name}");
        specs.insert(full.clone(), (*spec).clone());
        if !spec.widget() {
            continue;
        }
        let value = match values.next() {
            Some(value) => value.clone(),
            None if !required => continue,
            None => match &spec.options.default {
                Some(default) => default.clone(),
                None => return Err(format!("missing widget value for {full}")),
            },
        };
        if spec.control() {
            match values.as_slice().first() {
                Some(Value::String(control)) if CONTROLS.contains(&control.as_str()) => {
                    values.next();
                }
                _ => {}
            }
        }
        if spec.dynamic() {
            let key = value
                .as_str()
                .ok_or_else(|| format!("{full} must select an option"))?;
            let option = spec
                .option(key)
                .ok_or_else(|| format!("{full} has no option `{key}`"))?;
            let nested: Vec<(&str, &Spec, bool)> = option
                .inputs
                .required
                .0
                .iter()
                .map(|(name, spec)| (name.as_str(), spec, true))
                .chain(
                    option
                        .inputs
                        .optional
                        .0
                        .iter()
                        .map(|(name, spec)| (name.as_str(), spec, false)),
                )
                .collect();
            inputs.insert(full.clone(), value);
            widget_walk(&nested, &format!("{full}."), values, inputs, specs)?;
        } else {
            inputs.insert(full, value);
        }
    }
    Ok(())
}

/// Literal inputs from widget values, and the schema of every input name they produced.
type Mapped = (Map<String, Value>, HashMap<String, Spec>);

fn map_widgets(flat: &Flat, schema: &NodeSchema) -> Result<Mapped, String> {
    let mut inputs = Map::new();
    let mut specs = HashMap::new();
    let mut values = flat.widgets.iter();
    widget_walk(&schema.ordered(), "", &mut values, &mut inputs, &mut specs)?;
    if values.next().is_some() {
        return Err(format!(
            "{} ({}) has more widget values than its ComfyUI schema",
            flat.id, flat.class
        ));
    }
    Ok((inputs, specs))
}

fn link(value: &Value) -> Option<(&str, u64)> {
    let items = value.as_array().filter(|items| items.len() == 2)?;
    Some((items[0].as_str()?, items[1].as_u64()?))
}

fn compatible(origin: &str, expected: &str) -> bool {
    let wild = |kind: &str| kind == "*" || kind.starts_with("COMFY_MATCHTYPE");
    wild(origin)
        || wild(expected)
        || origin == expected
        || expected.split(',').any(|kind| kind.trim() == origin)
        || origin.split(',').any(|kind| kind.trim() == expected)
}

pub fn compile(template: &str, raw: &str, info: &ObjectInfo) -> Result<Compiled, CompileError> {
    let fail = |reason: String| CompileError {
        template: template.into(),
        reason,
    };
    if raw.len() > MAX_TEMPLATE_BYTES {
        return Err(fail("template is too large".into()));
    }
    let ui: Ui =
        serde_json::from_str(raw).map_err(|error| fail(format!("invalid template: {error}")))?;
    let definitions = ui.definitions.unwrap_or_default();
    let mut flattener = Flattener {
        template,
        subgraphs: definitions
            .subgraphs
            .iter()
            .map(|subgraph| (subgraph.id.as_str(), subgraph))
            .collect(),
        flat: Vec::new(),
        manifest: BTreeMap::new(),
        steps: 0,
    };
    let root = Rc::new(Scope {
        prefix: String::new(),
        nodes: &ui.nodes,
        links: &ui.links,
        parent: None,
        depth: 0,
    });
    flattener.expand(&root)?;
    let manifest = std::mem::take(&mut flattener.manifest);
    let flat = std::mem::take(&mut flattener.flat);

    // Frontend-only nodes and modes: drop notes and muted nodes, pass through reroutes
    // and bypassed nodes, fold legacy PrimitiveNode widgets into their consumers.
    let by_id: HashMap<&str, &Flat> = flat.iter().map(|node| (node.id.as_str(), node)).collect();
    let pass_through = |id: &str, slot: usize| -> Option<Option<Source>> {
        let node = by_id.get(id)?;
        if node.class == "Reroute" {
            return Some(node.sources.first().map(|(_, source)| source.clone()));
        }
        if node.class == "PrimitiveNode" {
            return Some(node.widgets.first().cloned().map(Source::Const));
        }
        if node.mode == 2 || matches!(node.class.as_str(), "Note" | "MarkdownNote") {
            return Some(None);
        }
        if node.mode == 4 {
            let schema = info.get(&node.class)?;
            let kind = schema.output_type(slot)?;
            let source = schema
                .ordered()
                .into_iter()
                .filter(|(_, spec, _)| compatible(&kind, spec.name()) && !spec.widget())
                .find_map(|(name, _, _)| {
                    node.sources
                        .iter()
                        .find(|(input, _)| input == name)
                        .map(|(_, source)| source.clone())
                });
            return Some(source);
        }
        None
    };
    let resolve = |source: &Source| -> Result<Option<Source>, CompileError> {
        let mut current = source.clone();
        for _ in 0..64 {
            let Source::Link(id, slot) = &current else {
                return Ok(Some(current));
            };
            match pass_through(id, *slot) {
                None => return Ok(Some(current)),
                Some(None) => return Ok(None),
                Some(Some(next)) => current = next,
            }
        }
        Err(fail("reroute or bypass chain is too long".into()))
    };

    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
    for node in &flat {
        if node.mode == 2
            || node.mode == 4
            || matches!(
                node.class.as_str(),
                "Note" | "MarkdownNote" | "Reroute" | "PrimitiveNode"
            )
        {
            continue;
        }
        if node.mode != 0 {
            return Err(fail(format!(
                "{} has unsupported mode {}",
                node.id, node.mode
            )));
        }
        let Some(schema) = info.get(&node.class) else {
            nodes.insert(
                node.id.clone(),
                Node {
                    class: node.class.clone(),
                    inputs: Map::new(),
                    specs: HashMap::new(),
                    error: Some(format!("unknown node class {} ({})", node.class, node.id)),
                },
            );
            continue;
        };
        let (mut inputs, specs, error) = match map_widgets(node, schema) {
            Ok((inputs, specs)) => (inputs, specs, None),
            Err(error) => (Map::new(), HashMap::new(), Some(error)),
        };
        for (name, source) in &node.sources {
            match resolve(source)? {
                Some(Source::Link(id, slot)) => {
                    inputs.insert(name.clone(), json!([id, slot]));
                }
                Some(Source::Const(value)) => {
                    inputs.insert(name.clone(), value);
                }
                Some(Source::Unbound) => {}
                None => {
                    inputs.remove(name);
                }
            }
        }
        nodes.insert(
            node.id.clone(),
            Node {
                class: node.class.clone(),
                inputs,
                specs,
                error,
            },
        );
    }

    // Constant folding: a switch whose condition is a literal (directly or through
    // primitive nodes) is replaced by its selected branch.
    let constant = |nodes: &BTreeMap<String, Node>, value: &Value| -> Option<Value> {
        let mut current = value.clone();
        for _ in 0..16 {
            let Some((id, _)) = link(&current) else {
                return Some(current);
            };
            let node = nodes.get(id)?;
            if !PRIMITIVES.contains(&node.class.as_str()) {
                return None;
            }
            current = node.inputs.get("value")?.clone();
        }
        None
    };
    loop {
        let switch = nodes.iter().find_map(|(id, node)| {
            (node.class == "ComfySwitchNode")
                .then(|| {
                    let condition = constant(&nodes, node.inputs.get("switch")?)?.as_bool()?;
                    let branch = if condition { "on_true" } else { "on_false" };
                    Some((id.clone(), node.inputs.get(branch).cloned()))
                })
                .flatten()
        });
        let Some((id, branch)) = switch else { break };
        nodes.remove(&id);
        for node in nodes.values_mut() {
            for value in node.inputs.values_mut() {
                if link(value).is_some_and(|(origin, _)| origin == id) {
                    *value = branch.clone().unwrap_or(Value::Null);
                }
            }
            node.inputs.retain(|_, value| !value.is_null());
        }
    }

    // Exactly one media save node defines the output; keep only what feeds it.
    let saves: Vec<&String> = nodes
        .iter()
        .filter(|(_, node)| SAVE_NODES.contains(&node.class.as_str()))
        .map(|(id, _)| id)
        .collect();
    let [save] = saves.as_slice() else {
        return Err(fail(format!(
            "expected exactly one media save node, found {}",
            saves.len()
        )));
    };
    let save = (*save).clone();
    let mut keep = BTreeSet::new();
    let mut stack = vec![save.clone()];
    while let Some(id) = stack.pop() {
        if !keep.insert(id.clone()) {
            continue;
        }
        if let Some(node) = nodes.get(&id) {
            for value in node.inputs.values() {
                if let Some((origin, _)) = link(value) {
                    stack.push(origin.to_owned());
                }
            }
        }
    }
    nodes.retain(|id, _| keep.contains(id));

    // Validate every surviving node against the live schema.
    let mut graph = Map::new();
    let mut models = Vec::new();
    let mut seeds = Vec::new();
    let mut classes = BTreeSet::new();
    if let Some(error) = nodes.values().find_map(|node| node.error.clone()) {
        return Err(fail(error));
    }
    for (id, node) in &nodes {
        let schema = info
            .get(&node.class)
            .ok_or_else(|| fail(format!("unknown node class {}", node.class)))?;
        if schema.api_node
            || !(schema.python_module == "nodes"
                || schema.python_module.starts_with("comfy_extras."))
        {
            return Err(fail(format!(
                "{} ({}) is not a local core node",
                node.class, schema.python_module
            )));
        }
        for (name, spec, required) in schema.ordered() {
            if required && !spec.autogrow() && !node.inputs.contains_key(name) {
                return Err(fail(format!(
                    "missing required input {}.{name} ({id})",
                    node.class
                )));
            }
        }
        for (name, value) in &node.inputs {
            let spec = match node.specs.get(name) {
                Some(spec) => spec.clone(),
                None => {
                    let (group, member) = name
                        .split_once('.')
                        .ok_or_else(|| fail(format!("unknown input {}.{name}", node.class)))?;
                    let grows = node
                        .specs
                        .get(group)
                        .filter(|spec| spec.autogrow())
                        .is_some_and(|spec| {
                            spec.options.template.as_ref().is_some_and(|template| {
                                template.names.iter().any(|known| known == member)
                            })
                        });
                    if !grows || link(value).is_none() {
                        return Err(fail(format!("unknown input {}.{name}", node.class)));
                    }
                    continue;
                }
            };
            if let Some((origin, slot)) = link(value) {
                let source = nodes
                    .get(origin)
                    .ok_or_else(|| fail(format!("{id}.{name} links to a missing node")))?;
                let kind = info
                    .get(&source.class)
                    .and_then(|schema| schema.output_type(slot as usize))
                    .ok_or_else(|| fail(format!("{id}.{name} links to a missing output")))?;
                if !compatible(&kind, spec.name()) {
                    return Err(fail(format!(
                        "{id}.{name} expects {} but receives {kind}",
                        spec.name()
                    )));
                }
                continue;
            }
            if let Some(file) = value.as_str().filter(|text| {
                MODEL_EXTENSIONS
                    .iter()
                    .any(|extension| text.ends_with(extension))
            }) {
                let (directory, url) = manifest.get(file).ok_or_else(|| {
                    fail(format!(
                        "model file {file} ({id}.{name}) is not listed in the template manifest"
                    ))
                })?;
                models.push(ModelSlot {
                    node: id.clone(),
                    input: name.clone(),
                    name: file.to_owned(),
                    directory: directory.clone(),
                    url: url.clone(),
                });
                continue;
            }
            if let Some(choices) = spec.choices().filter(|_| !spec.options.multiselect)
                && !choices.contains(value)
            {
                return Err(fail(format!(
                    "unsupported value {value} for {}.{name}",
                    node.class
                )));
            }
            if let Some(number) = value.as_f64()
                && (spec.options.min.is_some_and(|min| number < min)
                    || spec.options.max.is_some_and(|max| number > max))
            {
                return Err(fail(format!("{}.{name} is out of range", node.class)));
            }
            if spec.control() && name.contains("seed") {
                seeds.push(Target {
                    node: id.clone(),
                    input: name.clone(),
                });
            }
        }
        classes.insert(node.class.clone());
        graph.insert(
            id.clone(),
            json!({"class_type": node.class, "inputs": node.inputs}),
        );
    }
    if seeds.is_empty() {
        return Err(fail("no seed input found".into()));
    }
    seeds.sort_by(|left, right| (&left.node, &left.input).cmp(&(&right.node, &right.input)));

    // The prompt is the literal STRING feeding the positive conditioning.
    let texts = |role: &[&str]| -> Result<BTreeSet<(String, String)>, CompileError> {
        let mut found = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut stack: Vec<String> = Vec::new();
        for node in nodes.values() {
            for (name, value) in &node.inputs {
                let conditioning = node
                    .specs
                    .get(name)
                    .is_some_and(|spec| spec.name() == "CONDITIONING");
                let guider = info.get(&node.class).is_some_and(|schema| {
                    schema
                        .output
                        .iter()
                        .any(|kind| kind.as_str() == Some("GUIDER"))
                });
                let starts = role.contains(&name.as_str()) && (name != "conditioning" || guider);
                if conditioning
                    && starts
                    && let Some((origin, _)) = link(value)
                {
                    stack.push(origin.to_owned());
                }
            }
        }
        while let Some(id) = stack.pop() {
            if !visited.insert(id.clone()) || visited.len() > MAX_NODES {
                continue;
            }
            let node = &nodes[&id];
            let text = ["text", "prompt"].into_iter().find(|name| {
                node.specs
                    .get(*name)
                    .is_some_and(|spec| spec.name() == "STRING")
            });
            if let Some(input) = text {
                let mut current = (id.clone(), input.to_owned());
                for _ in 0..32 {
                    let value = nodes[&current.0].inputs.get(&current.1);
                    match value.and_then(link) {
                        None if value.is_some_and(Value::is_string) => {
                            found.insert(current.clone());
                            break;
                        }
                        None => {
                            return Err(fail(format!(
                                "{}.{} has no prompt text",
                                current.0, current.1
                            )));
                        }
                        Some((origin, _)) => {
                            let upstream = &nodes[origin];
                            let candidates: Vec<&str> = TEXT_PATH
                                .into_iter()
                                .filter(|name| upstream.inputs.contains_key(*name))
                                .collect();
                            let [next] = candidates.as_slice() else {
                                return Err(fail(format!("prompt source {origin} is ambiguous")));
                            };
                            current = (origin.to_owned(), (*next).to_owned());
                        }
                    }
                }
                continue;
            }
            for (name, value) in &node.inputs {
                if node
                    .specs
                    .get(name)
                    .is_some_and(|spec| spec.name() == "CONDITIONING")
                    && let Some((origin, _)) = link(value)
                {
                    stack.push(origin.to_owned());
                }
            }
        }
        Ok(found)
    };
    let positive = texts(&["positive", "conditioning"])?;
    let negative = texts(&["negative"])?;
    let prompt = match positive.iter().collect::<Vec<_>>().as_slice() {
        [(node, input)] if !negative.contains(&(node.clone(), input.clone())) => Target {
            node: node.clone(),
            input: input.clone(),
        },
        _ => {
            return Err(fail(format!(
                "expected one positive prompt text distinct from the negative, found {}",
                positive.len()
            )));
        }
    };

    let save_node = &nodes[&save];
    let format = match save_node.class.as_str() {
        "SaveImage" => MediaFormat::Png,
        "SaveAnimatedWEBP" => MediaFormat::Webp,
        "SaveImageAdvanced" if save_node.inputs.get("format") == Some(&json!("png")) => {
            MediaFormat::Png
        }
        "SaveVideo"
            if matches!(
                save_node.inputs.get("format").and_then(Value::as_str),
                Some("auto" | "mp4")
            ) && matches!(
                save_node.inputs.get("format.codec").and_then(Value::as_str),
                Some("auto" | "h264") | None
            ) =>
        {
            MediaFormat::Mp4
        }
        _ => {
            return Err(fail(format!(
                "unsupported output format for {}",
                save_node.class
            )));
        }
    };
    models.sort_by(|left, right| (&left.node, &left.input).cmp(&(&right.node, &right.input)));
    Ok(Compiled {
        template: template.into(),
        graph,
        prompt,
        seeds,
        output: CompiledOutput {
            node: save.clone(),
            class: save_node.class.clone(),
            format,
        },
        models,
        classes,
    })
}
