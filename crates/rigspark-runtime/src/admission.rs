//! Network side of automatic catalog admission: allow-listed fetches and the weekly run.

use crate::catalog_proposals::allowed_registry_redirect;
use futures_util::StreamExt;
use rigspark_core::{
    admission::{
        AdmissionInput, Gguf, GgufError, LibraryEntry, MAX_CONFIG_BYTES, MAX_GGUF_HEADER_BYTES,
        MAX_HTML_BYTES, MAX_LICENSE_HEAD_BYTES, RegistryConfig, build_entry, catalog_capabilities,
        observation, parse_gguf, parse_library, parse_tags, select_variants,
    },
    catalog::{Catalog, CatalogModel, EntryProvenance, SCHEMA_VERSION},
    registry_collector::{MANIFEST_ACCEPT, MAX_MANIFEST_BYTES, parse_layer, parse_reference},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    time::Duration,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio_util::sync::CancellationToken;
use url::Url;

pub const LIBRARY_URL: &str = "https://ollama.com/library?sort=newest";
pub const SCOPE_NAME: &str = "ollama-local-variants";
const GGUF_STEPS: [usize; 3] = [8 * 1024 * 1024, 32 * 1024 * 1024, MAX_GGUF_HEADER_BYTES];

fn name(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 128
        && part.as_bytes()[0].is_ascii_alphanumeric()
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// Only the listing, tags pages, manifests and blobs of official `library/` models.
pub fn allowed_request(url: &Url) -> bool {
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let segments: Vec<&str> = url.path().trim_start_matches('/').split('/').collect();
    match (url.host_str(), url.query(), segments.as_slice()) {
        (Some("ollama.com"), Some("sort=newest"), ["library"]) => true,
        (Some("ollama.com"), None, ["library", repo, "tags"]) => name(repo),
        (Some("registry.ollama.ai"), None, ["v2", "library", repo, "manifests", tag]) => {
            name(repo) && name(tag)
        }
        (Some("registry.ollama.ai"), None, ["v2", "library", repo, "blobs", digest]) => {
            name(repo)
                && digest.strip_prefix("sha256:").is_some_and(|hex| {
                    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
                })
        }
        _ => false,
    }
}

pub struct Fetched {
    pub status: u16,
    pub body: Vec<u8>,
}

#[async_trait::async_trait]
pub trait AdmissionTransport: Send + Sync {
    /// GETs an allow-listed URL. `range` asks for the first `range` bytes; `limit` caps the body.
    async fn fetch(
        &self,
        url: &Url,
        accept: Option<&'static str>,
        range: Option<usize>,
        limit: usize,
    ) -> io::Result<Fetched>;
}

pub struct NativeAdmissionTransport {
    client: reqwest::Client,
}
impl NativeAdmissionTransport {
    pub fn new() -> io::Result<Self> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .user_agent(concat!(
                "rigspark-catalog-admission/",
                env!("CARGO_PKG_VERSION")
            ))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let from_blob = attempt
                    .previous()
                    .last()
                    .is_some_and(|url| url.path().contains("/blobs/sha256:"));
                if attempt.previous().len() > 2
                    || !from_blob
                    || !allowed_registry_redirect(attempt.url().as_str())
                {
                    attempt.error("registry redirect refused")
                } else {
                    attempt.follow()
                }
            }))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|_| io::Error::other("admission client initialization failed"))?;
        Ok(Self { client })
    }
}
#[async_trait::async_trait]
impl AdmissionTransport for NativeAdmissionTransport {
    async fn fetch(
        &self,
        url: &Url,
        accept: Option<&'static str>,
        range: Option<usize>,
        limit: usize,
    ) -> io::Result<Fetched> {
        if !allowed_request(url) {
            return Err(io::Error::other("admission URL is not allow-listed"));
        }
        let mut request = self.client.get(url.clone());
        if let Some(accept) = accept {
            request = request.header(reqwest::header::ACCEPT, accept);
        }
        if let Some(range) = range {
            request = request.header(reqwest::header::RANGE, format!("bytes=0-{}", range - 1));
        }
        let response = request
            .send()
            .await
            .map_err(|_| io::Error::other("admission request failed"))?;
        let status = response.status().as_u16();
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| io::Error::other("admission body read failed"))?;
            body.extend_from_slice(&chunk);
            if body.len() >= limit {
                // A range read may stop at the cap; anything else larger than the cap is refused.
                if range.is_none() && body.len() > limit {
                    return Err(io::Error::other("admission response exceeds its size cap"));
                }
                body.truncate(limit);
                break;
            }
        }
        Ok(Fetched { status, body })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedBody {
    status: u16,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    json: Option<Value>,
    #[serde(default)]
    hex: Option<String>,
}

