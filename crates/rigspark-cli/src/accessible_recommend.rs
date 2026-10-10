use crate::accessible_text::{identifier, single_line};
use rigspark_core::{
    catalog::{Catalog, PerfDataset},
    ranking::{AdviceOptions, recommend_detailed},
    reports::{recommendation_text, safe_recommendation_command},
    sizing::{Hardware, memory_capacity},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    io::{self, Write},
};
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;

pub const MAX_ROWS: usize = 20;
pub const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
const MAX_ITEMS: usize = 1_000;
const BOUNDED_NOTICE: &str =
    "[output bounded; refine the search or inspect a numbered item for more]\n";

#[derive(Debug, PartialEq, Eq)]
pub enum RecommendOutcome {
    Exited,
    Cancelled,
    PrintCommand { command: String },
}

#[derive(Debug)]
struct Row {
    display: String,
    summary: String,
    search: String,
    evidence: String,
    need: f64,
}

#[derive(Debug)]
pub struct Recommendation {
    machine: String,
    scope: String,
    rows: Vec<Row>,
    wont_fit: Vec<String>,
    command: Option<String>,
    final_text: String,
    usable: f64,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn text(value: &Value) -> io::Result<&str> {
    value
        .as_str()
        .ok_or_else(|| invalid("missing recommendation text evidence"))
}

fn number(value: &Value) -> io::Result<f64> {
    value
        .as_f64()
        .filter(|number| number.is_finite())
        .ok_or_else(|| invalid("missing recommendation numeric evidence"))
}

fn legacy_fixed(value: f64, precision: usize) -> String {
    let binary_ticks = value * 2.0_f64.powi(precision as i32 + 1);
    let rounded = if binary_ticks.fract() == 0.0 && binary_ticks % 2.0 == 1.0 {
        let scale = 10.0_f64.powi(precision as i32);
        (value * scale).round() / scale
    } else {
        value
    };
    format!("{rounded:.precision$}")
}

fn gib(bytes: f64) -> String {
    format!("{} GiB", legacy_fixed(bytes / 1_073_741_824.0, 1))
}

fn list(values: &[String]) -> String {
    if values.is_empty() {
        "none".into()
    } else {
        values.join(", ")
    }
}

fn safe_id(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._:/-".contains(&byte)
        })
        && !value.split('/').any(|part| part == "..")
}

fn bounded_document(value: String) -> String {
    let budget = MAX_DOCUMENT_BYTES - BOUNDED_NOTICE.len();
    let mut kept = String::new();
    for line in value.split_inclusive('\n') {
        if kept.len() + line.len() > budget {
            kept.push_str(BOUNDED_NOTICE);
            return kept;
        }
        kept.push_str(line);
    }
    kept
}

fn check_inputs(catalog: &Catalog, hardware: &Hardware) -> io::Result<()> {
    if catalog.models.len() > MAX_ITEMS || hardware.gpu.len() > 128 {
        return Err(invalid("recommendation input collection limit exceeded"));
    }
    let mut nodes = 0usize;
    for model in &catalog.models {
        if model.capabilities.len() > MAX_ITEMS || model.quantizations.len() > MAX_ITEMS {
            return Err(invalid("recommendation input collection limit exceeded"));
        }
        nodes += 32 + model.capabilities.len() + model.quantizations.len() * 8;
        if nodes > 100_000 {
            return Err(invalid("recommendation input node limit exceeded"));
        }
    }
    for bytes in [
        hardware.total_ram_bytes,
        hardware.free_ram_bytes,
        hardware.free_disk_bytes,
    ]
    .into_iter()
    .chain(hardware.gpu.iter().map(|gpu| gpu.vram_bytes))
    {
        if !bytes.is_finite()
            || !(0.0..=9_007_199_254_740_991.0).contains(&bytes)
            || bytes.fract() != 0.0
        {
            return Err(invalid("invalid hardware byte count"));
        }
    }
    Ok(())
}

