use crate::{
    catalog::{BACKENDS, CatalogModel, PerfClass, PerfDataset, require},
    sizing::{
        Architecture, GpuVendor, Hardware, Platform, Quantization, SizingRequest, ValidationError,
        evaluate, parse_param_count, quant_bits,
    },
};
use serde::Serialize;
use serde_json::{Value, json};

const GIB: f64 = 1073741824.0;
pub fn unified(hardware: &Hardware) -> bool {
    hardware.is_unified()
}
pub fn largest_vram(hardware: &Hardware) -> f64 {
    hardware
        .gpu
        .iter()
        .map(|gpu| gpu.vram_bytes)
        .fold(0.0_f64, f64::max)
}
pub fn hardware_score(hardware: &Hardware) -> Value {
    let vram = largest_vram(hardware);
    let pool = if vram > 0.0 {
        vram
    } else if unified(hardware) {
        hardware.total_ram_bytes
    } else {
        0.0
    };
    let sub = [
        (pool / (24.0 * GIB)).clamp(0.0, 1.0),
        (hardware.total_ram_bytes / (64.0 * GIB)).clamp(0.0, 1.0),
        if vram > 0.0 {
            0.5 + 0.5 * (vram / (24.0 * GIB)).clamp(0.0, 1.0)
        } else if unified(hardware) {
            0.65
        } else {
            0.25
        },
        (hardware.free_disk_bytes / (200.0 * GIB)).clamp(0.0, 1.0),
    ];
    let weights = [0.4, 0.25, 0.25, 0.1];
    let mut best = 0;
    let mut deficit = -1.0;
    let mut total = 0.0;
    for index in 0..4 {
        total += weights[index] * sub[index];
        let current = weights[index] * (1.0 - sub[index]);
        if current > deficit {
            best = index;
            deficit = current;
        }
    }
    json!({"total":(total * 100.0).round() as u32,"sub":{"vram":sub[0],"ram":sub[1],"compute":sub[2],"storage":sub[3]},"bottleneck":(["vram","ram","compute","storage"][best])})
}

pub fn match_perf<'a>(hardware: &Hardware, dataset: &'a PerfDataset) -> Option<&'a PerfClass> {
    let discrete = hardware
        .gpu
        .iter()
        .filter(|gpu| {
            matches!(
                gpu.vendor,
                GpuVendor::Nvidia | GpuVendor::Amd | GpuVendor::Intel
            ) && gpu.vram_bytes > 0.0
        })
        .fold(None, |best: Option<&crate::sizing::Gpu>, gpu| {
            if best.is_none_or(|prior| gpu.vram_bytes > prior.vram_bytes) {
                Some(gpu)
            } else {
                best
            }
        });
    let (vendor, kind, bytes) = if unified(hardware) && hardware.platform != Platform::Darwin {
        // Only Apple unified classes are sourced; other unified SoCs stay honest `unknown`.
        ("shared", "unified", hardware.total_ram_bytes)
    } else if let Some(gpu) = discrete {
        (
            match gpu.vendor {
                GpuVendor::Nvidia => "nvidia",
                GpuVendor::Amd => "amd",
                _ => "intel",
            },
            "discrete",
            gpu.vram_bytes,
        )
    } else if unified(hardware) {
        ("apple", "unified", hardware.total_ram_bytes)
    } else {
        ("none", "cpu", hardware.total_ram_bytes)
    };
    dataset.classes.iter().find(|class| {
        class.vendor == vendor
            && class.kind == kind
            && bytes >= class.min_bytes
            && bytes < class.max_bytes
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Throughput {
    pub low_tok_per_sec: f64,
    pub high_tok_per_sec: f64,
    pub known: bool,
}
impl Throughput {
    pub fn unknown() -> Self {
        Self {
            low_tok_per_sec: 0.0,
            high_tok_per_sec: 0.0,
            known: false,
        }
    }
}

pub fn throughput(
    model: &CatalogModel,
    quant: &Quantization,
    hardware: &Hardware,
    dataset: &PerfDataset,
    backend: &str,
) -> Result<Throughput, ValidationError> {
    require(BACKENDS.contains(&backend), "unknown backend")?;
    if model.is_advisory_only() || quant.name.eq_ignore_ascii_case("Q1_0") {
        return Ok(Throughput::unknown());
    }
    let Some(class) = match_perf(hardware, dataset) else {
        return Ok(Throughput::unknown());
    };
    let efficiency = class
        .efficiency_by_backend
        .as_ref()
        .and_then(|values| values.get(backend))
        .copied()
        .or_else(|| {
            ["ollama", "llamacpp"]
                .contains(&backend)
                .then_some(class.efficiency)
        });
    let Some(efficiency) = efficiency else {
        return Ok(Throughput::unknown());
    };
    let Some(bytes) = decode_bytes(model, quant)? else {
        return Ok(Throughput::unknown());
    };
    let point = class.mem_bandwidth_gbps * 1e9 * efficiency / bytes;
    Ok(Throughput {
        low_tok_per_sec: (point * 0.7 * 10.0).round() / 10.0,
        high_tok_per_sec: (point * 1.3 * 10.0).round() / 10.0,
        known: true,
    })
}

/// Bytes read per decoded token (active weights at the quantization's bit width), or `None`
/// when no sourced estimate exists (1-bit quants, MoE without active params or known bits).
pub fn decode_bytes(
    model: &CatalogModel,
    quant: &Quantization,
) -> Result<Option<f64>, ValidationError> {
    if model.params == "unknown"
        || matches!(model.architecture, Architecture::Unknown)
        || quant.name.eq_ignore_ascii_case("Q1_0")
    {
        return Ok(None);
    }
    let label = if matches!(model.architecture, Architecture::Moe) {
        model.active_params.as_deref()
    } else {
        Some(model.params.as_str())
    };
    let Some(label) = label else {
        return Ok(None);
    };
    let bytes = if let Some(bits) = quant_bits(&quant.name) {
        (parse_param_count(label)? * bits / 8.0).ceil()
    } else if matches!(model.architecture, Architecture::Moe) {
        return Ok(None);
    } else {
        quant.disk_bytes
    };
    Ok((bytes > 0.0).then_some(bytes))
}

pub fn verdict(
    model: &CatalogModel,
    hardware: &Hardware,
    dataset: &PerfDataset,
    context: Option<f64>,
    backend: &str,
) -> Result<Value, ValidationError> {
    let sized = evaluate(&SizingRequest {
        model: model.sizing(),
        hardware: hardware.clone(),
        context,
    })?;
    let fit = sized.fit;
    if !fit.fits {
        return Ok(
            json!({"runnable":"no","throughput":Throughput::unknown(),"reason":fit.reason,"requiredBytes":fit.required_bytes,"usableBytes":fit.usable_bytes}),
        );
    }
    let quant = fit
        .quant
        .ok_or_else(|| ValidationError("fitting quantization missing".into()))?;
    let estimate = throughput(model, &quant, hardware, dataset, backend)?;
    let runnable =
        if estimate.known && (estimate.low_tok_per_sec + estimate.high_tok_per_sec) / 2.0 >= 10.0 {
            "yes"
        } else {
            "slow"
        };
    Ok(
        json!({"runnable":runnable,"throughput":estimate,"quant":quant,"requiredBytes":fit.required_bytes,"usableBytes":fit.usable_bytes}),
    )
}
