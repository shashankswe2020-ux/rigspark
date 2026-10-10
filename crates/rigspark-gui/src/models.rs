use crate::{
    Host,
    routes::{ApiResult, bad, body, json_response},
};
use axum::extract::Request;
use rigspark_core::{
    catalog::{Catalog, PerfDataset},
    ranking::{AdviceOptions, recommend_detailed},
};
use rigspark_runtime::{
    application::{LifecycleOptions, run_native_with_config},
    state::{Config, StateStore},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Read, sync::Arc};

/// Upper bound for one ranked-catalog page; covers the whole bundled catalog with headroom.
const MAX_RECOMMENDED_LIMIT: usize = 1000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdviceRequest {
    hardware: rigspark_core::sizing::Hardware,
    #[serde(default)]
    options: JsonAdviceOptions,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JsonAdviceOptions {
    context: Option<f64>,
    context_percent: Option<u8>,
    backend: Option<String>,
    limit: Option<usize>,
    kv_cache: Option<rigspark_core::sizing::KvCacheType>,
}

#[derive(Serialize)]
pub struct AdviceResponse {
    models: Vec<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AdviceError {
    InvalidArguments,
    InvalidRequest,
    RequestTooLarge,
    InternalError,
}

pub fn advice_json(reader: impl Read) -> Result<AdviceResponse, AdviceError> {
    const MAX_BYTES: usize = 64 * 1024;
    let mut input = Vec::new();
    reader
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut input)
        .map_err(|_| AdviceError::InvalidRequest)?;
    if input.len() > MAX_BYTES {
        return Err(AdviceError::RequestTooLarge);
    }
    let request: AdviceRequest =
        serde_json::from_slice(&input).map_err(|_| AdviceError::InvalidRequest)?;
    let limit = request.options.limit.unwrap_or(8);
    if !(1..=100).contains(&limit) {
        return Err(AdviceError::InvalidRequest);
    }
    let options = AdviceOptions {
        context: request.options.context,
        context_percent: request.options.context_percent,
        backend: request.options.backend,
        kv_cache: request.options.kv_cache,
        ..Default::default()
    };
    options
        .validate()
        .map_err(|_| AdviceError::InvalidRequest)?;
    let catalog =
        Catalog::parse(rigspark_core::MODELS_JSON).map_err(|_| AdviceError::InternalError)?;
    let perf =
        PerfDataset::parse(rigspark_core::PERF_JSON).map_err(|_| AdviceError::InternalError)?;
    let models = recommended(&catalog, &request.hardware, &perf, &options, limit)
        .map_err(|_| AdviceError::InvalidRequest)?;
    Ok(AdviceResponse { models })
}

fn active(host: &Host) -> Result<Value, crate::routes::ApiError> {
    let store = StateStore::new(Config::from_home(&host.home).map_err(|_| bad())?);
    Ok(match store.read().map_err(|_| bad())?.active {
        None => Value::Null,
        Some(active) => {
            let mut value = json!({"modelId":active.model_id,"backend":active.backend,"endpoint":active.endpoint,"port":active.port,"ownership":if active.owned_by_us{"owned"}else{"attached"}});
            if let Some(id) = active.runtime_model_id {
                value["runtimeModelId"] = json!(id);
            }
            if let Some(context) = active.context {
                value["context"] = json!(context);
            }
            if let Some(cache) = active.cache {
                value["cache"] = json!(cache);
            }
            value
        }
    })
}
pub fn recommended(
    catalog: &Catalog,
    hardware: &rigspark_core::sizing::Hardware,
    perf: &PerfDataset,
    options: &AdviceOptions,
    limit: usize,
) -> Result<Vec<Value>, rigspark_core::sizing::ValidationError> {
    let report = recommend_detailed(catalog, hardware, perf, options)?;
    let entries = report["ranked"]
        .as_array()
        .ok_or_else(|| rigspark_core::sizing::ValidationError("ranked models missing".into()))?;
    entries.iter().take(limit).map(|entry|{
        let source=catalog.models.iter().find(|model|entry["id"]==model.id).ok_or_else(||rigspark_core::sizing::ValidationError("ranked model absent".into()))?;
        let source=serde_json::to_value(source).map_err(|_|rigspark_core::sizing::ValidationError("invalid model".into()))?;
        let mut model=json!({});
        for key in ["id","family","params","architecture","activeParams","license","openWeight","contextLength","capabilities","releaseDate","addedAt","provenance","source","quantizations","kvBytesPerToken","benchmarkProxy"] {if let Some(value)=source.get(key){model[key]=value.clone();}}
        for key in ["verdict","requiredBytes","usableBytes","score","scores","throughput","throughputEvidence","backends"] {model[key]=entry[key].clone();}
        model["quant"]=entry["quant"].clone();
        model["diskBytes"]=source["quantizations"].as_array().and_then(|quants|quants.iter().find(|quant|quant["name"]==entry["quant"])).map(|quant|quant["diskBytes"].clone()).unwrap_or(Value::Null);
        if let Some(tokens)=entry.get("context") {model["contextTokens"]=tokens.clone();model["contextFitKnown"]=json!(!entry["kvCacheBytes"].is_null());model["contextSizing"]=json!({"tokens":tokens,"weightsBytes":entry["weightsBytes"],"kvCacheBytes":entry["kvCacheBytes"]});}
        if options.kv_cache.is_some() && let Some(precision)=entry.get("kvPrecision") {model["kvPrecision"]=precision.clone();}
        Ok(model)
    }).collect()
}
pub async fn dispatch(host: Arc<Host>, request: Request) -> ApiResult {
    let method = request.method().as_str().to_owned();
    let url = url::Url::parse(&format!("{}{}", host.origin(), request.uri())).map_err(|_| bad())?;
    let path = url.path();
    if path.starts_with("/api/catalog/") {
        use rigspark_runtime::catalog_update::{CatalogStore, OfficialCatalogTransport};
        let store = CatalogStore::official(&host.home);
        if url.query().is_some() {
            return Err(bad());
        }
        if path == "/api/catalog/status" && method == "GET" {
            return Ok(json_response(
                json!({"catalog": store.load().map_err(|_| bad())?.status}),
            ));
        }
        if path == "/api/catalog/update" && method == "POST" {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct UpdateRequest {}
            let _: UpdateRequest = body(request, 1024).await?;
            return match store.update(&OfficialCatalogTransport).await {
                Ok(status) => Ok(json_response(json!({"catalog": status}))),
                Err(error) => Ok(crate::error(
                    axum::http::StatusCode::BAD_REQUEST,
                    &error.to_string(),
                )),
            };
        }
        return Err(bad());
    }
    if path == "/api/models/active" && method == "GET" {
        return Ok(json_response(json!({"active":active(&host)?})));
    }
    if path == "/api/runtimes" && method == "GET" {
        return Ok(json_response(
            json!({"runtimes":rigspark_core::catalog::BACKENDS}),
        ));
    }
    if path.starts_with("/api/runtimes/") {
        let config = Config::from_home(&host.home).map_err(|_| bad())?;
        let mut runtimes = host.runtimes.lock().await;
        if path == "/api/runtimes/status" && method == "GET" {
            return Ok(json_response(
                json!({"runtimes":runtimes.list(config,&host.shutdown).await.map_err(|_|bad())?}),
            ));
        }
        let parts: Vec<_> = path.split('/').collect();
        if method == "POST" && parts.len() == 5 && ["start", "stop"].contains(&parts[4]) {
            return Ok(json_response(
                json!({"runtime":runtimes.change(config,parts[3],parts[4]=="start",&host.shutdown).await.map_err(|_|bad())?}),
            ));
        }
        return Err(bad());
    }
    if path == "/api/hardware" && method == "GET" {
        let (hardware, _) = rigspark_runtime::hardware::detect()
            .await
            .map_err(|_| bad())?;
        return Ok(json_response(json!({"hardware":hardware})));
    }
    let catalog = rigspark_runtime::catalog_update::CatalogStore::official(&host.home)
        .load()
        .map_err(|_| bad())?
        .catalog;
    if path == "/api/models/up" && method == "POST" {
        return activate_model(host, request, &catalog).await;
    }
    let (hardware, _) = rigspark_runtime::hardware::detect()
        .await
        .map_err(|_| bad())?;
    if path == "/api/models/recommended" && method == "GET" {
        let query: BTreeMap<String, String> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        let context = query
            .get("tokens")
            .map(|value| value.parse::<f64>().map_err(|_| bad()))
            .transpose()?;
        let preset = query
            .get("context")
            .map(|value| match value.as_str() {
                "low" => Ok(25),
                "mid" => Ok(50),
                "high" => Ok(75),
                "max" => Ok(100),
                _ => Err(bad()),
            })
            .transpose()?;
        let kv_cache = query
            .get("kvCache")
            .map(|value| rigspark_core::sizing::KvCacheType::parse(value).ok_or_else(bad))
            .transpose()?;
        let options = AdviceOptions {
            context,
            context_percent: preset,
            backend: query.get("runtime").cloned(),
            kv_cache,
            ..Default::default()
        };
        options.validate().map_err(|_| bad())?;
        let mut catalog = catalog;
        if let Some(months) = query.get("month") {
            let months = months.parse::<u8>().map_err(|_| bad())?;
            let today = rigspark_runtime::native_chat::timestamp().map_err(|_| bad())?;
            catalog
                .retain_recent(&today[..10], months)
                .map_err(|_| bad())?;
        }
        let perf = PerfDataset::parse(rigspark_core::PERF_JSON).map_err(|_| bad())?;
        let limit = query
            .get("limit")
            .map(|value| value.parse::<usize>().map_err(|_| bad()))
            .transpose()?
            .unwrap_or(8);
        if !(1..=MAX_RECOMMENDED_LIMIT).contains(&limit) {
            return Err(bad());
        }
        let models = recommended(&catalog, &hardware, &perf, &options, limit).map_err(|_| bad())?;
        return Ok(json_response(
            json!({"models":models,"runtime":query.get("runtime"),"contextPreset":query.get("context"),"kvCache":kv_cache}),
        ));
    }
    if path == "/api/models/installed" && method == "GET" {
        let query: BTreeMap<String, String> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        let port = query
            .get("port")
            .map(|value| value.parse::<u16>().map_err(|_| bad()))
            .transpose()?
            .unwrap_or(11434);
        let context = query
            .get("tokens")
            .map(|value| value.parse::<u32>().map_err(|_| bad()))
            .transpose()?;
        let (report, _, _) = rigspark_runtime::application::installed_inventory(
            &hardware,
            None,
            port,
            context,
            false,
            &host.shutdown,
        )
        .await
        .map_err(|_| bad())?;
        return Ok(json_response(
            json!({"models":report["models"],"source":"local-runtime-metadata"}),
        ));
    }
    Err(crate::routes::missing())
}

async fn activate_model(host: Arc<Host>, request: Request, catalog: &Catalog) -> ApiResult {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct Up {
        model: String,
        backend: Option<String>,
        port: Option<u16>,
        context: Option<u32>,
        #[serde(default)]
        installed: bool,
        #[serde(default)]
        bypass: bool,
        kv_cache: Option<rigspark_core::sizing::KvCacheType>,
        flash_attention: Option<rigspark_runtime::cache::FlashAttention>,
        prompt_cache: Option<rigspark_runtime::cache::PromptReuse>,
    }
    let input: Up = body(request, crate::MAX_REQUEST_BYTES).await?;
    let options = LifecycleOptions {
        command: "up".into(),
        model: Some(input.model),
        backend: input.backend,
        port: input.port,
        context: input.context,
        installed: input.installed,
        bypass: input.bypass,
        cache: rigspark_runtime::cache::CacheFlags {
            kv: input.kv_cache,
            flash_attention: input.flash_attention,
            prompt_reuse: input.prompt_cache,
        },
    };
    if let Err(error) =
        rigspark_runtime::application::check_selection_availability(&options, catalog)
    {
        return Ok(crate::error(
            axum::http::StatusCode::BAD_REQUEST,
            &error.to_string(),
        ));
    }
    let (hardware, _) = rigspark_runtime::hardware::detect()
        .await
        .map_err(|_| bad())?;
    if let Err(error) = run_native_with_config(
        &options,
        catalog,
        Some(&hardware),
        &host.shutdown,
        Config::from_home(&host.home).map_err(|_| bad())?,
    )
    .await
    {
        let message: String = rigspark_core::reports::strip_control(&error.0)
            .chars()
            .take(400)
            .collect();
        return Ok(crate::error(axum::http::StatusCode::BAD_REQUEST, &message));
    }
    let active = active(&host)?;
    host.ui.lock().await.model = active["modelId"].as_str().unwrap_or("local").into();
    Ok(json_response(json!({"active":active})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn advisory_activation_returns_explicit_error_without_touching_state() {
        use rigspark_core::catalog::{AdvisoryReason, Availability};
        let directory = tempfile::tempdir().unwrap();
        let host = Host::new(directory.path(), 4000).unwrap();
        let config = Config::from_home(directory.path()).unwrap();
        std::fs::write(&config.state, "invalid state").unwrap();
        let mut catalog = Catalog::parse(rigspark_core::MODELS_JSON).unwrap();
        catalog.models.truncate(1);
        catalog.models[0].availability = Some(Availability::AdvisoryOnly {
            reason: AdvisoryReason::BackendFormatUnsupported,
        });
        let request = Request::builder()
            .method("POST")
            .uri("/api/models/up")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                json!({"model":catalog.models[0].id,"bypass":true}).to_string(),
            ))
            .unwrap();
        let response = activate_model(host, request, &catalog)
            .await
            .unwrap_or_else(|error| panic!("{}: {}", error.0, error.1));
        assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        assert!(
            std::str::from_utf8(&bytes)
                .unwrap()
                .contains("not yet installable")
        );
        assert_eq!(
            std::fs::read_to_string(&config.state).unwrap(),
            "invalid state"
        );
        assert!(!directory.path().join("cache").exists());
    }
}
