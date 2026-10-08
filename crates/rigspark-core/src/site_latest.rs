//! Static last-month model data for the website's Ask Sparky panel.

use crate::{
    advice::decode_bytes,
    catalog::{Catalog, CatalogModel, PerfDataset, RecencyBasis, date},
    generation::GenerationCatalog,
    sizing::ValidationError,
};
use serde_json::{Value, json};

pub const WINDOW_DAYS: i64 = 31;
const GIB: f64 = 1_073_741_824.0;

fn basis(basis: RecencyBasis) -> &'static str {
    match basis {
        RecencyBasis::Released => "released",
        RecencyBasis::Added => "added",
    }
}

fn gib(bytes: f64) -> f64 {
    (bytes / GIB * 10.0).round() / 10.0
}

/// Text, image and video entries whose recency date is within 31 days of the text catalog's
/// `generatedAt`, newest first. Each item carries only what the site displays.
pub fn latest_items(
    text: &Catalog,
    generation: &GenerationCatalog,
) -> Result<Vec<Value>, ValidationError> {
    let reference = date(text.generated_at.get(..10).unwrap_or_default())?;
    let cutoff = reference - time::Duration::days(WINDOW_DAYS);
    let recent = |day: &str| date(day).is_ok_and(|day| day >= cutoff && day <= reference);
    let mut items = Vec::new();
    for model in &text.models {
        let Some((day, why)) = model.recency() else {
            continue;
        };
        if !recent(day) {
            continue;
        }
        let mut item = text_item(model)?;
        item["basis"] = json!(basis(why));
        item["date"] = json!(day);
        items.push(item);
    }
    for model in &generation.models {
        let Some((day, why)) = model.recency() else {
            continue;
        };
        if !recent(day) {
            continue;
        }
        items.push(json!({
            "id": model.id,
            "kind": model.kind.name(),
            "label": model.id,
            // Generation fit compares the largest file (stages swap) and all files together.
            "memGiB": gib(model.largest_bytes() as f64),
            "totalGiB": gib(model.total_bytes() as f64),
            "diskGiB": gib(model.total_bytes() as f64),
            "decodeBytes": null,
            "params": model.params,
            "dense": true,
            "basis": basis(why),
            "date": day,
            "runnable": model.workflow.is_some(),
        }));
    }
    items.sort_by(|left, right| {
        right["date"]
            .as_str()
            .cmp(&left["date"].as_str())
            .then_with(|| left["id"].as_str().cmp(&right["id"].as_str()))
    });
    Ok(items)
}

/// Display facts for one text model at the quantization `rigspark up` installs.
fn text_item(model: &CatalogModel) -> Result<Value, ValidationError> {
    let index = crate::registry_collector::pulled_quantization(model).unwrap_or(0);
    let quant = &model.quantizations[index];
    let mut item = json!({
        "id": model.id,
        "kind": "text",
        "label": format!("{} · {}", model.id, quant.name),
        "memGiB": gib(quant.min_ram_bytes),
        "diskGiB": gib(quant.disk_bytes),
        // Bytes read per token for the CLI's bandwidth estimate; null keeps speed `unknown`.
        "decodeBytes": decode_bytes(model, quant)?,
        "params": model.params,
        "dense": matches!(model.architecture, crate::sizing::Architecture::Dense),
        "runnable": true,
    });
    if let Some(active) = &model.active_params {
        item["activeParams"] = json!(active);
    }
    Ok(item)
}

/// Facts for the site's hand-picked models, in page order. Every id must be in the catalog.
pub fn popular_items(text: &Catalog, ids: &[String]) -> Result<Vec<Value>, ValidationError> {
    if ids.is_empty() {
        return Err(ValidationError("site popular models: none listed".into()));
    }
    ids.iter()
        .map(|id| {
            let model = text
                .models
                .iter()
                .find(|model| &model.id == id)
                .ok_or_else(|| {
                    ValidationError(format!("site popular model {id} is not in the catalog"))
                })?;
            text_item(model)
        })
        .collect()
}

const POPULAR_MARKER: &str = "data-popular=\"";

