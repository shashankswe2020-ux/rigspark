use super::{
    CatalogUpdateError, MAX_ARTIFACT_BYTES, SignedCatalog, VerifiedCatalog, decode_hex, verify,
};
use crate::secure_fs::Directory;
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use rigspark_core::catalog::Catalog;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

const STATE: &str = "catalog/snapshots.json";
const MAX_STATE_BYTES: u64 = (MAX_ARTIFACT_BYTES * 3) as u64;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogStatus {
    pub source: &'static str,
    pub revision: Option<u64>,
    pub digest: String,
    pub generated_at: String,
    pub published_at: Option<String>,
    pub model_count: usize,
    /// Models the user added from Hugging Face, included in `model_count`.
    pub local_count: usize,
    pub updates_configured: bool,
    pub warnings: Vec<String>,
}

pub struct LoadedCatalog {
    pub catalog: Catalog,
    pub raw: String,
    pub status: CatalogStatus,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshots {
    version: u8,
    highest_revision: u64,
    highest_digest: String,
    current: SignedCatalog,
    previous: Option<SignedCatalog>,
}

pub struct CatalogStore {
    home: PathBuf,
    pub(super) key: Option<[u8; 32]>,
}

impl CatalogStore {
    pub fn new(home: impl Into<PathBuf>, key: Option<[u8; 32]>) -> Self {
        Self {
            home: home.into(),
            key,
        }
    }

    pub fn official(home: impl Into<PathBuf>) -> Self {
        let key = decode_hex(include_str!("../catalog_public_key.hex").trim()).ok();
        Self::new(home, key)
    }

    fn read_state(&self) -> Result<Option<Snapshots>, CatalogUpdateError> {
        let result = Directory::open(&self.home)
            .and_then(|directory| directory.read(Path::new(STATE), MAX_STATE_BYTES, false));
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let state: Snapshots = serde_json::from_slice(&bytes)
            .map_err(|_| CatalogUpdateError::Invalid("cache state"))?;
        if state.version != 1
            || state.highest_revision == 0
            || decode_hex::<32>(&state.highest_digest).is_err()
        {
            return Err(CatalogUpdateError::Invalid(
                "cache version or high-water mark",
            ));
        }
        Ok(Some(state))
    }

    fn verify_cached(
        &self,
        envelope: &SignedCatalog,
    ) -> Result<VerifiedCatalog, CatalogUpdateError> {
        let bytes = serde_json::to_vec(envelope)
            .map_err(|_| CatalogUpdateError::Invalid("cache envelope"))?;
        verify(&bytes, &self.key.ok_or(CatalogUpdateError::NotConfigured)?)
    }

    fn selected(
        &self,
        snapshot: VerifiedCatalog,
        source: &'static str,
        warnings: Vec<String>,
    ) -> LoadedCatalog {
        let status = CatalogStatus {
            source,
            revision: Some(snapshot.revision),
            digest: snapshot.digest,
            generated_at: snapshot.catalog.generated_at.clone(),
            published_at: Some(snapshot.published_at),
            model_count: snapshot.catalog.models.len(),
            local_count: 0,
            updates_configured: self.key.is_some(),
            warnings,
        };
        LoadedCatalog {
            catalog: snapshot.catalog,
            raw: snapshot.raw,
            status,
        }
    }

    /// The signed or bundled catalog plus the models the user added from Hugging Face.
    pub fn load(&self) -> Result<LoadedCatalog, CatalogUpdateError> {
        let mut loaded = self.load_snapshot()?;
        match crate::local_catalog::LocalCatalog::new(&self.home).models() {
            Ok(added) => {
                let added: Vec<_> = added
                    .into_iter()
                    .filter(|entry| {
                        !loaded
                            .catalog
                            .models
                            .iter()
                            .any(|model| model.id == entry.id)
                    })
                    .collect();
                if !added.is_empty() {
                    let mut catalog = loaded.catalog.clone();
                    catalog.models.extend(added.iter().cloned());
                    catalog.schema_version =
                        rigspark_core::catalog::required_schema_version(&catalog.models)
                            .max(catalog.schema_version)
                            .max(3);
                    let raw = serde_json::to_string(&catalog)
                        .map_err(|_| CatalogUpdateError::Invalid("merged catalog"))?;
                    let catalog = Catalog::parse(&raw)
                        .map_err(|_| CatalogUpdateError::Invalid("merged catalog"))?;
                    loaded.status.local_count = added.len();
                    loaded.status.model_count = catalog.models.len();
                    loaded.catalog = catalog;
                    loaded.raw = raw;
                }
            }
            Err(error) => loaded
                .status
                .warnings
                .push(format!("Models you added are hidden: {error}")),
        }
        Ok(loaded)
    }

