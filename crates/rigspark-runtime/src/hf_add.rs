//! Add a GGUF model from Hugging Face to the user's local catalog: search, inspect a
//! repository's single-file quantizations, and build a pinned entry. Network use is
//! explicit and limited to allow-listed Hugging Face URLs; advice stays offline.

use crate::{
    admission::AdmissionTransport,
    catalog_request::{SelectedGguf, build_entry, get, gguf, today},
};
use rigspark_core::{
    catalog::{CatalogModel, EntryProvenance},
    catalog_request::Repository,
    generation_admission::{HfModel, MAX_METADATA_BYTES, license_from_tags},
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use url::Url;

const SEARCH_LIMIT: &str = "20";
const MAX_SEARCH_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HfAddError {
    #[error("{0}")]
    Reference(&'static str),
    #[error("the repository must be public, ungated and pinned to a commit")]
    Unpinned,
    #[error("the repository is not a text-generation model")]
    Pipeline,
    #[error("the repository does not declare one recognized open-weight license")]
    License,
    #[error("the repository has no single-file GGUF RigSpark can run")]
    NoGguf,
    #[error("{0} is not one of this repository's quantizations")]
    UnknownQuant(String),
    #[error("the GGUF cannot be added: {0}")]
    Metadata(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GgufChoice {
    pub file: String,
    pub quant: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inspection {
    pub repository: String,
    pub revision: String,
    pub license: &'static str,
    pub choices: Vec<GgufChoice>,
    #[serde(skip)]
    model: HfModel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
}

/// `owner/repo`, `https://huggingface.co/owner/repo` or `hf.co/owner/repo`, each with an
/// optional `:QUANT` suffix (returned upper-cased).
pub fn parse_reference(raw: &str) -> Result<(Repository, Option<String>), HfAddError> {
    let raw = raw.trim();
    let path = raw
        .strip_prefix("https://huggingface.co/")
        .or_else(|| raw.strip_prefix("hf.co/"))
        .or_else(|| raw.strip_prefix("huggingface.co/"))
        .unwrap_or(raw);
    let (path, quant) = match path.split_once(':') {
        Some((path, quant)) => {
            if quant.is_empty()
                || quant.len() > 32
                || !quant
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(HfAddError::Reference("invalid quantization name"));
            }
            (path, Some(quant.to_ascii_uppercase()))
        }
        None => (path, None),
    };
    // URL parsing would normalize dot segments away, so refuse them first.
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(HfAddError::Reference(
            "use owner/repository or a Hugging Face URL",
        ));
    }
    let repository = Repository::parse(&format!("https://huggingface.co/{path}"))
        .map_err(|_| HfAddError::Reference("use owner/repository or a Hugging Face URL"))?;
    Ok((repository, quant))
}

/// The quantization named in a GGUF file name, if RigSpark can size it.
fn quant_of(file: &str) -> Option<String> {
    let stem = file
        .strip_suffix(".gguf")
        .or_else(|| file.strip_suffix(".GGUF"))?;
    let upper = stem.to_ascii_uppercase();
    upper
        .rsplit(['-', '.'])
        .map(|token| token.trim_start_matches("UD_"))
        .find(|token| {
            let known = [
                "IQ1_", "IQ2_", "IQ3_", "IQ4_", "Q2_", "Q3_", "Q4_", "Q5_", "Q6_", "Q8_",
            ]
            .iter()
            .any(|prefix| token.starts_with(prefix))
                || ["F16", "BF16", "F32", "MXFP4"].contains(token);
            known
                && token.len() <= 16
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        .map(str::to_owned)
}

pub async fn inspect(
    repository: &Repository,
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
) -> Result<Inspection, HfAddError> {
    let url = format!(
        "https://huggingface.co/api/models/{}?blobs=true",
        repository.as_str()
    );
    let bytes = get(transport, cancel, &url, None, MAX_METADATA_BYTES).await?;
    let model = HfModel::parse(
        std::str::from_utf8(&bytes)
            .map_err(|_| HfAddError::Metadata("metadata is not UTF-8".into()))?,
    )
    .map_err(|error| HfAddError::Metadata(error.0))?;
    if !model.id.eq_ignore_ascii_case(repository.as_str()) || !model.pinned() {
        return Err(HfAddError::Unpinned);
    }
    if model
        .pipeline_tag
        .as_deref()
        .is_some_and(|tag| tag != "text-generation")
    {
        return Err(HfAddError::Pipeline);
    }
    let license = license_from_tags(&model.tags).ok_or(HfAddError::License)?;
    let mut choices: Vec<GgufChoice> = model
        .siblings
        .iter()
        .filter(|sibling| {
            let lower = sibling.rfilename.to_ascii_lowercase();
            !lower.contains("mmproj") && !lower.contains("-of-") && !sibling.rfilename.contains('/')
        })
        .filter_map(|sibling| {
            let artifact = sibling.lfs.as_ref()?;
            let quant = quant_of(&sibling.rfilename)?;
            (artifact.size > 0
                && artifact.size <= 9_007_199_254_740_991
                && artifact.sha256.len() == 64
                && artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .then(|| GgufChoice {
                file: sibling.rfilename.clone(),
                quant,
                bytes: artifact.size,
                sha256: artifact.sha256.to_ascii_lowercase(),
            })
        })
        .collect();
    choices.sort_by(|left, right| {
        left.bytes
            .cmp(&right.bytes)
            .then(left.file.cmp(&right.file))
    });
    choices.dedup_by(|later, earlier| later.quant == earlier.quant);
    if choices.is_empty() {
        return Err(HfAddError::NoGguf);
    }
    Ok(Inspection {
        repository: repository.as_str().into(),
        revision: model.sha.clone(),
        license,
        choices,
        model,
    })
}

/// Pinned `local` catalog entries for every inspected quantization. One GGUF header
/// (from the smallest file) supplies the shared model facts; each entry keeps its own
/// file, size and SHA-256, which `up` verifies before serving.
pub async fn entries(
    inspection: &Inspection,
    transport: &dyn AdmissionTransport,
    now: &str,
    cancel: &CancellationToken,
) -> Result<Vec<CatalogModel>, HfAddError> {
    let smallest = inspection.choices.first().ok_or(HfAddError::NoGguf)?;
    let repository =
        Repository::parse(&format!("https://huggingface.co/{}", inspection.repository))
            .map_err(|_| HfAddError::Reference("invalid repository"))?;
    let url = format!(
        "https://huggingface.co/{}/resolve/{}/{}",
        inspection.repository, inspection.revision, smallest.file
    );
    let parsed = gguf(transport, cancel, &url)
        .await?
        .map_err(|reason| HfAddError::Metadata(reason.into()))?;
    let today = today(now)?;
    inspection
        .choices
        .iter()
        .map(|choice| {
            let selected = SelectedGguf {
                file: &choice.file,
                sha256: &choice.sha256,
                bytes: choice.bytes,
                quant: &choice.quant,
            };
            let mut model = build_entry(
                &repository,
                &inspection.model,
                &parsed,
                &selected,
                inspection.license,
                &today,
            )
            .map_err(HfAddError::Metadata)?;
            let id = format!(
                "hf.co/{}:{}",
                inspection.repository.to_ascii_lowercase(),
                choice.quant.to_ascii_lowercase()
            );
            model.id = id.clone();
            model.provenance = EntryProvenance::Local;
            model.source.ollama = Some(id);
            Ok(model)
        })
        .collect()
}

/// The entry for one quantization of an inspected repository.
pub async fn entry(
    inspection: &Inspection,
    quant: &str,
    transport: &dyn AdmissionTransport,
    now: &str,
    cancel: &CancellationToken,
) -> Result<CatalogModel, HfAddError> {
    if !inspection
        .choices
        .iter()
        .any(|choice| choice.quant.eq_ignore_ascii_case(quant))
    {
        return Err(HfAddError::UnknownQuant(quant.to_ascii_uppercase()));
    }
    entries(inspection, transport, now, cancel)
        .await?
        .into_iter()
        .find(|model| model.quantizations[0].name.eq_ignore_ascii_case(quant))
        .ok_or_else(|| HfAddError::UnknownQuant(quant.to_ascii_uppercase()))
}

/// The largest quantization whose weights fit this machine, or `None` when none does.
pub fn best_fit<'a>(
    entries: &'a [CatalogModel],
    hardware: &rigspark_core::sizing::Hardware,
) -> Option<&'a CatalogModel> {
    entries
        .iter()
        .filter(|model| fits(model, hardware))
        .max_by(|left, right| {
            left.quantizations[0]
                .disk_bytes
                .total_cmp(&right.quantizations[0].disk_bytes)
        })
}

/// Whether an entry's weights fit this machine at its default context.
pub fn fits(model: &CatalogModel, hardware: &rigspark_core::sizing::Hardware) -> bool {
    rigspark_core::sizing::evaluate(&rigspark_core::sizing::SizingRequest {
        model: model.sizing(),
        hardware: hardware.clone(),
        context: None,
    })
    .is_ok_and(|result| result.fit.fits)
}

pub fn search_url(query: &str) -> Result<Url, HfAddError> {
    let query = query.trim();
    if query.is_empty()
        || query.len() > 80
        || !query
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b" ._-/".contains(&byte))
    {
        return Err(HfAddError::Reference(
            "search with letters, numbers, spaces, dots, dashes or slashes",
        ));
    }
    let mut url = Url::parse("https://huggingface.co/api/models")
        .map_err(|_| HfAddError::Reference("invalid search URL"))?;
    url.query_pairs_mut()
        .append_pair("search", query)
        .append_pair("filter", "gguf")
        .append_pair("sort", "downloads")
        .append_pair("limit", SEARCH_LIMIT);
    Ok(url)
}

/// Whether `url` is exactly a search this module builds.
pub fn is_search_url(url: &Url) -> bool {
    let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
    matches!(pairs.as_slice(), [(search, term), (filter, gguf), (sort, downloads), (limit, count)]
        if search == "search" && filter == "filter" && gguf == "gguf" && sort == "sort"
            && downloads == "downloads" && limit == "limit" && count == SEARCH_LIMIT
            && search_url(term).is_ok_and(|built| built.query() == url.query()))
}

pub async fn search(
    query: &str,
    transport: &dyn AdmissionTransport,
    cancel: &CancellationToken,
) -> Result<Vec<SearchHit>, HfAddError> {
    #[derive(Deserialize)]
    struct Hit {
        id: String,
        #[serde(default)]
        downloads: u64,
        #[serde(default)]
        likes: u64,
    }
    let url = search_url(query)?;
    let bytes = get(transport, cancel, url.as_str(), None, MAX_SEARCH_BYTES).await?;
    let hits: Vec<Hit> = serde_json::from_slice(&bytes)
        .map_err(|_| HfAddError::Metadata("invalid search response".into()))?;
    Ok(hits
        .into_iter()
        .filter(|hit| parse_reference(&hit.id).is_ok_and(|(_, quant)| quant.is_none()))
        .map(|hit| SearchHit {
            id: hit.id,
            downloads: hit.downloads,
            likes: hit.likes,
        })
        .collect())
}
