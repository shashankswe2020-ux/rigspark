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
                if follow_redirect(attempt.previous().len(), attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.error("catalog redirect is not permitted")
                }
            }))
            .build()
            .map_err(|_| CatalogUpdateError::Download)?;
        let response = client
            .get(CATALOG_URL)
            .header(reqwest::header::USER_AGENT, "rigspark-catalog")
            .send()
            .await
            .map_err(|_| CatalogUpdateError::Download)?;
        read_catalog(response, MAX_ARTIFACT_BYTES).await
    }
}

/// At most three redirects, each to an official HTTPS host.
fn follow_redirect(previous: usize, url: &url::Url) -> bool {
    previous < 3 && allowed(url)
}

/// Read a catalog response, refusing errors and anything over `limit` bytes.
async fn read_catalog(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, CatalogUpdateError> {
    if !response.status().is_success() {
        return Err(CatalogUpdateError::Download);
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(CatalogUpdateError::TooLarge);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CatalogUpdateError::Download)?
    {
        if chunk.len() > limit - bytes.len() {
            return Err(CatalogUpdateError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
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

    #[test]
    fn at_most_three_redirects_are_followed() {
        let url = url::Url::parse(CATALOG_URL).unwrap();
        assert!(follow_redirect(0, &url));
        assert!(follow_redirect(2, &url));
        assert!(!follow_redirect(3, &url));
        assert!(!follow_redirect(
            0,
            &url::Url::parse("https://evil.test/c.json").unwrap()
        ));
    }

    /// A response whose length is known up front (an exact body).
    fn sized(status: u16, body: &'static [u8]) -> reqwest::Response {
        let response = http::Response::builder().status(status).body(body).unwrap();
        reqwest::Response::from(response)
    }

    /// A response whose length is unknown until it is read (a streamed body).
    fn streamed(body: &'static [u8]) -> reqwest::Response {
        let chunks = body
            .chunks(2)
            .map(|chunk| Ok::<_, std::io::Error>(bytes::Bytes::from_static(chunk)));
        let body =
            reqwest::Body::wrap_stream(futures_util::stream::iter(chunks.collect::<Vec<_>>()));
        reqwest::Response::from(http::Response::builder().status(200).body(body).unwrap())
    }

    #[tokio::test]
    async fn catalog_bodies_are_bounded_and_need_success() {
        assert_eq!(
            read_catalog(sized(200, b"12345"), 5).await.unwrap(),
            b"12345"
        );
        assert_eq!(read_catalog(streamed(b"12345"), 5).await.unwrap(), b"12345");
        for status in [302, 404, 500] {
            assert!(matches!(
                read_catalog(sized(status, b"12345"), 5).await,
                Err(CatalogUpdateError::Download)
            ));
        }
        // A known length over the limit is refused before reading.
        assert!(matches!(
            read_catalog(sized(200, b"123456"), 5).await,
            Err(CatalogUpdateError::TooLarge)
        ));
        // An unknown length is counted while reading.
        assert!(matches!(
            read_catalog(streamed(b"123456"), 5).await,
            Err(CatalogUpdateError::TooLarge)
        ));
    }
}
