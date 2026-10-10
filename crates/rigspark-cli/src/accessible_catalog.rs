use crate::accessible_text::{identifier, single_line};
use rigspark_core::{
    catalog::{Catalog, CatalogModel},
    ranking::backends,
    sizing::{Architecture, Hardware, SizingRequest, evaluate, memory_capacity},
};
use serde::{Deserialize, Serialize};
use std::io::{self, Write};
use tokio::sync::mpsc::Receiver;
use tokio_util::sync::CancellationToken;

const ROW_LIMIT: usize = 20;
const NESTED_LIMIT: usize = 10;
const COLLECTION_LIMIT: usize = 1000;
const NODE_LIMIT: usize = 100_000;
const TEXT_LIMIT: usize = 1024 * 1024;
pub const MAX_CATALOG_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_CATALOG_INPUT_BYTES: usize = 256;
const NOTICE: &str = "[output bounded; refine the search or inspect a numbered item for more]\n";
const HELP: &str = "Commands: /text search; number details; ? help; q quit\n";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRefresh {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    pub skipped: Vec<String>,
    pub capped: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CatalogOptions {
    pub all: bool,
    pub refresh: Option<CatalogRefresh>,
}

#[derive(Debug)]
struct CatalogRow {
    display: String,
    summary: String,
    evidence: String,
    search: String,
    need: f64,
}

#[derive(Debug)]
pub struct CatalogPresentation {
    machine: String,
    filter: &'static str,
    total: usize,
    rows: Vec<CatalogRow>,
    refresh: String,
    usable: f64,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn collection(length: usize) -> io::Result<()> {
    if length > COLLECTION_LIMIT {
        return Err(invalid("catalog collection exceeds 1000 entries"));
    }
    Ok(())
}

fn text(value: &str) -> io::Result<()> {
    if value.len() > TEXT_LIMIT {
        return Err(invalid("catalog text exceeds 1 MiB"));
    }
    Ok(())
}

fn number(value: f64) -> io::Result<()> {
    if !value.is_finite()
        || !(0.0..=9_007_199_254_740_991.0).contains(&value)
        || value.fract() != 0.0
    {
        return Err(invalid("catalog bytes must be nonnegative safe integers"));
    }
    Ok(())
}

fn validate(catalog: &Catalog, hardware: &Hardware, options: &CatalogOptions) -> io::Result<()> {
    collection(catalog.models.len())?;
    collection(hardware.gpu.len())?;
    for bytes in [
        hardware.total_ram_bytes,
        hardware.free_ram_bytes,
        hardware.free_disk_bytes,
    ] {
        number(bytes)?;
    }
    for gpu in &hardware.gpu {
        number(gpu.vram_bytes)?;
    }
    let mut nodes = 16 + hardware.gpu.len() * 3;
    for model in &catalog.models {
        collection(model.capabilities.len())?;
        collection(model.quantizations.len())?;
        if model.quantizations.is_empty() {
            return Err(invalid("missing catalog quantization"));
        }
        nodes += 32 + model.capabilities.len() + model.quantizations.len() * 8;
        for value in [&model.id, &model.family, &model.params, &model.license]
            .into_iter()
            .chain(model.release_date.iter())
            .chain(model.added_at.iter())
            .chain(model.active_params.iter())
            .chain(model.capabilities.iter())
            .chain(model.source.ollama.iter())
            .chain(model.source.hf.iter())
        {
            text(value)?;
        }
        if model
            .benchmark_proxy
            .is_some_and(|value| !value.is_finite())
        {
            return Err(invalid("invalid catalog benchmark"));
        }
        if let Some(source) = &model.source.gguf {
            for value in [&source.repo, &source.revision, &source.file, &source.sha256] {
                text(value)?;
            }
        }
        if let Some(source) = &model.source.mlx {
            collection(source.files.len())?;
            nodes += 4 + source.files.len() * 4;
            text(&source.repo)?;
            text(&source.revision)?;
            for file in &source.files {
                text(&file.file)?;
                text(&file.sha256)?;
                number(file.bytes)?;
            }
        }
        for quant in &model.quantizations {
            text(&quant.name)?;
            if let Some(digest) = &quant.sha256 {
                text(digest)?;
            }
        }
        if nodes > NODE_LIMIT {
            return Err(invalid("catalog input node limit exceeded"));
        }
    }
    if let Some(refresh) = &options.refresh {
        for values in [
            &refresh.added,
            &refresh.updated,
            &refresh.removed,
            &refresh.skipped,
            &refresh.capped,
        ] {
            collection(values.len())?;
            nodes += values.len() + 1;
            for value in values {
                text(value)?;
            }
        }
    }
    if nodes > NODE_LIMIT {
        return Err(invalid("catalog input node limit exceeded"));
    }
    Ok(())
}

fn gib(bytes: f64) -> String {
    let value = bytes / 1_073_741_824.0;
    let quarter_ticks = value * 4.0;
    let rounded = if quarter_ticks.fract() == 0.0 && quarter_ticks % 2.0 == 1.0 {
        (value * 10.0).round() / 10.0
    } else {
        value
    };
    format!("{rounded:.1} GiB")
}

fn label(value: &impl Serialize) -> io::Result<String> {
    let value = serde_json::to_value(value).map_err(io::Error::other)?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("invalid catalog label"))
}

fn omitted(values: &[String]) -> String {
    let visible = values
        .iter()
        .take(NESTED_LIMIT)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    let visible = if visible.is_empty() {
        "none".into()
    } else {
        visible
    };
    if values.len() > NESTED_LIMIT {
        format!("{visible} (+{} more)", values.len() - NESTED_LIMIT)
    } else {
        visible
    }
}

fn lines(values: &[String]) -> io::Result<Vec<String>> {
    values.iter().map(|value| single_line(value)).collect()
}

fn sources(model: &CatalogModel) -> io::Result<String> {
    let mut result = Vec::new();
    if let Some(source) = &model.source.ollama {
        result.push(format!("ollama {}", single_line(source)?));
    }
    if let Some(source) = &model.source.hf {
        result.push(format!("hf {}", single_line(source)?));
    }
    if let Some(source) = &model.source.gguf {
        result.push(format!(
            "gguf {}@{} {} sha256:{}",
            single_line(&source.repo)?,
            single_line(&source.revision)?,
            single_line(&source.file)?,
            single_line(&source.sha256)?
        ));
    }
    if let Some(source) = &model.source.mlx {
        let files = source
            .files
            .iter()
            .map(|file| {
                Ok(format!(
                    "{} sha256:{} {} bytes",
                    single_line(&file.file)?,
                    single_line(&file.sha256)?,
                    file.bytes
                ))
            })
            .collect::<io::Result<Vec<_>>>()?;
        result.push(format!(
            "mlx {}@{}; {}",
            single_line(&source.repo)?,
            single_line(&source.revision)?,
            omitted(&files)
        ));
    }
    Ok(omitted(&result))
}

fn actionable(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.split('/').any(|segment| segment == "..")
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._:/-".contains(&byte)
        })
}

