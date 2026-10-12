use crate::{
    advice::{Throughput, throughput},
    catalog::{CatalogModel, PerfDataset},
    sizing::{
        GpuVendor, HEADROOM, Hardware, KvCacheType, SizingRequest, ValidationError, evaluate,
    },
};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathPlan {
    pub path: &'static str,
    pub fits: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    pub required_bytes: Option<f64>,
    pub usable_bytes: f64,
    /// Extra usable memory needed before the smallest quantization fits.
    pub shortfall_bytes: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_count: Option<usize>,
    pub throughput: Throughput,
    pub throughput_basis: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub model: String,
    pub context: Option<f64>,
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kv_cache_type: Option<&'static str>,
    pub unified_memory: bool,
    /// False when the requested context's KV cache cannot be sized from sourced geometry.
    pub kv_cache_known: bool,
    pub paths: Vec<PathPlan>,
    pub recommended: Option<&'static str>,
}

fn discrete(vendor: &GpuVendor) -> bool {
    matches!(
        vendor,
        GpuVendor::Nvidia | GpuVendor::Amd | GpuVendor::Intel
    )
}

struct Inputs<'a> {
    model: &'a CatalogModel,
    dataset: &'a PerfDataset,
    context: Option<f64>,
    backend: &'a str,
}

fn plan_path(
    inputs: &Inputs,
    path: &'static str,
    hardware: Hardware,
    modeled: bool,
    gpu_count: Option<usize>,
) -> Result<PathPlan, ValidationError> {
    let Inputs {
        model,
        dataset,
        context,
        backend,
    } = *inputs;
    let sized = evaluate(&SizingRequest {
        model: model.sizing(),
        hardware: hardware.clone(),
        context,
    })?;
    let fit = sized.fit;
    let shortfall = match (fit.fits, fit.reason, fit.required_bytes) {
        (false, Some("vram-bound" | "ram-bound"), Some(required)) => Some(
            (required / (1.0 - HEADROOM) - fit.usable_bytes)
                .max(0.0)
                .ceil(),
        ),
        _ => None,
    };
    let (estimate, basis) = match (&fit.quant, modeled) {
        (Some(quant), true) => {
            let estimate = throughput(model, quant, &hardware, dataset, backend)?;
            let basis = if estimate.known {
                "estimated"
            } else {
                "unknown: no sourced throughput class for this hardware and backend"
            };
            (estimate, basis)
        }
        (Some(_), false) => (
            Throughput::unknown(),
            "unknown: split across devices is not modeled; measure with llmup bench",
        ),
        (None, _) => (Throughput::unknown(), "unknown: does not fit"),
    };
    Ok(PathPlan {
        path,
        fits: fit.fits,
        quant: fit.quant.map(|quant| quant.name),
        required_bytes: fit.required_bytes,
        usable_bytes: fit.usable_bytes,
        shortfall_bytes: shortfall,
        reason: fit.reason,
        gpu_count,
        throughput: estimate,
        throughput_basis: basis,
    })
}

/// Evaluates every execution path this hardware offers for one model, without network access.
pub fn plan(
    model: &CatalogModel,
    hardware: &Hardware,
    dataset: &PerfDataset,
    context: Option<f64>,
    backend: &str,
) -> Result<Plan, ValidationError> {
    plan_with_cache(model, hardware, dataset, context, backend, None)
}

