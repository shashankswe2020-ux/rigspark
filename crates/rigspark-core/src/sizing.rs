use serde::{Deserialize, Serialize};
use thiserror::Error;

const SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const OS_RESERVE: f64 = 2.0 * 1024.0 * 1024.0 * 1024.0;
pub const HEADROOM: f64 = 0.15;

#[derive(Debug, Error)]
#[error("{0}")]
pub struct ValidationError(pub String);

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    Dense,
    Moe,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CpuArch {
    X64,
    Arm64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Darwin,
    Linux,
    Win32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GpuVendor {
    Apple,
    Nvidia,
    Amd,
    Intel,
    None,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Gpu {
    pub vendor: GpuVendor,
    pub vram_bytes: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Hardware {
    pub arch: CpuArch,
    pub platform: Platform,
    pub total_ram_bytes: f64,
    pub free_ram_bytes: f64,
    pub free_disk_bytes: f64,
    pub gpu: Vec<Gpu>,
    /// Shared CPU/GPU memory pool (Apple Silicon, AMD APUs, NVIDIA Grace/Jetson SoCs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified_memory: Option<bool>,
}

impl Hardware {
    /// Detected topology when known; otherwise only Apple Silicon Macs are assumed unified.
    pub fn is_unified(&self) -> bool {
        self.unified_memory
            .unwrap_or(self.arch == CpuArch::Arm64 && self.platform == Platform::Darwin)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectorArtifact {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Quantization {
    pub name: String,
    pub disk_bytes: f64,
    pub min_ram_bytes: f64,
    pub min_vram_bytes: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projectors: Vec<ProjectorArtifact>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Model {
    pub id: String,
    pub params: String,
    pub architecture: Architecture,
    pub context_length: f64,
    pub kv_bytes_per_token: Option<f64>,
    pub quantizations: Vec<Quantization>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SizingRequest {
    pub model: Model,
    pub hardware: Hardware,
    pub context: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FitResult {
    pub fits: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quant: Option<Quantization>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    pub usable_bytes: f64,
    pub required_bytes: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizingResult {
    pub fit: FitResult,
    pub memory_kind: &'static str,
    pub usable_bytes: f64,
    pub weights: Vec<f64>,
    pub required: Vec<f64>,
    pub at_context: Vec<Option<f64>>,
    pub max_context: Vec<Option<f64>>,
}

fn invalid(message: &str) -> ValidationError {
    ValidationError(message.into())
}

fn safe_number(value: f64, minimum: f64, label: &str) -> Result<(), ValidationError> {
    if !value.is_finite() || value < minimum || value > SAFE_INTEGER || value.fract() != 0.0 {
        return Err(invalid(&format!(
            "{label} must be a safe integer >= {minimum}"
        )));
    }
    Ok(())
}

pub fn parse_param_count(label: &str) -> Result<f64, ValidationError> {
    let split = label
        .len()
        .checked_sub(1)
        .ok_or_else(|| invalid("invalid parameter label"))?;
    if !label.is_ascii() {
        return Err(invalid("invalid parameter label"));
    }
    let (number, unit) = label.split_at(split);
    let mut parts = number.split('.');
    let integer = parts.next().unwrap_or("");
    let fraction = parts.next();
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.is_some_and(|value| {
            value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit())
        })
        || parts.next().is_some()
    {
        return Err(invalid("invalid parameter label"));
    }
    let multiplier = match unit {
        "M" => 1e6,
        "B" => 1e9,
        "T" => 1e12,
        _ => return Err(invalid("invalid parameter unit")),
    };
    let count = number
        .parse::<f64>()
        .map_err(|_| invalid("invalid parameter label"))?
        * multiplier;
    if !count.is_finite() || count <= 0.0 || count > SAFE_INTEGER {
        return Err(invalid("parameter count outside safe range"));
    }
    Ok(count)
}

pub fn quant_bits(name: &str) -> Option<f64> {
    let name = name.to_ascii_lowercase();
    [
        // OCP Microscaling: 4-bit elements plus one shared 8-bit scale per 32-element block.
        ("mxfp4", 4.25),
        ("iq1", 1.9),
        ("iq2", 2.4),
        ("iq3", 3.4),
        ("iq4", 4.3),
        ("q2", 2.8),
        ("q3", 3.5),
        ("q4", 4.7),
        ("q5", 5.6),
        ("q6", 6.6),
        ("q8", 8.5),
        ("f16", 16.5),
        ("fp16", 16.5),
        ("bf16", 16.5),
        ("f32", 32.5),
        ("fp32", 32.5),
    ]
    .into_iter()
    .find_map(|(prefix, bits)| name.starts_with(prefix).then_some(bits))
}

pub fn kv_cache_bytes(per_token: u64, tokens: u64) -> Result<u64, ValidationError> {
    if per_token == 0 {
        return Err(invalid("KV bytes per token must be positive"));
    }
    let bytes = per_token
        .checked_mul(tokens)
        .ok_or_else(|| invalid("KV size overflow"))?;
    if bytes > SAFE_INTEGER as u64 {
        return Err(invalid("KV size exceeds safe-integer range"));
    }
    Ok(bytes)
}

/// KV cache element types shared by llama.cpp and Ollama.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum KvCacheType {
    #[default]
    #[serde(rename = "f16")]
    F16,
    #[serde(rename = "q8_0")]
    Q8_0,
    #[serde(rename = "q4_0")]
    Q4_0,
}

impl KvCacheType {
    pub fn name(self) -> &'static str {
        match self {
            Self::F16 => "f16",
            Self::Q8_0 => "q8_0",
            Self::Q4_0 => "q4_0",
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        [Self::F16, Self::Q8_0, Self::Q4_0]
            .into_iter()
            .find(|kind| kind.name() == name)
    }
    /// Label for advice output; the implicit default keeps its historical `fp16` spelling.
    pub fn label(kind: Option<Self>) -> &'static str {
        match kind {
            None | Some(Self::F16) => "fp16",
            Some(kind) => kind.name(),
        }
    }
    /// (bytes per block, elements per block) from ggml's block_q8_0 and block_q4_0 layouts.
    fn block(self) -> (u64, u64) {
        match self {
            Self::F16 => (2, 1),
            Self::Q8_0 => (34, 32),
            Self::Q4_0 => (18, 32),
        }
    }
    fn bytes_for(self, elements: u64) -> Option<u64> {
        let (bytes, per_block) = self.block();
        elements.div_ceil(per_block).checked_mul(bytes)
    }
}

/// Re-sizes a catalog f16 KV rate (K and V halves) for the requested cache types.
pub fn typed_kv_bytes_per_token(
    f16_per_token: u64,
    k: KvCacheType,
    v: KvCacheType,
) -> Result<u64, ValidationError> {
    if f16_per_token == 0 || !f16_per_token.is_multiple_of(4) {
        return Err(invalid(
            "KV bytes per token must split into f16 K and V halves",
        ));
    }
    let elements = f16_per_token / 4;
    k.bytes_for(elements)
        .zip(v.bytes_for(elements))
        .and_then(|(k, v)| k.checked_add(v))
        .filter(|bytes| *bytes <= SAFE_INTEGER as u64)
        .ok_or_else(|| invalid("KV size exceeds safe-integer range"))
}

fn validate(request: &SizingRequest) -> Result<(), ValidationError> {
    let model = &request.model;
    if model.id.is_empty() || model.id.len() > 256 {
        return Err(invalid("invalid model id length"));
    }
    parse_param_count(&model.params)?;
    safe_number(model.context_length, 1.0, "contextLength")?;
    if let Some(context) = request.context {
        safe_number(context, 1.0, "context")?;
        if context > 10_000_000.0 {
            return Err(invalid("context exceeds ceiling"));
        }
    }
    if let Some(rate) = model.kv_bytes_per_token {
        safe_number(rate, 1.0, "kvBytesPerToken")?;
    }
    for (name, value) in [
        ("totalRamBytes", request.hardware.total_ram_bytes),
        ("freeRamBytes", request.hardware.free_ram_bytes),
        ("freeDiskBytes", request.hardware.free_disk_bytes),
    ] {
        safe_number(value, 0.0, name)?;
    }
    if request.hardware.gpu.len() > 128 || model.quantizations.len() > 1024 {
        return Err(invalid("input collection too large"));
    }
    for gpu in &request.hardware.gpu {
        safe_number(gpu.vram_bytes, 0.0, "vramBytes")?;
    }
    for quant in &model.quantizations {
        if quant.name.is_empty() || quant.name.len() > 100 {
            return Err(invalid("invalid quantization name"));
        }
        safe_number(quant.disk_bytes, 1.0, "diskBytes")?;
        safe_number(quant.min_ram_bytes, 0.0, "minRamBytes")?;
        safe_number(quant.min_vram_bytes, 0.0, "minVramBytes")?;
        if quant
            .sha256
            .as_ref()
            .is_some_and(|sha| sha.len() != 64 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(invalid("invalid sha256"));
        }
    }
    Ok(())
}

pub fn memory_capacity(hardware: &Hardware) -> (&'static str, f64) {
    let unified = hardware.is_unified();
    let vram = hardware
        .gpu
        .iter()
        .map(|gpu| gpu.vram_bytes)
        .fold(0.0_f64, f64::max);
    let memory_kind = if !unified && vram > 0.0 {
        "vram"
    } else {
        "ram"
    };
    let usable_bytes = if memory_kind == "vram" {
        vram
    } else {
        ((if unified {
            hardware.total_ram_bytes
        } else {
            hardware.free_ram_bytes
        }) - OS_RESERVE)
            .max(0.0)
    };
    (memory_kind, usable_bytes)
}
pub fn evaluate(request: &SizingRequest) -> Result<SizingResult, ValidationError> {
    validate(request)?;
    let model = &request.model;
    let hardware = &request.hardware;
    let (memory_kind, usable_bytes) = memory_capacity(hardware);
    let budget = usable_bytes * (1.0 - HEADROOM);
    let params = parse_param_count(&model.params)?;
    let mut weights = Vec::new();
    let mut required = Vec::new();
    let mut at_context = Vec::new();
    let mut max_context = Vec::new();
    let mut best: Option<(usize, f64)> = None;
    let mut smallest: Option<(f64, bool)> = None;
    for (index, quant) in model.quantizations.iter().enumerate() {
        let bits = quant_bits(&quant.name);
        if bits.is_none() && matches!(model.architecture, Architecture::Moe) {
            return Err(invalid("cannot size MoE with unknown quantization"));
        }
        let weight = quant
            .disk_bytes
            .max(bits.map_or(0.0, |bits| (params * bits / 8.0).ceil()));
        let legacy = weight + (weight * 0.15).ceil();
        safe_number(legacy, 1.0, "required memory")?;
        let slack = (weight * 0.05).ceil();
        let context_bytes = match (model.kv_bytes_per_token, request.context) {
            (Some(rate), Some(context)) => {
                let explicit = weight + kv_cache_bytes(rate as u64, context as u64)? as f64 + slack;
                safe_number(explicit, 1.0, "context memory")?;
                Some(legacy.max(explicit))
            }
            _ => None,
        };
        let maximum = model.kv_bytes_per_token.map(|rate| {
            if legacy > budget {
                0.0
            } else {
                ((budget - weight - slack) / rate).floor()
            }
        });
        let footprint = context_bytes.unwrap_or(legacy);
        let memory_fits = footprint <= budget;
        if memory_fits && quant.disk_bytes <= hardware.free_disk_bytes {
            let better = match best {
                None => true,
                Some((best_index, best_bytes)) => {
                    footprint > best_bytes
                        || (footprint == best_bytes
                            && bits
                                .zip(quant_bits(&model.quantizations[best_index].name))
                                .is_some_and(|(current, prior)| current > prior))
                }
            };
            if better {
                best = Some((index, footprint));
            }
        }
        if smallest.is_none_or(|(bytes, _)| footprint < bytes) {
            smallest = Some((footprint, memory_fits));
        }
        weights.push(weight);
        required.push(legacy);
        at_context.push(context_bytes);
        max_context.push(maximum);
    }
    let fit = if request
        .context
        .is_some_and(|context| context > model.context_length)
    {
        FitResult {
            fits: false,
            quant: None,
            reason: Some("context-bound"),
            usable_bytes,
            required_bytes: None,
        }
    } else if let Some((index, bytes)) = best {
        FitResult {
            fits: true,
            quant: Some(model.quantizations[index].clone()),
            reason: None,
            usable_bytes,
            required_bytes: Some(bytes),
        }
    } else {
        let reason = if smallest.is_some_and(|(_, fits)| fits) {
            "disk-bound"
        } else if memory_kind == "vram" {
            "vram-bound"
        } else {
            "ram-bound"
        };
        FitResult {
            fits: false,
            quant: None,
            reason: Some(reason),
            usable_bytes,
            required_bytes: smallest.map(|(bytes, _)| bytes),
        }
    };
    Ok(SizingResult {
        fit,
        memory_kind,
        usable_bytes,
        weights,
        required,
        at_context,
        max_context,
    })
}