pub fn build_catalog(
    catalog: &Catalog,
    hardware: &Hardware,
    options: &CatalogOptions,
) -> io::Result<CatalogPresentation> {
    validate(catalog, hardware, options)?;
    let mut models = catalog.models.iter().collect::<Vec<_>>();
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
        })
        .map_err(io::Error::other)?;
        if !options.all && !sizing.fit.fits {
            continue;
        }
        let smallest = model
            .quantizations
            .iter()
            .enumerate()
            .min_by(|(left, left_quant), (right, right_quant)| {
                sizing.required[*left]
                    .total_cmp(&sizing.required[*right])
                    .then_with(|| left_quant.name.cmp(&right_quant.name))
            })
            .ok_or_else(|| invalid("missing catalog quantization"))?;
        let quant = sizing.fit.quant.as_ref().unwrap_or(smallest.1);
        let required = sizing
            .fit
            .required_bytes
            .unwrap_or(sizing.required[smallest.0]);
        let display = identifier(&model.id)?;
        let family = single_line(&model.family)?;
        let architecture = match model.architecture {
            Architecture::Dense => "dense",
            Architecture::Moe => "moe",
            Architecture::Unknown => "unknown",
        };
        let fit = sizing.fit.reason.unwrap_or("fit");
        let release = single_line(&model.recency_label())?;
        let capabilities = lines(&model.capabilities)?;
        let quantizations = model
            .quantizations
            .iter()
            .map(|entry| {
                let digest = entry
                    .sha256
                    .as_deref()
                    .map(single_line)
                    .transpose()?
                    .unwrap_or_else(|| "unknown (not sourced)".into());
                let verified = if entry.digest_verified == Some(true) {
                    "verified"
                } else if entry.sha256.is_none() {
                    "size-only"
                } else {
                    "not verified"
                };
                Ok(format!(
                    "{} disk {} RAM {} VRAM {} SHA-256 {digest} Digest: {verified}",
                    single_line(&entry.name)?,
                    gib(entry.disk_bytes),
                    gib(entry.min_ram_bytes),
                    gib(entry.min_vram_bytes)
                ))
            })
            .collect::<io::Result<Vec<_>>>()?;
        let active = model
            .active_params
            .as_deref()
            .map(single_line)
            .transpose()?
            .map(|value| format!(", {value} active"))
            .unwrap_or_default();
        let kv = model
            .kv_bytes_per_token
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown (attention geometry not sourced)".into());
        let benchmark = model
            .benchmark_proxy
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown (not sourced)".into());
        let supported = backends(model, hardware)
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let evidence = format!(
            "{display}; family {family}; params {}{active}; architecture {architecture}; selected quant {}; need {}; fit {fit}; release {release}; license {}; open weight {}; context {}; KV bytes/token {kv}; benchmark {benchmark}; capabilities {}; backends {}; sources {}; quantizations {}",
            single_line(&model.params)?,
            single_line(&quant.name)?,
            gib(required),
            single_line(&model.license)?,
            if model.open_weight { "yes" } else { "no" },
            model
                .context_length
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".into()),
            omitted(&capabilities),
            omitted(&supported),
            sources(model)?,
            omitted(&quantizations)
        );
        let canonical = if actionable(&model.id) {
            model.id.as_str()
        } else {
            ""
        };
        let search = format!(
            "{canonical} {family} {} {architecture} {fit} {}",
            capabilities.join(" "),
            release.chars().take(4).collect::<String>()
        )
        .to_lowercase();
        rows.push(CatalogRow {
            display,
            summary: format!(
                "{} | {} | {fit} | {release}",
                single_line(&quant.name)?,
                gib(required)
            ),
            evidence,
            search,
            need: required,
        });
    }
    let (kind, usable) = memory_capacity(hardware);
    let gpus = hardware
        .gpu
        .iter()
        .map(|gpu| Ok(format!("{} {}", label(&gpu.vendor)?, gib(gpu.vram_bytes))))
        .collect::<io::Result<Vec<_>>>()?;
    let machine = format!(
        "{}/{}; RAM {} total / {} free; {} usable {kind}; disk {} free; GPUs {}",
        label(&hardware.platform)?,
        label(&hardware.arch)?,
        gib(hardware.total_ram_bytes),
        gib(hardware.free_ram_bytes),
        gib(usable),
        gib(hardware.free_disk_bytes),
        omitted(&gpus)
    );
    let refresh = match &options.refresh {
        None => "Not requested".into(),
        Some(refresh) => format!(
            "Dry-run diff: added {}; updated {}; removed {}; skipped {}; capped {}",
            omitted(&lines(&refresh.added)?),
            omitted(&lines(&refresh.updated)?),
            omitted(&lines(&refresh.removed)?),
            omitted(&lines(&refresh.skipped)?),
            omitted(&lines(&refresh.capped)?)
        ),
    };
    Ok(CatalogPresentation {
        machine,
        filter: if options.all { "all" } else { "fits" },
        total: catalog.models.len(),
        rows,
        refresh,
        usable,
    })
}

