use crate::{
    advice::{throughput, unified},
    catalog::{BACKENDS, CAPABILITIES, Catalog, CatalogModel, PerfDataset, date, require},
    sizing::{
        Architecture, Hardware, KvCacheType, SizingRequest, ValidationError, evaluate,
        parse_param_count, quant_bits,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdviceOptions {
    pub task: Option<String>,
    pub context: Option<f64>,
    pub context_percent: Option<u8>,
    #[serde(default)]
    pub max_context: bool,
    pub backend: Option<String>,
    pub available_backends: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kv_cache: Option<KvCacheType>,
}
impl AdviceOptions {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require(
            self.kv_cache.is_none()
                || self.context.is_some()
                || self.context_percent.is_some()
                || self.max_context,
            "a KV cache type requires context, contextPercent, or maxContext",
        )?;
        require(
            usize::from(self.context.is_some())
                + usize::from(self.context_percent.is_some())
                + usize::from(self.max_context)
                <= 1,
            "context, contextPercent, and maxContext are mutually exclusive",
        )?;
        require(
            self.task
                .as_ref()
                .is_none_or(|task| CAPABILITIES.contains(&task.as_str())),
            "invalid task",
        )?;
        require(
            self.backend
                .as_ref()
                .is_none_or(|backend| BACKENDS.contains(&backend.as_str())),
            "invalid backend",
        )?;
        require(
            self.context_percent
                .is_none_or(|percent| [25, 50, 75, 100].contains(&percent)),
            "invalid context percentage",
        )?;
        require(
            self.context.is_none_or(|context| {
                context.is_finite()
                    && context.fract() == 0.0
                    && (1.0..=10_000_000.0).contains(&context)
            }),
            "invalid context",
        )?;
        require(
            self.available_backends.as_ref().is_none_or(|backends| {
                backends
                    .iter()
                    .all(|backend| BACKENDS.contains(&backend.as_str()))
            }),
            "invalid available backend",
        )
    }
    pub fn tokens(&self, model: &CatalogModel) -> Option<f64> {
        self.context_percent
            .map(|percent| {
                (model.context_length * f64::from(percent) / 100.0)
                    .floor()
                    .max(1.0)
            })
            .or(self.context)
    }
}

pub fn backends(model: &CatalogModel, hardware: &Hardware) -> Vec<&'static str> {
    let mut result = Vec::new();
    if model.source.ollama.is_some() {
        result.push("ollama");
    }
    if model.source.gguf.is_some() {
        result.push("llamacpp");
    }
    if unified(hardware) && model.source.mlx.is_some() {
        result.push("mlx");
    }
    if model.source.gguf.is_some() || (unified(hardware) && model.source.mlx.is_some()) {
        result.push("lmstudio");
    }
    result
}