/// Replays recorded responses keyed by URL; unknown URLs answer 404.
pub struct RecordedAdmissionTransport {
    responses: BTreeMap<String, (u16, Vec<u8>)>,
}
impl RecordedAdmissionTransport {
    pub fn new(responses: BTreeMap<String, (u16, Vec<u8>)>) -> io::Result<Self> {
        for key in responses.keys() {
            let url = Url::parse(key).map_err(io::Error::other)?;
            if !allowed_request(&url) {
                return Err(io::Error::other("recorded URL is not allow-listed"));
            }
        }
        Ok(Self { responses })
    }
    pub fn parse(raw: &str) -> io::Result<Self> {
        let recorded: BTreeMap<String, RecordedBody> =
            serde_json::from_str(raw).map_err(io::Error::other)?;
        let mut responses = BTreeMap::new();
        for (url, body) in recorded {
            let bytes = match (body.text, body.json, body.hex) {
                (Some(text), None, None) => text.into_bytes(),
                (None, Some(value), None) => {
                    serde_json::to_vec(&value).map_err(io::Error::other)?
                }
                (None, None, Some(hex)) => decode_hex(&hex)?,
                (None, None, None) => Vec::new(),
                _ => return Err(io::Error::other("recorded body must have one encoding")),
            };
            responses.insert(url, (body.status, bytes));
        }
        Self::new(responses)
    }
}
fn decode_hex(hex: &str) -> io::Result<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return Err(io::Error::other("invalid recorded hex body"));
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|_| io::Error::other("invalid recorded hex body"))
        })
        .collect()
}
#[async_trait::async_trait]
impl AdmissionTransport for RecordedAdmissionTransport {
    async fn fetch(
        &self,
        url: &Url,
        _accept: Option<&'static str>,
        range: Option<usize>,
        limit: usize,
    ) -> io::Result<Fetched> {
        if !allowed_request(url) {
            return Err(io::Error::other("admission URL is not allow-listed"));
        }
        let (status, body) = self
            .responses
            .get(url.as_str())
            .cloned()
            .unwrap_or((404, Vec::new()));
        let cap = range.unwrap_or(usize::MAX).min(limit);
        if range.is_none() && body.len() > limit {
            return Err(io::Error::other("admission response exceeds its size cap"));
        }
        Ok(Fetched {
            status,
            body: body[..body.len().min(cap)].to_vec(),
        })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestLayers {
    config: Descriptor,
    layers: Vec<Descriptor>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Descriptor {
    media_type: String,
    digest: String,
}
fn sha256(digest: &str) -> Option<&str> {
    digest
        .strip_prefix("sha256:")
        .filter(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionState {
    /// `"<model sha256>:<license sha256 or none>"` → rejection reason, so ineligible weights
    /// are not re-downloaded every week.
    pub rejections: BTreeMap<String, String>,
}

pub struct AdmissionOptions {
    pub now: String,
    pub max_new_variants: usize,
    pub state: AdmissionState,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdmissionOutcome {
    #[serde(skip)]
    pub catalog: Option<Catalog>,
    #[serde(skip)]
    pub observations: Vec<Value>,
    #[serde(skip)]
    pub scope: Value,
    #[serde(skip)]
    pub state: AdmissionState,
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub reverified: Vec<String>,
    pub removed: Vec<String>,
    pub rejected: BTreeMap<String, String>,
    pub deferred: Vec<String>,
    pub failures: Vec<String>,
    pub complete: bool,
}

struct Run<'a> {
    transport: &'a dyn AdmissionTransport,
    cancel: &'a CancellationToken,
}
impl Run<'_> {
    async fn get(
        &self,
        url: &str,
        accept: Option<&'static str>,
        range: Option<usize>,
        limit: usize,
    ) -> io::Result<Vec<u8>> {
        if self.cancel.is_cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "catalog admission cancelled",
            ));
        }
        let url = Url::parse(url).map_err(io::Error::other)?;
        let fetched = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "catalog admission cancelled"));
            }
            result = tokio::time::timeout(Duration::from_secs(180), self.transport.fetch(&url, accept, range, limit)) => {
                result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "admission request timed out"))??
            }
        };
        let ok = if range.is_some() {
            matches!(fetched.status, 200 | 206)
        } else {
            (200..300).contains(&fetched.status)
        };
        if !ok {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("upstream status {}", fetched.status),
            ));
        }
        Ok(fetched.body)
    }
    async fn text(&self, url: &str, limit: usize) -> io::Result<String> {
        String::from_utf8(self.get(url, None, None, limit).await?)
            .map_err(|_| io::Error::other("upstream text is not UTF-8"))
    }
    /// `Ok(Err(reason))` is a content rejection (cacheable); `Err` is a transient failure.
    async fn gguf(&self, blob: &str) -> io::Result<Result<Gguf, &'static str>> {
        for step in GGUF_STEPS {
            let bytes = self.get(blob, None, Some(step), step).await?;
            match parse_gguf(&bytes) {
                Ok(parsed) => return Ok(Ok(parsed)),
                Err(GgufError::Truncated) if bytes.len() >= step => continue,
                Err(GgufError::Truncated) => return Ok(Err("GGUF header truncated")),
                Err(GgufError::Invalid(reason)) => return Ok(Err(reason)),
            }
        }
        Ok(Err("GGUF header exceeds 64 MiB"))
    }
}

