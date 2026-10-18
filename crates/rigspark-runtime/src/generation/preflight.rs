use super::{ComfyUi, GenerationError, ensure_local_only};
use rigspark_core::workflow::Marker;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tokio_util::sync::CancellationToken;

const MAX_NODE_BYTES: usize = 1024 * 1024;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn modern_combo_schema_requires_a_supported_single_choice() {
        let check = |spec: Value, value: Value, file| {
            accepts(
                spec.as_array().unwrap(),
                &value,
                file,
                &json!({}),
                &BTreeMap::new(),
            )
        };
        assert!(check(
            json!(["COMBO", {"options":["euler","heun"],"multiselect":false}]),
            json!("euler"),
            false
        ));
        assert!(check(
            json!(["COMBO", {"options":[]}]),
            json!("pending.safetensors"),
            true
        ));
        for spec in [
            json!(["COMBO", {"options":["heun"]}]),
            json!(["COMBO", {}]),
            json!(["COMBO", {"options":["euler",3]}]),
            json!(["COMBO", {"options":["euler"],"multiselect":true}]),
        ] {
            assert!(!check(spec, json!("euler"), false));
        }
    }
}

#[derive(Deserialize)]
struct NodeInfo {
    name: String,
    python_module: String,
    input: Inputs,
    output: Vec<String>,
    #[serde(default)]
    output_node: bool,
    #[serde(default)]
    api_node: bool,
}

#[derive(Deserialize)]
struct Inputs {
    #[serde(default)]
    required: BTreeMap<String, Vec<Value>>,
    #[serde(default)]
    optional: BTreeMap<String, Vec<Value>>,
}

#[derive(Default, Deserialize)]
struct Limits {
    min: Option<f64>,
    max: Option<f64>,
    options: Option<Vec<String>>,
    #[serde(default)]
    multiselect: bool,
}

fn incompatible(detail: impl Into<String>) -> GenerationError {
    GenerationError::Incompatible(format!(
        "This workflow needs compatible native ComfyUI nodes: {}. \
         Update ComfyUI and retry; no weights were downloaded.",
        detail.into()
    ))
}

// File inventories may be empty until installation; resolve_names checks them afterwards.
fn weight_input(class: &str, input: &str) -> bool {
    matches!(
        (class, input),
        ("UNETLoader", "unet_name") | ("CLIPLoader", "clip_name") | ("VAELoader", "vae_name")
    )
}

fn accepts(
    specification: &[Value],
    value: &Value,
    deferred_file: bool,
    graph: &Value,
    schemas: &BTreeMap<String, NodeInfo>,
) -> bool {
    if specification.is_empty() || specification.len() > 2 {
        return false;
    }
    let limits = match specification.get(1) {
        Some(options) => match serde_json::from_value::<Limits>(options.clone()) {
            Ok(limits) => limits,
            Err(_) => return false,
        },
        None => Limits::default(),
    };
    if let Some(choices) = specification[0].as_array() {
        return value.is_string()
            && choices.iter().all(Value::is_string)
            && (deferred_file || choices.contains(value));
    }
    if specification[0].as_str() == Some("COMBO") {
        return !limits.multiselect
            && limits.options.as_ref().is_some_and(|choices| {
                value.as_str().is_some_and(|value| {
                    deferred_file || choices.iter().any(|choice| choice == value)
                })
            });
    }
    if deferred_file {
        return false;
    }
    let Some(kind) = specification[0].as_str() else {
        return false;
    };
    if let Some(link) = value.as_array() {
        let Some((id, slot)) = link
            .first()
            .and_then(Value::as_str)
            .zip(link.get(1).and_then(Value::as_u64))
            .filter(|_| link.len() == 2)
        else {
            return false;
        };
        return graph[id]["class_type"]
            .as_str()
            .and_then(|class| schemas.get(class))
            .and_then(|schema| {
                usize::try_from(slot)
                    .ok()
                    .and_then(|slot| schema.output.get(slot))
            })
            .is_some_and(|output| output == kind);
    }
    let valid_type = match kind {
        "STRING" => value.is_string(),
        "INT" => value.as_i64().is_some(),
        "FLOAT" => value.as_f64().is_some(),
        "BOOLEAN" => value.is_boolean(),
        _ => false,
    };
    valid_type
        && value.as_f64().is_none_or(|number| {
            limits.min.is_none_or(|min| number >= min) && limits.max.is_none_or(|max| number <= max)
        })
}

/// Verifies the running ComfyUI implements every node the graph needs, before downloads.
pub(super) async fn check(
    client: &ComfyUi<'_>,
    endpoint: &str,
    graph: &Value,
    markers: &[Marker],
    cancel: &CancellationToken,
) -> Result<(), GenerationError> {
    ensure_local_only(graph)?;
    let nodes = graph
        .as_object()
        .ok_or_else(|| incompatible("invalid graph"))?;
    let mut classes: BTreeSet<&str> = markers.iter().map(|marker| marker.class.as_str()).collect();
    for node in nodes.values() {
        let class = node["class_type"]
            .as_str()
            .ok_or_else(|| incompatible("invalid node class"))?;
        classes.insert(class);
    }
    let mut schemas = BTreeMap::new();
    for class in classes {
        let raw = match client
            .get_json(
                endpoint,
                &format!("/object_info/{class}"),
                cancel,
                MAX_NODE_BYTES,
            )
            .await
        {
            Ok(raw) => raw,
            Err(
                GenerationError::Http(super::HttpError::Cancelled) | GenerationError::Cancelled,
            ) => {
                return Err(GenerationError::Cancelled);
            }
            Err(error) => return Err(incompatible(format!("cannot inspect {class}: {error}"))),
        };
        let info: NodeInfo = serde_json::from_value(raw[class].clone())
            .map_err(|_| incompatible(format!("missing or malformed {class}")))?;
        if info.name != class
            || info.api_node
            || !(info.python_module == "nodes" || info.python_module.starts_with("comfy_extras."))
        {
            return Err(incompatible(format!("{class} is not a local core node")));
        }
        schemas.insert(class.to_owned(), info);
    }
    for marker in markers {
        let schema = &schemas[&marker.class];
        if schema.python_module != marker.module || schema.output.first() != Some(&marker.output) {
            return Err(incompatible(format!(
                "native {} support is missing",
                marker.class
            )));
        }
    }
    for node in nodes.values() {
        let class = node["class_type"]
            .as_str()
            .ok_or_else(|| incompatible("invalid node"))?;
        let inputs = node["inputs"]
            .as_object()
            .ok_or_else(|| incompatible("invalid inputs"))?;
        let schema = &schemas[class];
        if schema
            .input
            .required
            .keys()
            .any(|key| !inputs.contains_key(key))
            || (class == "SaveImage" && !schema.output_node)
        {
            return Err(incompatible(format!("{class} contract changed")));
        }
        for (input, value) in inputs {
            let specification = schema
                .input
                .required
                .get(input)
                .or_else(|| schema.input.optional.get(input));
            if !specification.is_some_and(|specification| {
                accepts(
                    specification,
                    value,
                    weight_input(class, input),
                    graph,
                    &schemas,
                )
            }) {
                return Err(incompatible(format!("unsupported {class}.{input}")));
            }
        }
    }
    Ok(())
}
