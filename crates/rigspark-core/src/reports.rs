use crate::{
    advice::verdict,
    catalog::{Catalog, PerfDataset, resolve},
    ranking::{AdviceOptions, backends},
    sizing::{Hardware, KvCacheType, SizingRequest, ValidationError, evaluate},
};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

pub fn strip_control(text: &str) -> String {
    static ANSI: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b[@-Z\\-_]").expect("constant ANSI pattern")
    });
    ANSI.replace_all(text, "").chars().filter(|ch| !ch.is_control() && !matches!(*ch, '\u{ad}' | '\u{61c}' | '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{2028}' | '\u{2029}' | '\u{feff}')).collect()
}
pub fn sanitized(value: &Value) -> Value {
    match value {
        Value::String(text) => json!(strip_control(text)),
        Value::Array(items) => Value::Array(items.iter().map(sanitized).collect()),
        Value::Object(items) => Value::Object(
            items
                .iter()
                .map(|(key, value)| (strip_control(key), sanitized(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}
fn text(value: &Value) -> String {
    value.as_str().map(str::to_owned).unwrap_or_else(|| {
        value
            .as_f64()
            .map_or_else(|| "unknown".into(), |number| number.to_string())
    })
}
pub fn gib(bytes: f64) -> String {
    format!("{:.1} GiB", bytes / 1073741824.0)
}
fn grouped(value: &Value) -> String {
    if value.is_null() {
        return "unknown".into();
    }
    let raw = text(value);
    let mut out = String::new();
    for (index, ch) in raw.chars().enumerate() {
        if index > 0 && (raw.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}
pub fn table(columns: &[(&str, bool)], rows: Vec<Vec<String>>) -> String {
    let headers: Vec<String> = columns
        .iter()
        .map(|(name, _)| strip_control(name))
        .collect();
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            (0..columns.len())
                .map(|index| strip_control(row.get(index).map_or("", String::as_str)))
                .collect()
        })
        .collect();
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(index, header)| {
            rows.iter()
                .map(|row| row[index].encode_utf16().count())
                .chain([header.encode_utf16().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    std::iter::once(&headers)
        .chain(rows.iter())
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(index, cell)| {
                    let padding = " ".repeat(widths[index] - cell.encode_utf16().count());
                    if columns[index].1 {
                        format!("{padding}{cell}")
                    } else {
                        format!("{cell}{padding}")
                    }
                })
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end_matches(' ')
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn safe_recommendation_command(report: &Value) -> Option<&str> {
    let id = report["ranked"].as_array()?.first()?["id"].as_str()?;
    let command = report["command"].as_str()?;
    (!id.is_empty()
        && !id.starts_with('-')
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._:/-".contains(&byte)
        })
        && !id.split('/').any(|part| part == "..")
        && command.len() <= 256
        && command.strip_prefix("rigspark up ") == Some(id))
    .then_some(command)
}

pub fn recommendation_text(report: &Value, options: &AdviceOptions) -> String {
    let ranked = report["ranked"].as_array().cloned().unwrap_or_default();
    let misses = report["wontFit"].as_array().cloned().unwrap_or_default();
    if ranked.is_empty() && misses.is_empty() {
        return "No models in the catalog.".into();
    }
    let hw = &report["hardware"];
    let kv = KvCacheType::label(options.kv_cache);
    let note = if let Some(context) = options.context {
        format!(" — sized at {context}-token context (KV {kv})")
    } else if options.max_context {
        format!(" — largest holdable context per model (KV {kv})")
    } else {
        String::new()
    };
    let mut sections = vec![format!(
        "Ranked local LLMs for {}/{} ({} {} usable):{note}",
        text(&hw["arch"]),
        text(&hw["platform"]),
        gib(hw["usableMemoryBytes"].as_f64().unwrap_or(0.0)),
        text(&hw["memoryKind"])
    )];
    if ranked.is_empty() {
        sections.push("No models fit this hardware.".into());
    } else {
        let mut columns = vec![
            ("Rank", true),
            ("Model", false),
            ("Params", true),
            ("Quant", false),
        ];
        if options.context.is_some() {
            columns.extend([("Weights", true), ("KV Cache", true), ("Est. Mem", true)]);
        } else if options.max_context {
            columns.extend([
                ("Est. Mem", true),
                ("Max Context", true),
                ("Bound-By", false),
            ]);
        } else {
            columns.push(("Est. Mem", true));
        }
        columns.extend([
            ("Verdict", false),
            ("Est. tok/s", true),
            ("Backends", false),
            ("License", false),
            ("Score", true),
        ]);
        let rows = ranked
            .iter()
            .map(|entry| {
                let mut row = vec![
                    text(&entry["rank"]),
                    text(&entry["id"]),
                    text(&entry["params"]),
                    text(&entry["quant"]),
                ];
                if options.context.is_some() {
                    row.push(gib(entry["weightsBytes"].as_f64().unwrap_or(0.0)));
                    row.push(entry["kvCacheBytes"].as_f64().map_or("unknown".into(), gib));
                }
                row.push(gib(entry["requiredBytes"].as_f64().unwrap_or(0.0)));
                if options.max_context {
                    row.push(grouped(&entry["maxContextTokens"]));
                    row.push(text(&entry["boundBy"]));
                }
                let verdict = text(&entry["verdict"]);
                row.push(format!(
                    "{} {verdict}",
                    if verdict == "yes" { "✓" } else { "⚠️" }
                ));
                row.push(if entry["estTokPerSec"].is_null() {
                    "unknown".into()
                } else {
                    format!(
                        "{}–{}",
                        text(&entry["estTokPerSec"]["lowTokPerSec"]),
                        text(&entry["estTokPerSec"]["highTokPerSec"])
                    )
                });
                let backends = entry["backends"]
                    .as_array()
                    .map(|items| items.iter().map(text).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default();
                row.extend([
                    if backends.is_empty() {
                        "—".into()
                    } else {
                        backends
                    },
                    text(&entry["license"]),
                    format!("{:.2}", entry["score"].as_f64().unwrap_or(0.0)),
                ]);
                row
            })
            .collect();
        sections.push(table(&columns, rows));
        if let Some(command) = safe_recommendation_command(report) {
            sections.push(format!("Run the top pick:  {command}"));
        }
    }
    if !misses.is_empty() {
        sections.push(format!(
            "Won't fit ({}):\n{}",
            misses.len(),
            misses
                .iter()
                .map(|entry| format!(
                    "  ❌ {}  ({})",
                    strip_control(&text(&entry["id"])),
                    text(&entry["reason"])
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    sections.join("\n\n")
}

pub fn can_run(
    catalog: &Catalog,
    hardware: &Hardware,
    perf: &PerfDataset,
    query: &str,
    options: &AdviceOptions,
) -> Result<(Value, String), ValidationError> {
    let report = can_run_report(catalog, hardware, perf, query, options)?;
    Ok((report.json, report.text))
}

pub struct CanRunReport {
    pub json: Value,
    pub text: String,
    pub evidence: Value,
}

pub fn can_run_report(
    catalog: &Catalog,
    hardware: &Hardware,
    perf: &PerfDataset,
    query: &str,
    options: &AdviceOptions,
) -> Result<CanRunReport, ValidationError> {
    options.validate()?;
    let resolved = resolve(catalog, query).map_err(|error| ValidationError(error.message))?;
    let mut model = resolved.model.clone();
    if let Some(quant) = resolved.quant {
        model.quantizations = vec![quant.clone()];
    }
    if let Some(kind) = options.kv_cache {
        model = model.with_kv_cache(kind)?;
    }
    let backend = options.backend.as_deref().unwrap_or("ollama");
    let report = verdict(&model, hardware, perf, options.context, backend)?;
    let supported = backends(&model, hardware);
    let mut output = json!({"model":model.id,"verdict":report["runnable"],"quant":report["quant"]["name"],"reason":report["reason"],"throughput":report["throughput"],"backends":supported,"throughputBackend":backend});
    let auto_source = (model.provenance == crate::catalog::EntryProvenance::Auto).then(|| {
        format!(
            "Source: auto-sourced from the Ollama library ({})",
            model.recency_label()
        )
    });
    if auto_source.is_some() {
        output["provenance"] = json!("auto");
    }
    let mut lines = Vec::new();
    if let Some(context) = options.context {
        output["context"] = json!(context);
        output["contextFitKnown"] = json!(model.kv_bytes_per_token.is_some());
        output["requiredBytes"] = report["requiredBytes"].clone();
        output["usableBytes"] = report["usableBytes"].clone();
        if options.kv_cache.is_some() {
            output["kvPrecision"] = json!(KvCacheType::label(options.kv_cache));
        }
        lines.push(format!(
            "Context: {context} tokens (KV {} estimate)",
            KvCacheType::label(options.kv_cache)
        ));
        if model.kv_bytes_per_token.is_none() {
            lines.push("Requested context fit unknown: attention geometry unavailable; verdict below is based on weights only.".into());
        }
    }
    let id = strip_control(&model.id);
    let runnable = text(&report["runnable"]);
    if runnable == "no" {
        let reason = text(&report["reason"]);
        let label = match reason.as_str() {
            "ram-bound" => "not enough RAM",
            "disk-bound" => "not enough free disk",
            "context-bound" => "context exceeds the model's limit",
            _ => "not enough VRAM",
        };
        lines.push(format!("❌ {id}: no — does not fit ({reason}: {label})"));
    } else {
        let known = report["throughput"]["known"] == true;
        let low = report["throughput"]["lowTokPerSec"].as_f64().unwrap_or(0.0);
        let high = report["throughput"]["highTokPerSec"]
            .as_f64()
            .unwrap_or(0.0);
        let detail = if runnable == "yes" {
            "runs comfortably".into()
        } else if known {
            format!(
                "fits, but ~{} tok/s is below the comfort floor",
                (((low + high) / 2.0) * 10.0).round() / 10.0
            )
        } else {
            "fits, but throughput can't be estimated for this hardware".into()
        };
        lines.push(format!(
            "{} {id}: {runnable} — {detail}",
            if runnable == "yes" { "✓" } else { "⚠️" }
        ));
        lines.push(format!(
            "Quant: {}",
            strip_control(&text(&report["quant"]["name"]))
        ));
        lines.push(if known {
            format!("Estimated throughput: {low}–{high} tok/s")
        } else {
            "Estimated throughput: unknown (no performance profile for this hardware)".into()
        });
        lines.push(format!(
            "Backends: {} (throughput scoped to {backend})",
            if supported.is_empty() {
                "none".into()
            } else {
                supported.join(", ")
            }
        ));
    }
    if let Some(line) = auto_source {
        lines.push(line);
    }
    let unknown_reason = if report["throughput"]["known"] == true {
        Value::Null
    } else if report["runnable"] == "no" {
        json!("not-evaluated-model-does-not-fit")
    } else {
        json!("no-sourced-performance-profile")
    };
    let mut evidence = json!({"modelId":model.id,"runnable":report["runnable"],"quant":report["quant"]["name"],"reason":report["reason"],"throughput":report["throughput"],"backends":supported,"throughputBackend":backend,"requiredBytes":report["requiredBytes"],"usableBytes":report["usableBytes"],"throughputEvidence":{"source":"offline-estimate","unknownReason":unknown_reason}});
    if let Some(context) = options.context {
        evidence["context"] = json!(context);
        evidence["contextFitKnown"] = json!(model.kv_bytes_per_token.is_some());
    }
    Ok(CanRunReport {
        json: sanitized(&output),
        text: lines.join("\n"),
        evidence,
    })
}

pub fn catalog_text(
    catalog: &Catalog,
    hardware: &Hardware,
    all: bool,
) -> Result<String, ValidationError> {
    let mut models: Vec<_> = catalog.models.iter().collect();
    models.sort_by(|left, right| {
        right
            .recency()
            .map(|(day, _)| day)
            .cmp(&left.recency().map(|(day, _)| day))
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut rows = Vec::new();
    for model in models {
        let sizing = evaluate(&SizingRequest {
            model: model.sizing(),
            hardware: hardware.clone(),
            context: None,
        })?;
        if !all && !sizing.fit.fits {
            continue;
        }
        let smallest = model
            .quantizations
            .iter()
            .enumerate()
            .min_by(|(left, lq), (right, rq)| {
                sizing.required[*left]
                    .total_cmp(&sizing.required[*right])
                    .then_with(|| lq.name.cmp(&rq.name))
            })
            .map(|(_, quant)| quant)
            .ok_or_else(|| ValidationError("missing catalog quant".into()))?;
        let quant = sizing.fit.quant.as_ref().unwrap_or(smallest);
        rows.push(vec![
            model.id.clone(),
            model.params.clone(),
            if matches!(model.architecture, crate::sizing::Architecture::Moe) {
                "moe".into()
            } else {
                "dense".into()
            },
            quant.name.clone(),
            format!(
                "{:.1}",
                sizing.fit.required_bytes.unwrap_or(0.0) / 1073741824.0
            ),
            sizing.fit.reason.unwrap_or("fit").into(),
            model.recency_label(),
        ]);
    }
    let header = format!(
        "Catalog (Filter: {}, shown: {}/{})",
        if all { "all" } else { "fits" },
        rows.len(),
        catalog.models.len()
    );
    if rows.is_empty() {
        return Ok(format!(
            "{header}\nNo models fit this hardware. Re-run with --all to see the full catalog.\n"
        ));
    }
    Ok(format!(
        "{header}\n{}\n",
        table(
            &[
                ("Model", false),
                ("Params", false),
                ("Arch", false),
                ("Quant", false),
                ("Need GiB", true),
                ("Fit", false),
                ("Release", false)
            ],
            rows
        )
    ))
}