fn interrupted(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Interrupted
}

/// One admission pass: discover, source, merge and produce evidence. Curated entries are
/// never modified; a failed listing aborts without changes.
pub async fn admit(
    catalog: &Catalog,
    transport: &dyn AdmissionTransport,
    options: &AdmissionOptions,
    cancel: &CancellationToken,
) -> io::Result<AdmissionOutcome> {
    let now = OffsetDateTime::parse(&options.now, &Rfc3339)
        .map_err(|_| io::Error::other("invalid admission clock"))?
        .to_offset(time::UtcOffset::UTC);
    let stamp = now
        .format(time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .map_err(io::Error::other)?;
    let today = stamp[..10].to_string();
    let run = Run { transport, cancel };
    let library =
        parse_library(&run.text(LIBRARY_URL, MAX_HTML_BYTES).await?).map_err(io::Error::other)?;

    let mut outcome = AdmissionOutcome {
        complete: true,
        state: options.state.clone(),
        ..Default::default()
    };
    let curated: BTreeMap<&str, &CatalogModel> = catalog
        .models
        .iter()
        .filter(|model| model.provenance == EntryProvenance::Curated)
        .map(|model| (model.id.as_str(), model))
        .collect();
    let curated_digests: BTreeSet<&str> = curated
        .values()
        .flat_map(|model| model.quantizations.iter())
        .filter_map(|quant| quant.sha256.as_deref())
        .collect();
    let mut auto: BTreeMap<String, CatalogModel> = catalog
        .models
        .iter()
        .filter(|model| model.provenance == EntryProvenance::Auto)
        .map(|model| (model.id.clone(), model.clone()))
        .collect();
    let listed: BTreeSet<&str> = library
        .iter()
        .map(|entry| entry.repository.as_str())
        .collect();
    for id in auto.keys().cloned().collect::<Vec<_>>() {
        if !listed.contains(parse_reference(&id).0) {
            auto.remove(&id);
            outcome.removed.push(id);
        }
    }
    let mut eligible: BTreeSet<String> = BTreeSet::new();
    let mut seen_digests: BTreeSet<String> = BTreeSet::new();
    let mut new_variants = 0;

    for entry in &library {
        let repo = entry.repository.as_str();
        let tags_url = format!("https://ollama.com/library/{repo}/tags");
        let selected = match run.text(&tags_url, MAX_HTML_BYTES).await {
            Ok(html) => match parse_tags(&html, repo) {
                Ok(tags) => select_variants(&tags),
                Err(_) => {
                    outcome
                        .failures
                        .push(format!("{repo}: tags page unparseable"));
                    outcome.complete = false;
                    continue;
                }
            },
            Err(error) if interrupted(&error) => return Err(error),
            Err(_) => {
                outcome
                    .failures
                    .push(format!("{repo}: tags page unavailable"));
                outcome.complete = false;
                continue;
            }
        };
        let selected_ids: BTreeSet<String> =
            selected.iter().map(|tag| format!("{repo}:{tag}")).collect();
        for id in auto.keys().cloned().collect::<Vec<_>>() {
            if parse_reference(&id).0 == repo && !selected_ids.contains(&id) {
                auto.remove(&id);
                outcome.removed.push(id);
            }
        }
        for tag in &selected {
            let id = format!("{repo}:{tag}");
            if curated.contains_key(id.as_str()) {
                continue;
            }
            match admit_variant(
                &run,
                entry,
                tag,
                &id,
                &auto,
                &curated_digests,
                &mut seen_digests,
                &today,
                &mut new_variants,
                options,
                &mut outcome,
            )
            .await
            {
                Ok(Some(model)) => {
                    eligible.insert(id.clone());
                    auto.insert(id, model);
                }
                Ok(None) => {}
                Err(error) if interrupted(&error) => return Err(error),
                Err(error) => {
                    outcome.failures.push(format!("{id}: {error}"));
                    outcome.complete = false;
                    if auto.contains_key(&id) {
                        eligible.insert(id);
                    }
                }
            }
        }
    }
    for id in &outcome.deferred {
        eligible.insert(id.clone());
    }

    let mut models: Vec<CatalogModel> = catalog
        .models
        .iter()
        .filter(|model| model.provenance == EntryProvenance::Curated)
        .cloned()
        .chain(auto.into_values())
        .collect();
    models.sort_by(|left, right| {
        right
            .recency()
            .map(|(day, _)| day)
            .cmp(&left.recency().map(|(day, _)| day))
            .then_with(|| left.id.cmp(&right.id))
    });
    outcome.observations = models
        .iter()
        .filter(|model| model.provenance == EntryProvenance::Auto)
        .map(|model| observation(model, &stamp))
        .collect();
    let mut variants: BTreeSet<String> = models.iter().map(|model| model.id.clone()).collect();
    variants.extend(eligible);
    outcome.scope = json!({
        "name": SCOPE_NAME,
        "checkedAt": stamp,
        "source": "https://ollama.com/library",
        "complete": outcome.complete,
        "variants": variants,
    });
    let changed =
        !outcome.added.is_empty() || !outcome.updated.is_empty() || !outcome.removed.is_empty();
    let encoded = json!({
        "schemaVersion": SCHEMA_VERSION,
        "generatedAt": if changed { stamp.clone() } else { catalog.generated_at.clone() },
        "models": models,
    });
    outcome.catalog = Some(Catalog::parse(&encoded.to_string()).map_err(io::Error::other)?);
    outcome.removed.sort();
    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
async fn admit_variant(
    run: &Run<'_>,
    entry: &LibraryEntry,
    tag: &str,
    id: &str,
    auto: &BTreeMap<String, CatalogModel>,
    curated_digests: &BTreeSet<&str>,
    seen_digests: &mut BTreeSet<String>,
    today: &str,
    new_variants: &mut usize,
    options: &AdmissionOptions,
    outcome: &mut AdmissionOutcome,
) -> io::Result<Option<CatalogModel>> {
    let repo = entry.repository.as_str();
    let blob =
        |digest: &str| format!("https://registry.ollama.ai/v2/library/{repo}/blobs/{digest}");
    let manifest_raw = run
        .get(
            &format!("https://registry.ollama.ai/v2/library/{repo}/manifests/{tag}"),
            Some(MANIFEST_ACCEPT),
            None,
            MAX_MANIFEST_BYTES,
        )
        .await?;
    let manifest_text = std::str::from_utf8(&manifest_raw)
        .map_err(|_| io::Error::other("manifest is not UTF-8"))?;
    let Some(layer) = parse_layer(manifest_text).map_err(io::Error::other)? else {
        outcome
            .rejected
            .insert(id.into(), "no GGUF model layer".into());
        return Ok(None);
    };
    let descriptors: ManifestLayers =
        serde_json::from_str(manifest_text).map_err(io::Error::other)?;
    if curated_digests.contains(layer.sha256.as_str()) || !seen_digests.insert(layer.sha256.clone())
    {
        return Ok(None);
    }
    let license_digest = descriptors
        .layers
        .iter()
        .find(|layer| layer.media_type == "application/vnd.ollama.image.license")
        .and_then(|layer| sha256(&layer.digest))
        .map(str::to_string);
    let cache_key = format!(
        "{}:{}",
        layer.sha256,
        license_digest.as_deref().unwrap_or("none")
    );
    let prior = auto.get(id);
    if let Some(prior) = prior
        && prior.quantizations[0].sha256.as_deref() == Some(layer.sha256.as_str())
        && prior.quantizations[0].disk_bytes == layer.disk_bytes
        && !outcome.state.rejections.contains_key(&cache_key)
    {
        let mut refreshed = prior.clone();
        let capabilities = catalog_capabilities(&entry.capabilities);
        if capabilities != prior.capabilities {
            refreshed.capabilities = capabilities;
            outcome.updated.push(id.into());
        } else {
            outcome.reverified.push(id.into());
        }
        return Ok(Some(refreshed));
    }
    if let Some(reason) = outcome.state.rejections.get(&cache_key) {
        outcome.rejected.insert(id.into(), reason.clone());
        return Ok(None);
    }
    if prior.is_none() {
        if *new_variants >= options.max_new_variants {
            outcome.deferred.push(id.into());
            outcome.complete = false;
            return Ok(None);
        }
        *new_variants += 1;
    }
    let config_digest = sha256(&descriptors.config.digest)
        .ok_or_else(|| io::Error::other("invalid config digest"))?;
    let config_raw = run
        .get(
            &blob(&format!("sha256:{config_digest}")),
            None,
            None,
            MAX_CONFIG_BYTES,
        )
        .await?;
    let config = RegistryConfig::parse(
        std::str::from_utf8(&config_raw).map_err(|_| io::Error::other("config is not UTF-8"))?,
    );
    let license_head = match &license_digest {
        Some(digest) => Some(
            String::from_utf8_lossy(
                &run.get(
                    &blob(&format!("sha256:{digest}")),
                    None,
                    Some(MAX_LICENSE_HEAD_BYTES),
                    MAX_LICENSE_HEAD_BYTES,
                )
                .await?,
            )
            .into_owned(),
        ),
        None => None,
    };
    let reject = |outcome: &mut AdmissionOutcome, reason: &str| {
        outcome.rejected.insert(id.into(), reason.into());
        outcome
            .state
            .rejections
            .insert(cache_key.clone(), reason.into());
        Ok(None)
    };
    let Ok(config) = config else {
        return reject(outcome, "model config lacks family, size or file type");
    };
    let gguf = match run.gguf(&blob(&format!("sha256:{}", layer.sha256))).await? {
        Ok(gguf) => gguf,
        Err(reason) => return reject(outcome, reason),
    };
    let built = build_entry(&AdmissionInput {
        repository: repo,
        tag,
        capabilities: &entry.capabilities,
        layer: &layer,
        config: &config,
        gguf: &gguf,
        license_head: license_head.as_deref(),
        today: prior
            .and_then(|model| model.added_at.as_deref())
            .unwrap_or(today),
    });
    match built {
        Ok(model) => {
            outcome.state.rejections.remove(&cache_key);
            if prior.is_some() {
                outcome.updated.push(id.into());
            } else {
                outcome.added.push(id.into());
            }
            Ok(Some(model))
        }
        Err(rejection) => reject(outcome, rejection.reason()),
    }
}