    /// Only the signed or bundled catalog, without models the user added.
    pub fn load_published(&self) -> Result<Catalog, CatalogUpdateError> {
        Ok(self.load_snapshot()?.catalog)
    }

    fn load_snapshot(&self) -> Result<LoadedCatalog, CatalogUpdateError> {
        let mut warnings = Vec::new();
        let catalog = Catalog::parse(rigspark_core::MODELS_JSON)
            .map_err(|_| CatalogUpdateError::Invalid("bundled catalog"))?;
        // A downloaded snapshot only wins while it is at least as new as the catalog
        // compiled into this binary; otherwise upgrading rigspark would stay pinned to it.
        let current = |snapshot: &VerifiedCatalog| {
            generated(&snapshot.catalog.generated_at) >= generated(&catalog.generated_at)
        };
        match self.read_state() {
            Ok(Some(state)) => {
                match self.verify_cached(&state.current) {
                    Ok(snapshot) if current(&snapshot) => {
                        return Ok(self.selected(snapshot, "updated", warnings));
                    }
                    Ok(_) => {}
                    Err(_) => {
                        warnings.push("Active catalog failed verification; using fallback.".into())
                    }
                }
                if let Some(previous) = state.previous {
                    match self.verify_cached(&previous) {
                        Ok(snapshot) if current(&snapshot) => {
                            return Ok(self.selected(snapshot, "previous", warnings));
                        }
                        Ok(_) => {}
                        Err(_) => warnings.push("Previous catalog failed verification.".into()),
                    }
                }
            }
            Ok(None) => (),
            Err(_) => warnings
                .push("Catalog cache is unreadable or invalid; using bundled catalog.".into()),
        }
        let status = CatalogStatus {
            source: "bundled",
            revision: None,
            digest: format!(
                "{:x}",
                Sha256::digest(rigspark_core::MODELS_JSON.as_bytes())
            ),
            generated_at: catalog.generated_at.clone(),
            published_at: None,
            model_count: catalog.models.len(),
            local_count: 0,
            updates_configured: self.key.is_some(),
            warnings,
        };
        Ok(LoadedCatalog {
            catalog,
            raw: rigspark_core::MODELS_JSON.into(),
            status,
        })
    }

    pub fn install(&self, bytes: &[u8]) -> Result<CatalogStatus, CatalogUpdateError> {
        let snapshot = verify(bytes, &self.key.ok_or(CatalogUpdateError::NotConfigured)?)?;
        std::fs::create_dir_all(&self.home)?;
        let directory = Directory::open(&self.home)?;
        directory.ensure_dir(Path::new("catalog"))?;
        let cache = directory.subdir(Path::new("catalog"))?;
        let mut options = cap_std::fs::OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
        }
        let lock = cache.open_with("update.lock", &options)?.into_std();
        if !lock.metadata()?.is_file() {
            return Err(CatalogUpdateError::Invalid("update lock"));
        }
        lock.try_lock().map_err(|_| CatalogUpdateError::Busy)?;
        let state = self.read_state()?;
        if let Some(state) = &state
            && (snapshot.revision < state.highest_revision
                || (snapshot.revision == state.highest_revision
                    && snapshot.digest != state.highest_digest))
        {
            return Err(CatalogUpdateError::Rollback);
        }
        let previous = state.and_then(|state| {
            if self
                .verify_cached(&state.current)
                .is_ok_and(|current| current.revision < snapshot.revision)
            {
                Some(state.current)
            } else {
                state
                    .previous
                    .filter(|previous| self.verify_cached(previous).is_ok())
            }
        });
        let current =
            serde_json::from_slice(bytes).map_err(|_| CatalogUpdateError::Invalid("envelope"))?;
        let state = Snapshots {
            version: 1,
            highest_revision: snapshot.revision,
            highest_digest: snapshot.digest.clone(),
            current,
            previous,
        };
        let encoded = serde_json::to_vec(&state)
            .map_err(|_| CatalogUpdateError::Invalid("cache encoding"))?;
        if encoded.len() as u64 > MAX_STATE_BYTES {
            return Err(CatalogUpdateError::TooLarge);
        }
        directory.write(Path::new(STATE), &encoded, false, false)?;
        Ok(self.selected(snapshot, "updated", Vec::new()).status)
    }
}

/// Generation time of a validated catalog; unparsable stamps sort first.
fn generated(stamp: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(stamp, &time::format_description::well_known::Rfc3339).ok()
}
