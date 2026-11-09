use super::{CatalogStatus, CatalogStore, CatalogUpdateError, MAX_ARTIFACT_BYTES};
use std::time::Duration;

pub const CATALOG_URL: &str =
    "https://github.com/rigspark/rigspark/releases/download/catalog-v1/catalog.json";

#[async_trait::async_trait]
pub trait CatalogTransport: Send + Sync {
    async fn download(&self) -> Result<Vec<u8>, CatalogUpdateError>;
}

pub struct OfficialCatalogTransport;

fn allowed(url: &url::Url) -> bool {
    url.scheme() == "https"
        && url.port_or_known_default() == Some(443)
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(
            url.host_str(),
            Some(
                "github.com"
                    | "release-assets.githubusercontent.com"
                    | "objects.githubusercontent.com"
            )
        )
}

#[async_trait::async_trait]
impl CatalogTransport for OfficialCatalogTransport {
    async fn download(&self) -> Result<Vec<u8>, CatalogUpdateError> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 3 || !allowed(attempt.url()) {
                    attempt.error("catalog redirect is not permitted")
                } else {
                    attempt.follow()
                }
            }))
            .build()
            .map_err(|_| CatalogUpdateError::Download)?;
        let mut response = client
            .get(CATALOG_URL)
            .header(reqwest::header::USER_AGENT, "rigspark-catalog")
            .send()
            .await
            .map_err(|_| CatalogUpdateError::Download)?;
        if !response.status().is_success() {
            return Err(CatalogUpdateError::Download);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ARTIFACT_BYTES as u64)
        {
            return Err(CatalogUpdateError::TooLarge);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| CatalogUpdateError::Download)?
        {
            if chunk.len() > MAX_ARTIFACT_BYTES - bytes.len() {
                return Err(CatalogUpdateError::TooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

impl CatalogStore {
    pub async fn update(
        &self,
        transport: &dyn CatalogTransport,
    ) -> Result<CatalogStatus, CatalogUpdateError> {
        self.key.ok_or(CatalogUpdateError::NotConfigured)?;
        self.install(&transport.download().await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_stay_on_official_https_hosts() {
        assert!(allowed(&url::Url::parse(CATALOG_URL).unwrap()));
        assert!(allowed(
            &url::Url::parse("https://release-assets.githubusercontent.com/path?token=value")
                .unwrap()
        ));
        for raw in [
            "http://github.com/path",
            "https://github.com:8443/path",
            "https://github.com.evil.test/path",
            "https://user@github.com/path",
            "https://127.0.0.1/path",
        ] {
            assert!(!allowed(&url::Url::parse(raw).unwrap()));
        }
    }
}