/// Like [`plan`], sizing the KV cache at `kv_cache` precision instead of f16.
pub fn plan_with_cache(
    model: &CatalogModel,
    hardware: &Hardware,
    dataset: &PerfDataset,
    context: Option<f64>,
    backend: &str,
    kv_cache: Option<KvCacheType>,
) -> Result<Plan, ValidationError> {
    if kv_cache.is_some() && context.is_none() {
        return Err(ValidationError("a KV cache type requires a context".into()));
    }
    let resized = kv_cache.map(|kind| model.with_kv_cache(kind)).transpose()?;
    let model = resized.as_ref().unwrap_or(model);
    let inputs = Inputs {
        model,
        dataset,
        context,
        backend,
    };
    let unified = hardware.is_unified();
    let cpu_only = model
        .quantizations
        .iter()
        .any(crate::sizing::Quantization::is_bitnet);
    let mut paths = Vec::new();
    let gpus: Vec<_> = hardware
        .gpu
        .iter()
        .filter(|gpu| !cpu_only && discrete(&gpu.vendor) && gpu.vram_bytes > 0.0)
        .cloned()
        .collect();
    if unified && !cpu_only {
        paths.push(plan_path(&inputs, "unified", hardware.clone(), true, None)?);
    } else if let Some(largest) = gpus
        .iter()
        .max_by(|left, right| left.vram_bytes.total_cmp(&right.vram_bytes))
    {
        let mut single = hardware.clone();
        single.gpu = vec![largest.clone()];
        paths.push(plan_path(&inputs, "gpu", single, true, Some(1))?);
        let peers: Vec<_> = gpus
            .iter()
            .filter(|gpu| gpu.vendor == largest.vendor)
            .collect();
        if peers.len() > 1 {
            let mut pooled = hardware.clone();
            let mut device = largest.clone();
            device.vram_bytes = peers.iter().map(|gpu| gpu.vram_bytes).sum();
            pooled.gpu = vec![device];
            paths.push(plan_path(
                &inputs,
                "multi-gpu",
                pooled,
                false,
                Some(peers.len()),
            )?);
        }
        let mut offload = hardware.clone();
        offload.gpu.clear();
        offload.unified_memory = Some(false);
        offload.free_ram_bytes += gpus.iter().map(|gpu| gpu.vram_bytes).sum::<f64>();
        paths.push(plan_path(
            &inputs,
            "cpu-offload",
            offload,
            false,
            Some(gpus.len()),
        )?);
    }
    let mut cpu = hardware.clone();
    cpu.gpu.clear();
    cpu.unified_memory = Some(false);
    paths.push(plan_path(&inputs, "cpu", cpu, true, None)?);
    let recommended = paths.iter().find(|path| path.fits).map(|path| path.path);
    Ok(Plan {
        model: model.id.clone(),
        context,
        backend: backend.into(),
        kv_cache_type: kv_cache.map(|kind| KvCacheType::label(Some(kind))),
        unified_memory: unified,
        kv_cache_known: context.is_none() || model.kv_bytes_per_token.is_some(),
        paths,
        recommended,
    })
}

fn gib(bytes: f64) -> String {
    format!("{:.1} GiB", bytes / 1073741824.0)
}

pub fn format_plan(plan: &Plan) -> String {
    let context = plan.context.map_or_else(
        || "default context".to_owned(),
        |tokens| format!("{tokens} tokens"),
    );
    let context = match plan.kv_cache_type {
        Some(kind) => format!("{context}, KV {kind}"),
        None => context,
    };
    let mut text = format!(
        "Plan for {} ({context}, {} backend{})\n\n",
        plan.model,
        plan.backend,
        if plan.unified_memory {
            ", unified memory"
        } else {
            ""
        }
    );
    if !plan.kv_cache_known {
        text.push_str(
            "KV cache at this context is unknown (no sourced attention geometry); sizes cover weights only.\n\n",
        );
    }
    for path in &plan.paths {
        let label = match path.gpu_count {
            Some(1) => format!("{} (1 GPU)", path.path),
            Some(count) if path.path != "cpu-offload" => format!("{} ({count} GPUs)", path.path),
            _ => path.path.to_owned(),
        };
        let fit = if path.fits {
            format!(
                "fits {} using {} of {}",
                path.quant.as_deref().unwrap_or("?"),
                path.required_bytes
                    .map_or_else(|| "unknown".to_owned(), gib),
                gib(path.usable_bytes)
            )
        } else if let Some(shortfall) = path.shortfall_bytes {
            format!("does not fit; needs {} more usable memory", gib(shortfall))
        } else {
            format!("does not fit ({})", path.reason.unwrap_or("unknown"))
        };
        let speed = if path.throughput.known {
            format!(
                "{}-{} tok/s estimated",
                path.throughput.low_tok_per_sec, path.throughput.high_tok_per_sec
            )
        } else if path.fits && path.throughput_basis.contains("not modeled") {
            "speed unknown (not modeled; measure with llmup bench)".to_owned()
        } else if path.fits {
            "speed unknown (no sourced throughput data)".to_owned()
        } else {
            String::new()
        };
        text.push_str(&format!("  {label:<22} {fit}"));
        if !speed.is_empty() {
            text.push_str(&format!("; {speed}"));
        }
        text.push('\n');
    }
    text.push_str(&match plan.recommended {
        Some(path) => format!("\nRecommended path: {path}\n"),
        None => "\nNo path fits this model at this context.\n".to_owned(),
    });
    text
}
