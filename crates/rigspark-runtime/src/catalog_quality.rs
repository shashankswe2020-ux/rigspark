use crate::catalog_update::MAX_ARTIFACT_BYTES;
use rigspark_core::catalog::{Catalog, CatalogModel};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const MINIMUM_PERCENT: usize = 90;
pub const EVIDENCE_MAX_AGE_DAYS: i64 = 7;

#[derive(Debug, Error)]
#[error("invalid catalog quality evidence: {0}")]
pub struct QualityError(pub &'static str);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Evidence {
    policy_version: u8,
    scopes: Vec<Scope>,
    observations: Vec<Observation>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    name: String,
    checked_at: String,
    source: String,
    complete: bool,
    variants: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Observation {
    id: String,
    checked_at: String,
    sources: Vec<String>,
    model: CatalogModel,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryResult {
    id: String,
    checked_at: Option<String>,
    sources: Vec<String>,
    fresh: bool,
    verified: bool,
    reason: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityReport {
    pub policy_version: u8,
    pub evaluated_at: String,
    pub catalog_sha256: String,
    pub evidence_sha256: String,
    pub minimum_percent: usize,
    pub evidence_max_age_days: i64,
    pub catalog_entries: usize,
    pub fresh_entries: usize,
    pub verified_entries: usize,
    pub eligible_variants: usize,
    pub covered_variants: usize,
    pub coverage_percent: Option<f64>,
    pub recency_percent: f64,
    pub freshness_percent: Option<f64>,
    pub correctness_percent: f64,
    pub passed: bool,
    pub blockers: Vec<String>,
    pub missing_variants: Vec<String>,
    pub scopes: Vec<Scope>,
    pub entries: Vec<EntryResult>,
}

fn recent(raw: &str, now: OffsetDateTime) -> Result<bool, QualityError> {
    let checked =
        OffsetDateTime::parse(raw, &Rfc3339).map_err(|_| QualityError("invalid timestamp"))?;
    if checked > now {
        return Err(QualityError("future verification date"));
    }
    Ok(now - checked <= time::Duration::days(EVIDENCE_MAX_AGE_DAYS))
}

fn source_url(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn facts(model: &CatalogModel) -> Result<serde_json::Value, QualityError> {
    let mut value = serde_json::to_value(model).map_err(|_| QualityError("invalid model facts"))?;
    for quant in value["quantizations"].as_array_mut().unwrap() {
        for field in ["minRamBytes", "minVramBytes", "digestVerified"] {
            quant.as_object_mut().unwrap().remove(field);
        }
    }
    Ok(value)
}

fn citations_match(observation: &Observation) -> bool {
    let source = &observation.model.source;
    let cites_repo = |repo: &str| {
        let root = format!("https://huggingface.co/{repo}");
        observation
            .sources
            .iter()
            .any(|url| url == &root || url.starts_with(&format!("{root}/")))
    };
    if source.hf.as_deref().is_some_and(|repo| !cites_repo(repo))
        || source
            .gguf
            .as_ref()
            .is_some_and(|gguf| !cites_repo(&gguf.repo))
        || source
            .mlx
            .as_ref()
            .is_some_and(|mlx| !cites_repo(&mlx.repo))
    {
        return false;
    }
    if let Some(reference) = &source.ollama {
        let (repo, tag) = rigspark_core::registry_collector::parse_reference(reference);
        let repo = if repo.contains('/') {
            repo.to_string()
        } else {
            format!("library/{repo}")
        };
        let manifest = format!("https://registry.ollama.ai/v2/{repo}/manifests/{tag}");
        if !observation.sources.contains(&manifest) {
            return false;
        }
        if source.hf.is_none() {
            let page = format!("https://ollama.com/{repo}:{tag}");
            if !observation.sources.contains(&page) {
                return false;
            }
        }
    }
    source.hf.is_some() || source.ollama.is_some() || source.gguf.is_some() || source.mlx.is_some()
}

fn matches_facts(model: &CatalogModel, observed: &CatalogModel) -> Result<bool, QualityError> {
    let mut observed = observed.clone();
    if model.kv_bytes_per_token.is_none() {
        observed.kv_bytes_per_token = None;
    }
    if model.benchmark_proxy.is_none() {
        observed.benchmark_proxy = None;
    }
    Ok(facts(model)? == facts(&observed)?)
}

fn pinned(model: &CatalogModel) -> bool {
    if model.source.ollama.is_some() {
        model
            .quantizations
            .iter()
            .all(|quant| quant.sha256.is_some())
    } else {
        model.source.gguf.is_some() || model.source.mlx.is_some()
    }
}

fn percent(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 * 100.0 / denominator as f64
    }
}

pub fn evaluate(
    catalog_raw: &str,
    evidence_raw: &str,
    now: &str,
) -> Result<QualityReport, QualityError> {
    if catalog_raw.len() > MAX_ARTIFACT_BYTES || evidence_raw.len() > MAX_ARTIFACT_BYTES {
        return Err(QualityError("input exceeds 16 MiB"));
    }
    let catalog = Catalog::parse(catalog_raw).map_err(|_| QualityError("catalog schema"))?;
    if catalog.models.is_empty() {
        return Err(QualityError("empty catalog"));
    }
    let evidence: Evidence = rigspark_core::catalog::parse_document(evidence_raw)
        .map_err(|_| QualityError("evidence schema"))?;
    if evidence.policy_version != 1
        || evidence.scopes.len() > 32
        || evidence.observations.len() > 10000
    {
        return Err(QualityError("unsupported policy or collection limit"));
    }
    let clock =
        OffsetDateTime::parse(now, &Rfc3339).map_err(|_| QualityError("evaluation date"))?;
    recent(&catalog.generated_at, clock)?;
    let mut blockers = Vec::new();
    let mut inventory = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut inventory_known = !evidence.scopes.is_empty();
    for scope in &evidence.scopes {
        if scope.name.is_empty()
            || !names.insert(scope.name.clone())
            || !source_url(&scope.source)
            || scope.variants.len() > 10000
        {
            return Err(QualityError("invalid coverage scope"));
        }
        if !recent(&scope.checked_at, clock)? || !scope.complete || scope.variants.is_empty() {
            inventory_known = false;
        }
        for variant in &scope.variants {
            if variant.is_empty()
                || variant.len() > 256
                || variant.chars().any(char::is_control)
                || !inventory.insert(variant.clone())
            {
                return Err(QualityError("duplicate or invalid inventory variant"));
            }
        }
    }
    if !names.contains("ollama-local-variants") {
        inventory_known = false;
    }
    if !inventory_known {
        blockers.push(
            "Upstream variant inventory is missing, incomplete, empty, or older than 7 days."
                .into(),
        );
    }
    let models: BTreeMap<_, _> = catalog
        .models
        .iter()
        .map(|model| (model.id.as_str(), model))
        .collect();
    let mut observations = BTreeMap::new();
    for observation in &evidence.observations {
        if observation.id != observation.model.id
            || !models.contains_key(observation.id.as_str())
            || observations
                .insert(observation.id.as_str(), observation)
                .is_some()
            || observation.sources.is_empty()
            || observation.sources.len() > 16
            || !observation.sources.iter().all(|source| source_url(source))
            || !citations_match(observation)
        {
            return Err(QualityError("duplicate, unbound, or uncited observation"));
        }
        let observed_catalog = serde_json::json!({"schemaVersion":rigspark_core::catalog::SCHEMA_VERSION,"generatedAt":observation.checked_at,"models":[observation.model]});
        Catalog::parse(&observed_catalog.to_string())
            .map_err(|_| QualityError("observed model facts"))?;
        recent(&observation.checked_at, clock)?;
    }
    let mut entries = Vec::new();
    let mut fresh_entries = 0;
    let mut verified_entries = 0;
    for model in &catalog.models {
        let mut entry = EntryResult {
            id: model.id.clone(),
            checked_at: None,
            sources: Vec::new(),
            fresh: false,
            verified: false,
            reason: "missing evidence",
        };
        if inventory_known && !inventory.contains(&model.id) {
            blockers.push(format!(
                "{}: catalog entry excluded from declared coverage scope",
                model.id
            ));
        }
        if let Some(observation) = observations.get(model.id.as_str()) {
            entry.checked_at = Some(observation.checked_at.clone());
            entry.sources = observation.sources.clone();
            entry.fresh = recent(&observation.checked_at, clock)?;
            if !matches_facts(model, &observation.model)? {
                entry.reason = "authoritative facts contradict catalog";
                blockers.push(format!(
                    "{}: authoritative facts contradict catalog",
                    model.id
                ));
            } else if !pinned(model) {
                entry.reason = "required artifact integrity evidence missing";
            } else if !entry.fresh {
                entry.reason = "verification evidence older than 7 days";
            } else {
                entry.verified = true;
                entry.reason = "verified";
            }
        }
        fresh_entries += usize::from(entry.fresh);
        verified_entries += usize::from(entry.verified);
        entries.push(entry);
    }
    let missing_variants: Vec<_> = inventory
        .iter()
        .filter(|id| !models.contains_key(id.as_str()))
        .cloned()
        .collect();
    let covered_variants = inventory.len() - missing_variants.len();
    let coverage_percent = inventory_known.then(|| percent(covered_variants, inventory.len()));
    let recency_percent = percent(fresh_entries, catalog.models.len());
    let freshness_percent = coverage_percent.map(|coverage| coverage.min(recency_percent));
    let correctness_percent = percent(verified_entries, catalog.models.len());
    let passed = blockers.is_empty()
        && inventory_known
        && covered_variants * 100 >= inventory.len() * MINIMUM_PERCENT
        && fresh_entries * 100 >= catalog.models.len() * MINIMUM_PERCENT
        && verified_entries * 100 >= catalog.models.len() * MINIMUM_PERCENT;
    Ok(QualityReport {
        policy_version: 1,
        evaluated_at: now.into(),
        catalog_sha256: format!("{:x}", Sha256::digest(catalog_raw.as_bytes())),
        evidence_sha256: format!("{:x}", Sha256::digest(evidence_raw.as_bytes())),
        minimum_percent: MINIMUM_PERCENT,
        evidence_max_age_days: EVIDENCE_MAX_AGE_DAYS,
        catalog_entries: catalog.models.len(),
        fresh_entries,
        verified_entries,
        eligible_variants: inventory.len(),
        covered_variants,
        coverage_percent,
        recency_percent,
        freshness_percent,
        correctness_percent,
        passed,
        blockers,
        missing_variants,
        scopes: evidence.scopes,
        entries,
    })
}

pub fn sign_reviewed_catalog(
    payload: crate::catalog_update::CatalogPayload,
    evidence: &str,
    seed: &[u8; 32],
    public_key: &[u8; 32],
) -> Result<(Vec<u8>, Vec<u8>), QualityError> {
    use crate::catalog_update::{SignedCatalog, sign_catalog};
    use ed25519_dalek::{Signer, SigningKey};
    let report = evaluate(&payload.catalog, evidence, &payload.published_at)?;
    if !report.passed {
        return Err(QualityError(
            "both scores must be at least 90% with no hard blockers",
        ));
    }
    let revision = payload.revision;
    let artifact = sign_catalog(payload, seed, public_key)
        .map_err(|_| QualityError("catalog signing failed"))?;
    let report_payload = serde_json::json!({
        "reportType":"rigspark-catalog-quality", "reportVersion":1,
        "revision":revision, "catalogArtifactSha256":format!("{:x}", Sha256::digest(&artifact)),
        "quality":report, "evidence":evidence
    })
    .to_string();
    let signature = SigningKey::from_bytes(seed)
        .sign(report_payload.as_bytes())
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let quality = serde_json::to_vec(&SignedCatalog {
        payload: report_payload,
        signature,
    })
    .map_err(|_| QualityError("report encoding"))?;
    if quality.len() > MAX_ARTIFACT_BYTES {
        return Err(QualityError("signed quality report exceeds 16 MiB"));
    }
    Ok((artifact, quality))
}
