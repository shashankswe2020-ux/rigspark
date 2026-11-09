//! Loopback API for local voice: the voice catalog, starting or attaching a speak and a
//! listen server, text to speech and speech to text.
use crate::{
    Host,
    routes::{ApiError, ApiResult, bad, body, json_response},
};
use axum::{
    body::to_bytes,
    extract::Request,
    http::{StatusCode, header},
    response::IntoResponse,
};
use rigspark_core::voice::{SizeBasis, VoiceCatalog, VoiceEngine, VoiceKind, fit};
use rigspark_runtime::voice::{MAX_AUDIO_BYTES, SpeakRequest, probe_engine_in};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRequest {
    model: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KindRequest {
    kind: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachRequest {
    kind: String,
    endpoint: String,
    model: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpeakBody {
    text: String,
    voice: Option<String>,
    speed: Option<f64>,
}

fn authorized(host: &Host, request: &Request) -> Result<(), ApiError> {
    if request
        .headers()
        .get("x-llmup-token")
        .and_then(|value| value.to_str().ok())
        != Some(host.token.as_str())
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "missing or invalid capability token",
        ));
    }
    Ok(())
}

fn kind(value: &str) -> Result<VoiceKind, ApiError> {
    VoiceKind::parse(value).ok_or_else(bad)
}

fn failure(status: StatusCode, message: &str) -> axum::response::Response {
    crate::error(status, message)
}

async fn engines_and_status(host: &Host) -> Result<(Value, Value), ApiError> {
    let (hardware, _) = rigspark_runtime::hardware::detect()
        .await
        .map_err(|_| bad())?;
    let engines: Vec<_> = [VoiceEngine::WhisperCpp, VoiceEngine::MlxAudio]
        .into_iter()
        .map(|engine| probe_engine_in(engine, &hardware, Some(host.engines.root())))
        .collect();
    Ok((
        serde_json::to_value(engines).map_err(|_| bad())?,
        serde_json::to_value(host.voice.status()).map_err(|_| bad())?,
    ))
}

async fn models(host: &Host) -> ApiResult {
    let catalog = VoiceCatalog::bundled().map_err(|_| bad())?;
    let (hardware, _) = rigspark_runtime::hardware::detect()
        .await
        .map_err(|_| bad())?;
    let models: Vec<_> = catalog
        .models
        .iter()
        .map(|model| {
            json!({
                "id": model.id,
                "kind": model.kind,
                "engine": model.engine,
                "params": model.params,
                "license": model.license,
                "releaseDate": model.release_date,
                "default": model.default,
                "source": model.source,
                "engineModel": model.engine_model,
                "weightsBytes": model.weights_bytes,
                "sizeBasis": model.size_basis,
                "integrity": if model.size_basis == SizeBasis::ModelCard {
                    "downloaded by the engine; not verified by RigSpark"
                } else {
                    "SHA-256 pinned and verified before loading"
                },
                "languages": model.languages,
                "voices": model.voices,
                "fit": fit(model, &hardware),
            })
        })
        .collect();
    let (engines, status) = engines_and_status(host).await?;
    Ok(json_response(
        json!({"models": models, "engines": engines, "status": status}),
    ))
}

async fn speak(host: Arc<Host>, request: Request) -> ApiResult {
    authorized(&host, &request)?;
    let input: SpeakBody = body(request, crate::MAX_REQUEST_BYTES).await?;
    let catalog = VoiceCatalog::bundled().map_err(|_| bad())?;
    let cancel = host.shutdown.child_token();
    match host
        .voice
        .speak(
            &SpeakRequest {
                text: input.text,
                voice: input.voice,
                speed: input.speed,
            },
            &catalog,
            &cancel,
        )
        .await
    {
        Ok(audio) => Ok((
            [
                (header::CONTENT_TYPE, "audio/wav"),
                (header::CONTENT_DISPOSITION, "inline"),
            ],
            audio,
        )
            .into_response()),
        Err(error) => Ok(failure(StatusCode::BAD_GATEWAY, &error.0)),
    }
}

async fn transcribe(host: Arc<Host>, request: Request) -> ApiResult {
    authorized(&host, &request)?;
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("audio/wav")
    {
        return Err(ApiError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "audio/wav required",
        ));
    }
    let language = request
        .headers()
        .get("x-voice-language")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let audio = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        to_bytes(request.into_body(), MAX_AUDIO_BYTES),
    )
    .await
    .map_err(|_| ApiError(StatusCode::REQUEST_TIMEOUT, "body timeout"))?
    .map_err(|_| ApiError(StatusCode::PAYLOAD_TOO_LARGE, "audio exceeds 48 MiB"))?;
    let cancel = host.shutdown.child_token();
    match host
        .voice
        .transcribe(&audio, language.as_deref(), &cancel)
        .await
    {
        Ok(text) => Ok(json_response(json!({"text": text}))),
        Err(error) => Ok(failure(StatusCode::BAD_GATEWAY, &error.0)),
    }
}

pub async fn dispatch(host: Arc<Host>, request: Request) -> ApiResult {
    if request.uri().query().is_some() {
        return Err(bad());
    }
    let method = request.method().as_str().to_owned();
    let path = request.uri().path().to_owned();
    match (method.as_str(), path.as_str()) {
        ("GET", "/api/voice/models") => models(&host).await,
        ("GET", "/api/voice/status") => {
            let (engines, status) = engines_and_status(&host).await?;
            Ok(json_response(json!({"engines": engines, "status": status})))
        }
        ("POST", "/api/voice/start") => {
            authorized(&host, &request)?;
            let input: StartRequest = body(request, crate::MAX_REQUEST_BYTES).await?;
            let catalog = VoiceCatalog::bundled().map_err(|_| bad())?;
            let model = catalog
                .models
                .iter()
                .find(|model| model.id == input.model)
                .ok_or_else(bad)?;
            let (hardware, _) = rigspark_runtime::hardware::detect()
                .await
                .map_err(|_| bad())?;
            match host.voice.start(model, &hardware, &host.shutdown) {
                Ok(()) => Ok((
                    StatusCode::ACCEPTED,
                    axum::Json(json!({"status": host.voice.status()})),
                )
                    .into_response()),
                Err(error) => Ok(failure(StatusCode::BAD_REQUEST, &error.0)),
            }
        }
        ("POST", "/api/voice/stop") => {
            authorized(&host, &request)?;
            let input: KindRequest = body(request, crate::MAX_REQUEST_BYTES).await?;
            let stopped = host.voice.stop(kind(&input.kind)?);
            Ok(json_response(
                json!({"stopped": stopped, "status": host.voice.status()}),
            ))
        }
        ("POST", "/api/voice/attach") => {
            authorized(&host, &request)?;
            let input: AttachRequest = body(request, crate::MAX_REQUEST_BYTES).await?;
            match host
                .voice
                .attach(
                    kind(&input.kind)?,
                    input.endpoint.trim(),
                    input.model.trim(),
                )
                .await
            {
                Ok(()) => Ok(json_response(json!({"status": host.voice.status()}))),
                Err(error) => Ok(failure(StatusCode::BAD_REQUEST, &error.0)),
            }
        }
        ("POST", "/api/voice/speak") => speak(host, request).await,
        ("POST", "/api/voice/transcribe") => transcribe(host, request).await,
        _ => Err(bad()),
    }
}