impl CatalogPresentation {
    pub fn usable_bytes(&self) -> f64 {
        self.usable
    }

    pub fn need_bytes(&self) -> impl Iterator<Item = f64> + '_ {
        self.rows.iter().map(|row| row.need)
    }

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
        vec![
            self.machine.clone(),
            format!(
                "Catalog: {}; {} of {} models",
                self.filter,
                self.rows.len(),
                self.total
            ),
            format!("Refresh: {}", self.refresh),
        ]
    }
}

fn bounded(document: &str) -> String {
    let mut output = String::new();
    for line in document.split_inclusive('\n') {
        if output.len() + line.len() > MAX_CATALOG_FRAME_BYTES - NOTICE.len() {
            output.push_str(NOTICE);
            break;
        }
        output.push_str(line);
    }
    output
}

fn row(row: &CatalogRow, index: usize) -> String {
    format!("{}. {}", index + 1, row.evidence)
}

pub fn catalog_screen(catalog: &CatalogPresentation) -> String {
    let rows = catalog
        .rows
        .iter()
        .take(ROW_LIMIT)
        .enumerate()
        .map(|(index, entry)| row(entry, index))
        .collect::<Vec<_>>()
        .join("\n");
    let rows = if rows.is_empty() {
        format!(
            "Empty: {}",
            if catalog.total == 0 {
                "catalog-empty"
            } else {
                "no-models-fit"
            }
        )
    } else {
        rows
    };
    let more = if catalog.rows.len() > ROW_LIMIT {
        format!(
            "\nShowing first {ROW_LIMIT} of {}; use /text to refine.",
            catalog.rows.len()
        )
    } else {
        String::new()
    };
    bounded(&format!(
        "rigspark / Catalog / Accessible\n1. Machine\n{}\n2. Catalog ({}; {}/{})\n{rows}{more}\n3. Refresh\n{}\n4. Controls\n{HELP}",
        catalog.machine,
        catalog.filter,
        catalog.rows.len(),
        catalog.total,
        catalog.refresh
    ))
}