pub fn recommend(
    catalog: &Catalog,
    hardware: &Hardware,
    perf: &PerfDataset,
    options: &AdviceOptions,
) -> Result<Value, ValidationError> {
    recommend_inner(catalog, hardware, perf, options, false)
}
pub fn recommend_detailed(
    catalog: &Catalog,
    hardware: &Hardware,
    perf: &PerfDataset,
    options: &AdviceOptions,
) -> Result<Value, ValidationError> {
    recommend_inner(catalog, hardware, perf, options, true)
}
fn recommend_inner(
    catalog: &Catalog,
    hardware: &Hardware,
    perf: &PerfDataset,
    options: &AdviceOptions,
    detailed: bool,
) -> Result<Value, ValidationError> {
    options.validate()?;
    let reference = OffsetDateTime::parse(&catalog.generated_at, &Rfc3339)
        .map_err(|_| ValidationError("invalid reference date".into()))?;
    let backend = options.backend.as_deref().unwrap_or("ollama");
    let mut entries: Vec<(Value, f64, f64, String, String)> = Vec::new();
    let mut wont_fit = Vec::new();
    let mut usable = 0.0;
    let mut kind = "ram";
    for model in &catalog.models {
        let resized = options
            .kv_cache
            .map(|kind| model.with_kv_cache(kind))
            .transpose()?;
        let model = resized.as_ref().unwrap_or(model);
        let context = options.tokens(model);
        let sized = evaluate(&SizingRequest {
            model: model.sizing(),
            hardware: hardware.clone(),
            context,
        })?;
        usable = sized.usable_bytes;
        kind = sized.memory_kind;
        let supported = backends(model, hardware);
        if options
            .available_backends
            .as_ref()
            .is_some_and(|available| {
                !supported
                    .iter()
                    .any(|backend| available.iter().any(|name| name == backend))
            })
        {
            continue;
        }
        let fit = sized.fit;
        if !fit.fits {
            wont_fit.push(json!({"id":model.id,"reason":fit.reason}));
            continue;
        }
        let quant = fit
            .quant
            .ok_or_else(|| ValidationError("missing fitting quant".into()))?;
        let required = fit
            .required_bytes
            .ok_or_else(|| ValidationError("missing fitting size".into()))?;
        let param_class = ((parse_param_count(&model.params)?.log10() - 9.0) / 3.0).clamp(0.0, 1.0);
        let quality = model
            .benchmark_proxy
            .map_or(param_class, |proxy| 0.5 * param_class + 0.5 * proxy);
        let fit_score = (1.0 - (required / usable - 0.6).abs() / 0.6).clamp(0.0, 1.0);
        let active = if matches!(model.architecture, Architecture::Moe) {
            model.active_params.as_deref().unwrap_or(&model.params)
        } else {
            &model.params
        };
        let bandwidth = if kind == "vram" {
            1.0
        } else if unified(hardware) {
            0.5
        } else {
            0.2
        };
        let speed = (bandwidth
            * (4.7 / quant_bits(&quant.name).unwrap_or(4.7)).clamp(0.0, 1.0)
            * (7e9 / parse_param_count(active)?))
        .clamp(0.0, 1.0);
        let (day, _) = model
            .recency()
            .ok_or_else(|| ValidationError("catalog entry has no recency date".into()))?;
        let released = date(day)?.midnight().assume_utc();
        let age = (reference - released).as_seconds_f64() / 86400.0;
        let recency = (1.0 - age / 730.0).clamp(0.0, 1.0);
        let capability = if options
            .task
            .as_ref()
            .is_none_or(|task| model.capabilities.contains(task))
        {
            1.0
        } else {
            0.0
        };
        let score =
            (0.3 * quality + 0.2 * fit_score + 0.2 * speed + 0.15 * recency + 0.15 * capability)
                .clamp(0.0, 1.0);
        let estimate = throughput(model, &quant, hardware, perf, backend)?;
        let verdict = if estimate.known
            && (estimate.low_tok_per_sec + estimate.high_tok_per_sec) / 2.0 >= 10.0
        {
            "yes"
        } else {
            "slow"
        };
        let mut entry = json!({"rank":0,"id":model.id,"family":model.family,"params":model.params,"quant":quant.name,
            "requiredBytes":required,"license":model.license,"capabilities":model.capabilities,"score":score,"verdict":verdict,
            "estTokPerSec": if estimate.known { json!({"lowTokPerSec":estimate.low_tok_per_sec,"highTokPerSec":estimate.high_tok_per_sec}) } else { Value::Null },
            "backends":supported,"throughputBackend":backend});
        if model.provenance == crate::catalog::EntryProvenance::Auto {
            entry["provenance"] = json!("auto");
        }
        if detailed {
            entry["throughputEvidence"] = json!({"backend":backend,"source":"offline-estimate","unknownReason":if estimate.known{None}else{Some("no-sourced-performance-profile")}});
            entry["scores"] = json!({"quality":quality,"fit":fit_score,"speed":speed,"recency":recency,"capability":capability});
            entry["usableBytes"] = json!(usable);
            entry["throughput"] = serde_json::to_value(&estimate)
                .map_err(|_| ValidationError("invalid throughput".into()))?;
        }
        let index = model
            .quantizations
            .iter()
            .position(|candidate| candidate.name == quant.name)
            .ok_or_else(|| ValidationError("selected quant missing".into()))?;
        if let Some(tokens) = context {
            entry["context"] = json!(tokens);
            entry["weightsBytes"] = json!(sized.weights[index]);
            entry["kvCacheBytes"] = json!(model.kv_bytes_per_token.map(|rate| rate * tokens));
            entry["kvPrecision"] = json!(KvCacheType::label(options.kv_cache));
        } else if options.max_context {
            let maximum = sized.max_context[index];
            entry["maxContextTokens"] =
                json!(maximum.map(|tokens| tokens.min(model.context_length)));
            entry["boundBy"] =
                json!(
                    maximum.map_or("unknown", |tokens| if tokens < model.context_length {
                        "hardware"
                    } else {
                        "model"
                    })
                );
            entry["kvPrecision"] = json!(KvCacheType::label(options.kv_cache));
        }
        entries.push((
            entry,
            score,
            model.benchmark_proxy.unwrap_or(-1.0),
            day.to_string(),
            model.id.clone(),
        ));
    }
    entries.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| right.2.total_cmp(&left.2))
            .then_with(|| right.3.cmp(&left.3))
            .then_with(|| left.4.cmp(&right.4))
    });
    let ranked: Vec<Value> = entries
        .into_iter()
        .enumerate()
        .map(|(index, (mut entry, ..))| {
            entry["rank"] = json!(index + 1);
            entry
        })
        .collect();
    let command = ranked
        .first()
        .and_then(|entry| entry["id"].as_str())
        .map(|id| format!("rigspark up {id}"));
    Ok(
        json!({"hardware":{"arch":hardware.arch,"platform":hardware.platform,"usableMemoryBytes":usable,"memoryKind":kind},"ranked":ranked,"wontFit":wont_fit,"command":command}),
    )
}
