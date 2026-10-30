//! Models the user added from Hugging Face, kept beside the signed catalog in
//! `~/.rigspark/catalog/local.json`. Entries stay pinned to a commit and SHA-256, so `up`
//! verifies them exactly like catalog models.

use crate::secure_fs::Directory;
use rigspark_core::catalog::{Catalog, CatalogModel, EntryProvenance};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

const FILE: &str = "catalog/local.json";
const MAX_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MODELS: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum LocalCatalogError {
    #[error("your added models could not be read; fix or delete {0}")]
    Corrupt(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0} is already in the RigSpark catalog")]
    Conflict(String),
    #[error("you can add at most {MAX_MODELS} models")]
    Full,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct LocalCatalog {
    home: PathBuf,
}

impl LocalCatalog {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    pub fn path(&self) -> PathBuf {
        self.home.join(FILE)
    }

    /// Added models; none when nothing was added yet.
    pub fn models(&self) -> Result<Vec<CatalogModel>, LocalCatalogError> {
        let bytes = match Directory::open(&self.home)
            .and_then(|directory| directory.read(Path::new(FILE), MAX_BYTES, false))
        {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| LocalCatalogError::Corrupt(self.path().display().to_string()))?;
        Catalog::parse_local(text)
            .map(|catalog| catalog.models)
            .map_err(|_| LocalCatalogError::Corrupt(self.path().display().to_string()))
    }

    /// Adds or replaces `entry`; ids already in the signed or bundled catalog are refused.
    pub fn add(&self, entry: CatalogModel) -> Result<(), LocalCatalogError> {
        if entry.provenance != EntryProvenance::Local {
            return Err(LocalCatalogError::Invalid(
                "only models added from Hugging Face can be stored locally".into(),
            ));
        }
        let published = crate::catalog_update::CatalogStore::official(&self.home)
            .load_published()
            .map_err(|error| LocalCatalogError::Invalid(error.to_string()))?;
        if published.models.iter().any(|model| model.id == entry.id) {
            return Err(LocalCatalogError::Conflict(entry.id));
        }
        let mut models = self.models()?;
        models.retain(|model| model.id != entry.id);
        if models.len() >= MAX_MODELS {
            return Err(LocalCatalogError::Full);
        }
        models.push(entry);
        models.sort_by(|left, right| left.id.cmp(&right.id));
        self.write(models)
    }

    /// Removes an added model; `false` when it was not there.
    pub fn remove(&self, id: &str) -> Result<bool, LocalCatalogError> {
        let mut models = self.models()?;
        let before = models.len();
        models.retain(|model| model.id != id);
        if models.len() == before {
            return Ok(false);
        }
        self.write(models)?;
        Ok(true)
    }

    fn write(&self, models: Vec<CatalogModel>) -> Result<(), LocalCatalogError> {
        let document = serde_json::json!({
            "schemaVersion": rigspark_core::catalog::SCHEMA_VERSION,
            "generatedAt": now(),
            "models": models,
        });
        let text = serde_json::to_string_pretty(&document)
            .map_err(|error| LocalCatalogError::Invalid(error.to_string()))?;
        Catalog::parse_local(&text).map_err(|error| LocalCatalogError::Invalid(error.0))?;
        std::fs::create_dir_all(&self.home)?;
        let directory = Directory::open(&self.home)?;
        directory.ensure_dir(Path::new("catalog"))?;
        let exists = self.path().exists();
        directory.write(Path::new(FILE), text.as_bytes(), !exists, false)?;
        Ok(())
    }
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(time::macros::format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
        .unwrap_or_else(|_| "1970-01-01T00:00:00.000Z".into())
}