fn filtered<'catalog>(
    catalog: &'catalog CatalogPresentation,
    query: &str,
) -> Vec<&'catalog CatalogRow> {
    let needle = query.to_lowercase();
    catalog
        .rows
        .iter()
        .filter(|row| row.search.contains(&needle))
        .collect()
}

pub async fn run_catalog<W: Write>(
    catalog: &CatalogPresentation,
    input: &mut Receiver<io::Result<String>>,
    output: &mut W,
    cancel: &CancellationToken,
) -> io::Result<()> {
    if cancel.is_cancelled() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "interactive input cancelled",
        ));
    }
    output.write_all(catalog_screen(catalog).as_bytes())?;
    output.flush()?;
    let mut query = String::new();
    loop {
        let raw = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(io::Error::new(io::ErrorKind::Interrupted, "interactive input cancelled")),
            answer = input.recv() => match answer { Some(answer) => answer?, None => return Ok(()) },
        };
        if raw.len() > MAX_CATALOG_INPUT_BYTES {
            return Err(invalid("answer exceeds 256 bytes"));
        }
        let safe = single_line(&raw)?;
        let command = safe.trim();
        let document = if command == "q" {
            return Ok(());
        } else if command == "?" {
            HELP.into()
        } else if let Some(search) = command.strip_prefix('/') {
            query = search.trim().into();
            let matches = filtered(catalog, &query);
            let rows = matches
                .iter()
                .take(ROW_LIMIT)
                .enumerate()
                .map(|(index, entry)| row(entry, index))
                .collect::<Vec<_>>()
                .join("\n");
            let more = if matches.len() > ROW_LIMIT {
                format!(
                    "\nShowing first {ROW_LIMIT} of {}; refine search for more.",
                    matches.len()
                )
            } else {
                String::new()
            };
            format!(
                "Filter: {}\n{}{more}\n",
                if query.is_empty() { "off" } else { &query },
                if rows.is_empty() { "No results" } else { &rows }
            )
        } else if (1..=3).contains(&command.len())
            && !command.starts_with('0')
            && command.bytes().all(|byte| byte.is_ascii_digit())
        {
            let index = command.parse::<usize>().map_err(io::Error::other)? - 1;
            match filtered(catalog, &query).get(index) {
                Some(entry) => format!("Details: {}\n{}\n", entry.display, row(entry, index)),
                None => "No such result.\n".into(),
            }
        } else {
            "Unknown command. Enter ? for help.\n".into()
        };
        output.write_all(bounded(&document).as_bytes())?;
        output.flush()?;
    }
}
