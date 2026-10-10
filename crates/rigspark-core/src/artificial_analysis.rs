use crate::{
    catalog::{PinnedFile, PinnedFileSource, repository, require},
    generation_admission::{HfModel, license_from_tags},
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
pub const INDEX_URL: &str =
    "https://artificialanalysis.ai/evaluations/artificial-analysis-intelligence-index";
pub mod coverage;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexRelease {
    pub slug: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IndexCreator {
    pub id: String,
    pub slug: String,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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

impl PublisherMapping {
    pub fn validate(&self) -> Result<(), ValidationError> {
        require(
            slug(&self.creator_slug) && slug(&self.release_slug),
            "invalid Artificial Analysis publisher mapping",
        )?;
        repository(&self.repo)
    }
}

#[derive(Debug, Serialize)]
pub struct PublisherArtifact {
    pub source: PinnedFileSource,
    pub license: String,
    pub format: WeightFormat,
    pub has_config: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WeightFormat {
    Safetensors,
    Gguf,
}

/// The caller must supply a reviewed official-publisher mapping and a complete export selection.
/// This verifies publisher-reported digests, not downloaded weight contents or backend support.
pub fn parse_publisher_artifact(
    mapping: &PublisherMapping,
    files: &[String],
    raw: &str,
) -> Result<PublisherArtifact, ValidationError> {
    mapping.validate()?;
    require(
        (1..=256).contains(&files.len()),
        "invalid export selection size",
    )?;
    let model = HfModel::parse(raw)?;
    #[derive(Deserialize)]
    struct Access {
        private: bool,
        gated: bool,
        #[serde(rename = "cardData")]
        card: Option<Card>,
    }
    #[derive(Deserialize)]
    struct Card {
        license: Option<String>,
    }
    let access: Access =
        serde_json::from_str(raw).map_err(|error| ValidationError(error.to_string()))?;
    require(
        model.id == mapping.repo && model.pinned() && !access.private && !access.gated,
        "publisher repository is not the mapped public pinned source",
    )?;
    let license = license_from_tags(&model.tags).ok_or_else(|| {
        ValidationError("missing, conflicting or unsupported publisher license".into())
    })?;
    if let Some(card_license) = access.card.and_then(|card| card.license) {
        require(
            card_license == license,
            "conflicting publisher license evidence",
        )?;
    }
    let mut siblings = BTreeMap::new();
    for sibling in &model.siblings {
        require(
            siblings
                .insert(sibling.rfilename.as_str(), sibling)
                .is_none(),
            "duplicate publisher file metadata",
        )?;
    }
    let has_config = siblings.contains_key("config.json");
    let mut source = PinnedFileSource {
        repo: model.id,
        revision: model.sha.to_ascii_lowercase(),
        files: Vec::new(),
    };
    let mut digest_sizes = BTreeMap::new();
    for path in files {
        let sibling = siblings
            .get(path.as_str())
            .ok_or_else(|| ValidationError("selected weight file is missing".into()))?;
        let lfs = sibling.lfs.as_ref().ok_or_else(|| {
            ValidationError("selected weight file or LFS evidence is missing".into())
        })?;
        require(
            sibling.size.is_none_or(|size| size == lfs.size),
            "conflicting publisher file size evidence",
        )?;
        if let Some(previous) = digest_sizes.insert(lfs.sha256.to_ascii_lowercase(), lfs.size) {
            require(previous == lfs.size, "conflicting publisher digest sizes")?;
        }
        source.files.push(PinnedFile {
            file: path.clone(),
            sha256: lfs.sha256.to_ascii_lowercase(),
            bytes: lfs.size as f64,
        });
    }
    source.validate_weights()?;
    require(
        source
            .files
            .iter()
            .all(|file| file.file.ends_with(".safetensors"))
            || source.files.iter().all(|file| file.file.ends_with(".gguf")),
        "publisher export mixes weight formats",
    )?;
    validate_export_shards(&source)?;
    source
        .files
        .sort_by(|left, right| left.file.cmp(&right.file));
    let format = if source.files[0].file.ends_with(".safetensors") {
        WeightFormat::Safetensors
    } else {
        WeightFormat::Gguf
    };
    Ok(PublisherArtifact {
        source,
        license: license.into(),
        format,
        has_config,
    })
}

fn validate_export_shards(source: &PinnedFileSource) -> Result<(), ValidationError> {
    static SHARD: OnceLock<Regex> = OnceLock::new();
    let shard = SHARD.get_or_init(|| {
        Regex::new(r"^(.*)-([0-9]{5})-of-([0-9]{5})\.(safetensors|gguf)$")
            .expect("valid weight shard pattern")
    });
    let mut groups: BTreeMap<(String, String), (usize, BTreeSet<usize>)> = BTreeMap::new();
    for file in &source.files {
        if let Some(parts) = shard.captures(&file.file) {
            let index = parts[2].parse::<usize>().expect("five digits");
            let count = parts[3].parse::<usize>().expect("five digits");
            let (expected, indices) = groups
                .entry((parts[1].into(), parts[4].into()))
                .or_insert_with(|| (count, BTreeSet::new()));
            require(
                count > 0
                    && count <= 256
                    && count == *expected
                    && index > 0
                    && index <= count
                    && indices.insert(index),
                "invalid or conflicting publisher weight shards",
            )?;
        }
    }
    require(
        groups
            .values()
            .all(|(count, indices)| *count == indices.len()),
        "incomplete publisher weight shard selection",
    )
}

#[derive(Debug, Deserialize)]
pub struct PublisherConfig {
    pub model_type: Option<String>,
    #[serde(rename = "max_position_embeddings")]
    pub context_length: Option<u64>,
}
impl PublisherConfig {
    pub const MAX_BYTES: usize = 64 * 1024;

    pub fn parse(raw: &[u8]) -> Result<Self, ValidationError> {
        require(
            raw.len() <= Self::MAX_BYTES,
            "publisher configuration exceeds 64 KiB",
        )?;
        let config: Self =
            serde_json::from_slice(raw).map_err(|error| ValidationError(error.to_string()))?;
        require(
            config.model_type.as_deref().is_none_or(text)
                && config.context_length.is_none_or(|value| value > 0),
            "invalid publisher configuration facts",
        )?;
        Ok(config)
    }
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
        mapping.validate()?;
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
        let identity = artifact_identity(&observation.source)?;
        for file in &identity.files {
            if let Some(previous) = sizes.insert(file.sha256.clone(), file.bytes) {
                require(
                    previous == file.bytes,
                    "conflicting sizes for the same weight digest",
                )?;
            }
        }
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

fn artifact_identity(source: &PinnedFileSource) -> Result<ArtifactIdentity, ValidationError> {
    source.validate_weights()?;
    let mut files: Vec<_> = source
        .files
        .iter()
        .map(|file| WeightDigest {
            sha256: file.sha256.to_ascii_lowercase(),
            bytes: file.bytes as u64,
        })
        .collect();
    files.sort();
    Ok(ArtifactIdentity {
        repo: source.repo.clone(),
        revision: source.revision.to_ascii_lowercase(),
        files,
    })
}