pub fn build_recommendation(
    catalog: &Catalog,
    hardware: &Hardware,
    perf: &PerfDataset,
    options: &AdviceOptions,
) -> io::Result<Recommendation> {
    check_inputs(catalog, hardware)?;
    let report = recommend_detailed(catalog, hardware, perf, options).map_err(io::Error::other)?;
    let hardware_json = serde_json::to_value(hardware).map_err(io::Error::other)?;
    let (kind, usable) = memory_capacity(hardware);
    let gpus = hardware_json["gpu"]
        .as_array()
        .ok_or_else(|| invalid("missing GPU evidence"))?
        .iter()
        .map(|gpu| {
            Ok(format!(
                "{} {}",
                text(&gpu["vendor"])?,
                gib(number(&gpu["vramBytes"])?)
            ))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let machine = format!(
        "{}/{}; {} usable {kind}; RAM {} total, {} free; disk {} free; GPUs {}",
        text(&hardware_json["platform"])?,
        text(&hardware_json["arch"])?,
        gib(usable),
        gib(hardware.total_ram_bytes),
        gib(hardware.free_ram_bytes),
        gib(hardware.free_disk_bytes),
        list(&gpus)
    );
    let context = options.context.map_or_else(
        || {
            if options.max_context {
                "maximum".into()
            } else {
                "default".into()
            }
        },
        |tokens| tokens.to_string(),
    );
    let scope = format!(
        "Scope: task {}; context {context}; backend {}; {}",
        single_line(options.task.as_deref().unwrap_or("any"))?,
        single_line(options.backend.as_deref().unwrap_or("ollama"))?,
        if options.available_backends.is_some() {
            "installed backends only"
        } else {
            "all compatible backends"
        }
    );
    let models: HashMap<_, _> = catalog
        .models
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect();
    if models.len() != catalog.models.len() {
        return Err(invalid("duplicate recommendation model id"));
    }
    let ranked = report["ranked"]
        .as_array()
        .ok_or_else(|| invalid("missing ranked models"))?;
    let mut rows = Vec::with_capacity(ranked.len());
    for entry in ranked {
        let id = text(&entry["id"])?;
        let model = models
            .get(id)
            .ok_or_else(|| invalid("ranked model missing from catalog"))?;
        let capabilities = model
            .capabilities
            .iter()
            .map(|value| single_line(value))
            .collect::<io::Result<Vec<_>>>()?;
        let backends = entry["backends"]
            .as_array()
            .ok_or_else(|| invalid("missing backend evidence"))?
            .iter()
            .map(|value| single_line(text(value)?))
            .collect::<io::Result<Vec<_>>>()?;
        let provenance = &entry["throughputEvidence"];
        let estimate = &entry["estTokPerSec"];
        let unknown = provenance["unknownReason"].as_str();
        let label = if estimate.is_null() {
            "unknown".into()
        } else {
            format!(
                "{}\u{2013}{} tok/s",
                number(&estimate["lowTokPerSec"])?,
                number(&estimate["highTokPerSec"])?
            )
        };
        let unknown_label =
            unknown.map_or(String::new(), |reason| format!("; unknown reason {reason}"));
        let source = single_line(&format!(
            "{}{}",
            text(&provenance["source"])?,
            unknown.map_or(String::new(), |reason| format!("; {reason}"))
        ))?;
        let context_evidence = if let Some(tokens) = options.tokens(model) {
            format!(
                "{tokens} tokens; weights {} bytes; {}",
                number(&entry["weightsBytes"])?,
                if entry["kvCacheBytes"].is_null() {
                    "KV cache unknown".into()
                } else {
                    format!("KV cache {} bytes", number(&entry["kvCacheBytes"])?)
                }
            )
        } else if options.max_context {
            if entry["maxContextTokens"].is_null() {
                "maximum context unknown".into()
            } else {
                format!(
                    "maximum context {} tokens; bound by {}",
                    number(&entry["maxContextTokens"])?,
                    text(&entry["boundBy"])?
                )
            }
        } else {
            "default context footprint".into()
        };
        let scores = &entry["scores"];
        let display = identifier(id)?;
        let verdict = text(&entry["verdict"])?;
        let evidence = format!(
            "{display}; rank {}; {}; {}; {}; verdict {verdict}; throughput {}{unknown_label}; backends {}; score {}; capabilities {}; license {}; context {}; {}; throughput backend {}; source {source}; scores quality {}, fit {}, speed {}, recency {}, capability {}",
            number(&entry["rank"])?,
            single_line(&model.params)?,
            single_line(text(&entry["quant"])?)?,
            gib(number(&entry["requiredBytes"])?),
            single_line(&label)?,
            list(&backends),
            legacy_fixed(number(&entry["score"])?, 2),
            list(&capabilities),
            single_line(&model.license)?,
            model
                .context_length
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".into()),
            single_line(&context_evidence)?,
            single_line(text(&provenance["backend"])?)?,
            legacy_fixed(number(&scores["quality"])?, 2),
            legacy_fixed(number(&scores["fit"])?, 2),
            legacy_fixed(number(&scores["speed"])?, 2),
            legacy_fixed(number(&scores["recency"])?, 2),
            legacy_fixed(number(&scores["capability"])?, 2)
        );
        let search = format!(
            "{} {} {} {verdict}",
            if safe_id(id) { id } else { "" },
            capabilities.join(" "),
            backends.join(" ")
        )
        .to_lowercase();
        rows.push(Row {
            need: number(&entry["requiredBytes"])?,
            display,
            summary: format!(
                "#{} | {} | {} | {verdict} | {} | {}",
                number(&entry["rank"])?,
                single_line(text(&entry["quant"])?)?,
                gib(number(&entry["requiredBytes"])?),
                single_line(&label)?,
                legacy_fixed(number(&entry["score"])?, 2),
            ),
            search,
            evidence,
        });
    }
    let wont_fit = report["wontFit"]
        .as_array()
        .ok_or_else(|| invalid("missing non-fitting models"))?
        .iter()
        .map(|entry| {
            Ok(format!(
                "{}; {}",
                identifier(text(&entry["id"])?)?,
                single_line(text(&entry["reason"])?)?
            ))
        })
        .collect::<io::Result<Vec<_>>>()?;
    let command = safe_recommendation_command(&report).map(str::to_owned);
    let final_text = recommendation_text(&report, options);
    Ok(Recommendation {
        machine,
        scope,
        rows,
        wont_fit,
        command,
        final_text,
        usable,
    })
}

impl Recommendation {
    pub fn usable_bytes(&self) -> f64 {
        self.usable
    }

    pub fn need_bytes(&self) -> impl Iterator<Item = f64> + '_ {
        self.rows.iter().map(|row| row.need)
    }
}

impl Recommendation {
    pub fn visual_rows(&self) -> impl ExactSizeIterator<Item = (&str, &str, &str, &str)> {
        self.rows.iter().map(|row| {
            (
                row.display.as_str(),
                row.summary.as_str(),
                row.search.as_str(),
                row.evidence.as_str(),
            )
        })
    }

    pub fn visual_overview(&self) -> Vec<String> {
        let mut lines = vec![
            self.machine.clone(),
            self.scope.clone(),
            format!("Won't fit: {}", self.wont_fit.len()),
        ];
        lines.extend(self.wont_fit.iter().cloned());
        lines
    }

    pub fn print_command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    pub fn final_text(&self) -> &str {
        &self.final_text
    }

    fn help(&self) -> String {
        format!(
            "Commands: /text search; number details; ? help;{} q quit\n",
            if self.command.is_some() {
                " p finish and print result;"
            } else {
                ""
            }
        )
    }

    pub fn format(&self) -> String {
        let mut output = format!(
            "rigspark / Recommend / Accessible\n1. Machine\n{}\n{}\n2. Ranked models\n",
            self.machine, self.scope
        );
        for (index, row) in self.rows.iter().take(MAX_ROWS).enumerate() {
            output.push_str(&format!("{}. {}\n", index + 1, row.evidence));
        }
        if self.rows.len() > MAX_ROWS {
            output.push_str(&format!(
                "Showing first {MAX_ROWS} of {}; use /text to refine.\n",
                self.rows.len()
            ));
        }
        output.push_str("3. Won't fit\n");
        if self.wont_fit.is_empty() {
            output.push_str("None\n");
        }
        for (index, row) in self.wont_fit.iter().take(MAX_ROWS).enumerate() {
            output.push_str(&format!("{}. {row}\n", index + 1));
        }
        if self.wont_fit.len() > MAX_ROWS {
            output.push_str(&format!(
                "Showing first {MAX_ROWS} of {}; use catalog --all to inspect the rest.\n",
                self.wont_fit.len()
            ));
        }
        output.push_str("4. Controls\n");
        output.push_str(&self.help());
        bounded_document(output)
    }

    fn filtered(&self, query: &str) -> Vec<&Row> {
        let needle = query.to_lowercase();
        self.rows
            .iter()
            .filter(|row| row.search.contains(&needle))
            .collect()
    }

    fn search(&self, query: &str) -> String {
        let rows = self.filtered(query);
        let mut output = format!("Filter: {}\n", if query.is_empty() { "off" } else { query });
        if rows.is_empty() {
            output.push_str("No results\n");
        }
        for (index, row) in rows.iter().take(MAX_ROWS).enumerate() {
            output.push_str(&format!("{}. {}\n", index + 1, row.evidence));
        }
        if rows.len() > MAX_ROWS {
            output.push_str(&format!(
                "Showing first {MAX_ROWS} of {}; refine search for more.\n",
                rows.len()
            ));
        }
        bounded_document(output)
    }

    fn details(&self, query: &str, index: usize) -> String {
        self.filtered(query).get(index).map_or_else(
            || "No such result.\n".into(),
            |row| {
                bounded_document(format!(
                    "Details: {}\n{}. {}\n",
                    row.display,
                    index + 1,
                    row.evidence
                ))
            },
        )
    }
}

fn write_frame(output: &mut impl Write, text: &str) -> io::Result<()> {
    output.write_all(text.as_bytes())?;
    output.flush()
}

pub async fn run_recommendation(
    recommendation: &Recommendation,
    input: &mut Receiver<io::Result<String>>,
    output: &mut impl Write,
    cancellation: &CancellationToken,
) -> io::Result<RecommendOutcome> {
    if cancellation.is_cancelled() {
        return Ok(RecommendOutcome::Cancelled);
    }
    write_frame(output, &recommendation.format())?;
    let mut query = String::new();
    loop {
        let raw = tokio::select! {
            biased;
            () = cancellation.cancelled() => return Ok(RecommendOutcome::Cancelled),
            line = input.recv() => match line {
                None => return Ok(RecommendOutcome::Exited),
                Some(line) => line?,
            }
        };
        let bounded = single_line(&raw)?;
        let line = bounded.trim();
        match line {
            "q" => return Ok(RecommendOutcome::Exited),
            "?" => write_frame(output, &recommendation.help())?,
            "p" if recommendation.command.is_some() => {
                return Ok(RecommendOutcome::PrintCommand {
                    command: recommendation
                        .command
                        .clone()
                        .ok_or_else(|| invalid("missing print command"))?,
                });
            }
            line if line.starts_with('/') => {
                query = line[1..].trim().into();
                write_frame(output, &recommendation.search(&query))?;
            }
            line if !line.is_empty()
                && line.len() <= 3
                && !line.starts_with('0')
                && line.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                let index = line
                    .parse::<usize>()
                    .map_err(|_| invalid("invalid result number"))?
                    - 1;
                write_frame(output, &recommendation.details(&query, index))?;
            }
            _ => write_frame(output, "Unknown command. Enter ? for help.\n")?,
        }
        tokio::task::yield_now().await;
    }
}
