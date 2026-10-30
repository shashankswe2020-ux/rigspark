use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rigspark_core::catalog::Catalog;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

mod store;
mod transport;
pub use store::{CatalogStatus, CatalogStore, LoadedCatalog};
pub use transport::{CATALOG_URL, CatalogTransport, OfficialCatalogTransport};

pub const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum CatalogUpdateError {
    #[error("catalog artifact exceeds the size limit")]
    TooLarge,
    #[error("catalog signature or public key is invalid")]
    Signature,
    #[error("invalid catalog artifact: {0}")]
    Invalid(&'static str),
    #[error("catalog updates are unavailable: production signing key is not provisioned")]
    NotConfigured,
    #[error("catalog storage operation failed")]
    Storage(#[from] std::io::Error),
    #[error("catalog update is older than the installed snapshot or reuses its revision")]
    Rollback,
    #[error("another catalog update is in progress")]
    Busy,
    #[error("catalog download failed")]
    Download,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SignedCatalog {
    pub payload: String,
    pub signature: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogPayload {
    pub format_version: u8,
    pub revision: u64,
    pub published_at: String,
    pub catalog: String,
}

pub struct VerifiedCatalog {
    pub revision: u64,
    pub published_at: String,
    pub digest: String,
    pub catalog: Catalog,
    pub raw: String,
}

pub fn decode_hex<const SIZE: usize>(raw: &str) -> Result<[u8; SIZE], CatalogUpdateError> {
    if raw.len() != SIZE * 2 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CatalogUpdateError::Signature);
    }
    let mut result = [0; SIZE];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&raw[index * 2..index * 2 + 2], 16)
            .map_err(|_| CatalogUpdateError::Signature)?;
    }
    Ok(result)
}

pub fn verify(bytes: &[u8], key: &[u8; 32]) -> Result<VerifiedCatalog, CatalogUpdateError> {
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(CatalogUpdateError::TooLarge);
    }
    let envelope: SignedCatalog = serde_json::from_slice(bytes)
        .map_err(|_| CatalogUpdateError::Invalid("signed envelope"))?;
    let key = VerifyingKey::from_bytes(key).map_err(|_| CatalogUpdateError::Signature)?;
    let signature = Signature::from_bytes(&decode_hex(&envelope.signature)?);
    key.verify_strict(envelope.payload.as_bytes(), &signature)
        .map_err(|_| CatalogUpdateError::Signature)?;
    let payload: CatalogPayload = serde_json::from_str(&envelope.payload)
        .map_err(|_| CatalogUpdateError::Invalid("payload"))?;
    if payload.format_version != 1 || payload.revision == 0 {
        return Err(CatalogUpdateError::Invalid(
            "unsupported format or revision",
        ));
    }
    let published = OffsetDateTime::parse(&payload.published_at, &Rfc3339)
        .map_err(|_| CatalogUpdateError::Invalid("publication date"))?;
    let catalog = Catalog::parse(&payload.catalog)
        .map_err(|_| CatalogUpdateError::Invalid("catalog schema or metadata"))?;
    let generated = OffsetDateTime::parse(&catalog.generated_at, &Rfc3339)
        .map_err(|_| CatalogUpdateError::Invalid("generation date"))?;
    if catalog.models.is_empty() || generated > published || catalog.has_local() {
        return Err(CatalogUpdateError::Invalid(
            "empty catalog or inconsistent dates",
        ));
    }
    Ok(VerifiedCatalog {
        revision: payload.revision,
        published_at: payload.published_at,
        digest: format!("{:x}", Sha256::digest(envelope.payload.as_bytes())),
        catalog,
        raw: payload.catalog,
    })
}

pub fn sign_catalog(
    payload: CatalogPayload,
    seed: &[u8; 32],
    public_key: &[u8; 32],
) -> Result<Vec<u8>, CatalogUpdateError> {
    let signing_key = SigningKey::from_bytes(seed);
    if signing_key.verifying_key().to_bytes() != *public_key {
        return Err(CatalogUpdateError::Signature);
    }
    if payload.catalog.len() > MAX_ARTIFACT_BYTES {
        return Err(CatalogUpdateError::TooLarge);
    }
    let payload =
        serde_json::to_string(&payload).map_err(|_| CatalogUpdateError::Invalid("payload"))?;
    let signature = signing_key
        .sign(payload.as_bytes())
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let bytes = serde_json::to_vec(&SignedCatalog { payload, signature })
        .map_err(|_| CatalogUpdateError::Invalid("envelope"))?;
    verify(&bytes, public_key)?;
    Ok(bytes)
}
