use crate::{
    catalog::{PinnedFileSource, repository, require},
    sizing::ValidationError,
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

pub const MAX_INDEX_HTML_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_IDENTITY_RECORDS: usize = 10_000;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexRelease {
    pub slug: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexCreator {
    pub id: String,
    pub slug: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexModel {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub short_name: String,
    pub suffix: Option<String>,
    pub release: IndexRelease,
    pub release_date: Option<String>,
    pub deprecated: bool,
    pub is_reasoning: bool,
    pub is_open_weights: bool,
    pub creator: IndexCreator,
}

fn text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

impl IndexModel {
    fn validate(&self) -> Result<(), ValidationError> {
        require(
            text(&self.id)
                && slug(&self.slug)
                && text(&self.name)
                && text(&self.short_name)
                && self.suffix.as_deref().is_none_or(text)
                && slug(&self.release.slug)
                && text(&self.release.name)
                && text(&self.creator.id)
                && slug(&self.creator.slug)
                && text(&self.creator.name),
            "invalid Artificial Analysis model identity",
        )
    }
}

pub fn parse_index_html(raw: &str) -> Result<Vec<IndexModel>, ValidationError> {
    require(
        raw.len() <= MAX_INDEX_HTML_BYTES,
        "Artificial Analysis index exceeds 4 MiB",
    )?;
    static SCRIPT: OnceLock<Regex> = OnceLock::new();
    let script = SCRIPT.get_or_init(|| {
        Regex::new(r"(?s)<script(?:\s[^>]*)?>\s*self\.__next_f\.push\((.*?)\)\s*</script>")
            .expect("valid Artificial Analysis script pattern")
    });
    let mut payload = String::new();
    for captures in script.captures_iter(raw) {
        let value: Value = serde_json::from_str(&captures[1])
            .map_err(|error| ValidationError(error.to_string()))?;
        if let Some(chunk) = value
            .as_array()
            .and_then(|items| items.get(1))
            .and_then(Value::as_str)
        {
            payload.push_str(chunk);
        }
    }
    let marker = "\"initialModels\":";
    let start = payload
        .find(marker)
        .map(|index| index + marker.len())
        .ok_or_else(|| ValidationError("Artificial Analysis inventory missing".into()))?;
    let mut deserializer = serde_json::Deserializer::from_str(&payload[start..]);
    let models = Vec::<IndexModel>::deserialize(&mut deserializer)
        .map_err(|error| ValidationError(error.to_string()))?;
    require(!models.is_empty(), "Artificial Analysis inventory is empty")?;
    require(
        models.len() <= MAX_IDENTITY_RECORDS,
        "Artificial Analysis inventory has too many models",
    )?;
    for model in &models {
        model.validate()?;
    }
    Ok(models)
}

/// An explicit association, not a claim that the repository's license or artifacts are verified.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublisherMapping {
    pub creator_slug: String,
    pub release_slug: String,
    pub repo: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublisherMatch {
    pub creator_slug: String,
    pub release_slug: String,
    pub repo: String,
    pub model_ids: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct PublisherMatches {
    pub matched: Vec<PublisherMatch>,
    pub unmatched: Vec<String>,
    pub excluded: Vec<String>,
}

pub fn match_publishers(
    inventory: &[IndexModel],
    mappings: &[PublisherMapping],
) -> Result<PublisherMatches, ValidationError> {
    require(
        !inventory.is_empty()
            && inventory.len() <= MAX_IDENTITY_RECORDS
            && mappings.len() <= MAX_IDENTITY_RECORDS,
        "invalid Artificial Analysis identity inventory size",
    )?;
    let mut publishers = BTreeMap::new();
    for mapping in mappings {
        require(
            slug(&mapping.creator_slug) && slug(&mapping.release_slug),
            "invalid Artificial Analysis publisher mapping",
        )?;
        repository(&mapping.repo)?;
        let key = (mapping.creator_slug.as_str(), mapping.release_slug.as_str());
        if let Some(previous) = publishers.insert(key, mapping.repo.as_str()) {
            require(
                previous == mapping.repo,
                &format!("ambiguous publisher mapping: {}/{}", key.0, key.1),
            )?;
        }
    }
    let mut seen = BTreeSet::new();
    let mut matched: BTreeMap<(&str, &str), PublisherMatch> = BTreeMap::new();
    let mut unmatched = Vec::new();
    let mut excluded = Vec::new();
    for model in inventory {
        model.validate()?;
        require(
            seen.insert(&model.id),
            "duplicate Artificial Analysis model id",
        )?;
        if !model.is_open_weights {
            excluded.push(model.id.clone());
            continue;
        }
        let key = (model.creator.slug.as_str(), model.release.slug.as_str());
        let Some(repo) = publishers.get(&key) else {
            unmatched.push(model.id.clone());
            continue;
        };
        matched
            .entry(key)
            .or_insert_with(|| PublisherMatch {
                creator_slug: key.0.into(),
                release_slug: key.1.into(),
                repo: (*repo).into(),
                model_ids: Vec::new(),
            })
            .model_ids
            .push(model.id.clone());
    }
    for entry in matched.values_mut() {
        entry.model_ids.sort();
    }
    unmatched.sort();
    excluded.sort();
    Ok(PublisherMatches {
        matched: matched.into_values().collect(),
        unmatched,
        excluded,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactObservation {
    pub model_id: String,
    pub source: PinnedFileSource,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct WeightDigest {
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ArtifactIdentity {
    pub repo: String,
    pub revision: String,
    pub files: Vec<WeightDigest>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactGroup {
    pub identity: ArtifactIdentity,
    pub model_ids: Vec<String>,
}

/// Groups resolved weights, not names. License and publisher evidence still require verification.
pub fn artifact_groups(
    inventory: &[IndexModel],
    mappings: &[PublisherMapping],
    observations: &[ArtifactObservation],
) -> Result<Vec<ArtifactGroup>, ValidationError> {
    require(
        observations.len() <= MAX_IDENTITY_RECORDS,
        "too many artifact observations",
    )?;
    let matches = match_publishers(inventory, mappings)?;
    let publishers: BTreeMap<&str, &str> = matches
        .matched
        .iter()
        .flat_map(|entry| {
            entry
                .model_ids
                .iter()
                .map(|id| (id.as_str(), entry.repo.as_str()))
        })
        .collect();
    let mut sizes = BTreeMap::new();
    let mut groups: BTreeMap<ArtifactIdentity, BTreeSet<String>> = BTreeMap::new();
    for observation in observations {
        require(text(&observation.model_id), "invalid artifact model id")?;
        require(
            publishers.get(observation.model_id.as_str()).copied()
                == Some(observation.source.repo.as_str()),
            &format!(
                "artifact has no matching open-weight publisher: {}",
                observation.model_id
            ),
        )?;
        observation.source.validate_weights()?;
        let mut files = Vec::new();
        for file in &observation.source.files {
            let sha256 = file.sha256.to_ascii_lowercase();
            let bytes = file.bytes as u64;
            if let Some(previous) = sizes.insert(sha256.clone(), bytes) {
                require(
                    previous == bytes,
                    "conflicting sizes for the same weight digest",
                )?;
            }
            files.push(WeightDigest { sha256, bytes });
        }
        files.sort();
        let identity = ArtifactIdentity {
            repo: observation.source.repo.clone(),
            revision: observation.source.revision.to_ascii_lowercase(),
            files,
        };
        groups
            .entry(identity)
            .or_default()
            .insert(observation.model_id.clone());
    }
    Ok(groups
        .into_iter()
        .map(|(identity, model_ids)| ArtifactGroup {
            identity,
            model_ids: model_ids.into_iter().collect(),
        })
        .collect())
}