/// Model ids marked `data-popular="id"` in the site HTML, in order. Fails when none exist or a
/// marker is empty, unterminated, or not a plain model id.
pub fn popular_ids(html: &str) -> Result<Vec<String>, ValidationError> {
    let invalid = |reason: &str| ValidationError(format!("site popular marker: {reason}"));
    let mut ids = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find(POPULAR_MARKER) {
        let value = &rest[start + POPULAR_MARKER.len()..];
        let end = value.find('"').ok_or_else(|| invalid("unterminated"))?;
        let id = &value[..end];
        if id.is_empty()
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".:-_/".contains(&byte))
        {
            return Err(invalid("not a model id"));
        }
        ids.push(id.to_owned());
        rest = &value[end..];
    }
    if ids.is_empty() {
        return Err(invalid("none found"));
    }
    Ok(ids)
}

/// Sourced hardware classes with the default (Ollama) backend efficiency the CLI estimates with.
pub fn hardware_classes(perf: &PerfDataset) -> Vec<Value> {
    perf.classes
        .iter()
        .map(|class| {
            let efficiency = class
                .efficiency_by_backend
                .as_ref()
                .and_then(|values| values.get("ollama"))
                .copied()
                .unwrap_or(class.efficiency);
            json!({
                "id": class.id,
                "label": class.label,
                "vendor": class.vendor,
                "kind": class.kind,
                "bandwidthGBps": class.mem_bandwidth_gbps,
                "efficiency": efficiency,
                "minBytes": class.min_bytes,
                "maxBytes": class.max_bytes,
            })
        })
        .collect()
}

/// `site/data/latest.js`: inert, deterministic data loaded under `script-src 'self'`.
pub fn latest_script(
    text: &Catalog,
    generation: &GenerationCatalog,
    perf: &PerfDataset,
    popular: &[String],
) -> Result<String, ValidationError> {
    let data = json!({
        "generatedAt": text.generated_at,
        "windowDays": WINDOW_DAYS,
        "models": latest_items(text, generation)?,
        "popular": popular_items(text, popular)?,
        "hardware": hardware_classes(perf),
    });
    let encoded = serde_json::to_string(&data)
        .map_err(|error| ValidationError(error.to_string()))?
        .replace('<', "\\u003c");
    Ok(format!(
        "// Generated by `cargo catalog-site` from the shipped catalogs. Do not edit.\nglobalThis.RIGSPARK_LATEST = Object.freeze({encoded});\n"
    ))
}

const COUNT_MARKER: &str = "<span data-catalog-count=\"";

/// Rewrites every `<span data-catalog-count="text">N</span>` in the site HTML with the number of
/// text models in the catalog, so published counts cannot drift from what `recommend` ranks.
/// Fails when no marker exists, or a marker names an unknown kind or holds anything but digits.
pub fn stamp_catalog_counts(html: &str, text: &Catalog) -> Result<String, ValidationError> {
    let invalid = |reason: &str| ValidationError(format!("site catalog count: {reason}"));
    let total = text.models.len().to_string();
    let mut output = String::with_capacity(html.len());
    let mut rest = html;
    let mut markers = 0;
    while let Some(start) = rest.find(COUNT_MARKER) {
        let after_marker = &rest[start + COUNT_MARKER.len()..];
        let kind_end = after_marker
            .find("\">")
            .ok_or_else(|| invalid("unterminated marker"))?;
        if &after_marker[..kind_end] != "text" {
            return Err(invalid("unknown kind"));
        }
        let content = &after_marker[kind_end + 2..];
        let value_end = content
            .find("</span>")
            .ok_or_else(|| invalid("unclosed marker"))?;
        let value = &content[..value_end];
        if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid("marker must hold a number"));
        }
        let open_len = start + COUNT_MARKER.len() + kind_end + 2;
        output.push_str(&rest[..open_len]);
        output.push_str(&total);
        rest = &content[value_end..];
        markers += 1;
    }
    if markers == 0 {
        return Err(invalid("no data-catalog-count markers found"));
    }
    output.push_str(rest);
    Ok(output)
}
