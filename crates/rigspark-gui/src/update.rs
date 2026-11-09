use serde::Deserialize;
use serde_json::{Value, json};
fn version(raw: &str) -> Option<[u64; 3]> {
    let parts: Vec<_> = raw.strip_prefix('v').unwrap_or(raw).split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut result = [0; 3];
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        result[index] = part.parse().ok()?;
        if result[index] > 9007199254740991 {
            return None;
        }
    }
    Some(result)
}
pub fn status(current: &str, tag: Option<&str>) -> Value {
    let unknown = || json!({"currentVersion":current,"latestVersion":null,"state":"unknown","releaseUrl":null});
    let Some((installed, latest)) = version(current).zip(tag.and_then(version)) else {
        return unknown();
    };
    let newer = latest > installed;
    json!({"currentVersion":current,"latestVersion":tag.unwrap_or("").trim_start_matches('v'),"state":if newer{"update-available"}else{"current"},"releaseUrl":if newer{Some("https://github.com/rigspark/rigspark/releases")}else{None}})
}
/// Where installed builds ask for the newest release.
pub const LATEST_RELEASE: &str = "https://api.github.com/repos/rigspark/rigspark/releases/latest";
/// Ask `url` (a GitHub "latest release" endpoint) whether a newer release exists.
pub async fn fetch(url: &str) -> Value {
    let current = env!("CARGO_PKG_VERSION");
    let operation = async {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .ok()?;
        let mut response = client
            .get(url)
            .header("accept", "application/vnd.github+json")
            .header("user-agent", "rigspark-native")
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.ok()? {
            if bytes.len() + chunk.len() > 65536 {
                return None;
            }
            bytes.extend_from_slice(&chunk);
        }
        #[derive(Deserialize)]
        struct Release {
            tag_name: String,
        }
        serde_json::from_slice::<Release>(&bytes)
            .ok()
            .map(|release| release.tag_name)
    };
    let tag = tokio::time::timeout(std::time::Duration::from_secs(3), operation)
        .await
        .ok()
        .flatten();
    status(current, tag.as_deref())
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    /// Serve one HTTP response on loopback and return its URL.
    async fn serve_once(status: &'static str, body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        format!("http://{address}/releases/latest")
    }

    #[tokio::test]
    async fn fetch_reads_the_latest_release_tag() {
        let url = serve_once("200 OK", r#"{"tag_name":"v999.0.0"}"#).await;
        let value = fetch(&url).await;
        assert_eq!(value["state"], "update-available");
        assert_eq!(value["latestVersion"], "999.0.0");
        assert_eq!(value["currentVersion"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn fetch_is_unknown_when_the_release_check_fails() {
        let url = serve_once("301 Moved Permanently", "").await;
        assert_eq!(fetch(&url).await["state"], "unknown");
    }

    #[test]
    fn installed_builds_ask_the_rigspark_organisation() {
        assert_eq!(
            LATEST_RELEASE,
            "https://api.github.com/repos/rigspark/rigspark/releases/latest"
        );
    }

    #[test]
    fn release_comparison_is_bounded_and_honest() {
        assert_eq!(
            status("0.11.4", Some("v0.12.0"))["state"],
            "update-available"
        );
        assert_eq!(status("0.11.4", Some("0.11.3"))["state"], "current");
        assert_eq!(status("0.11.4", Some("0.12.0-beta"))["state"], "unknown");
        assert_eq!(status("0.11.4", None)["releaseUrl"], Value::Null);
        assert_eq!(
            status("0.11.2", Some("v0.12.0")),
            json!({"currentVersion":"0.11.2","latestVersion":"0.12.0","state":"update-available","releaseUrl":"https://github.com/rigspark/rigspark/releases"})
        );
        for tag in ["v0.11.2", "v0.10.9"] {
            assert_eq!(status("0.11.2", Some(tag))["state"], "current", "{tag}");
            assert_eq!(status("0.11.2", Some(tag))["releaseUrl"], Value::Null);
        }
        let unknown = json!({"currentVersion":"0.11.2","latestVersion":null,"state":"unknown","releaseUrl":null});
        for tag in ["latest", "v0.12", "v0.12.0-beta.1", "1.2.3.4"] {
            assert_eq!(status("0.11.2", Some(tag)), unknown, "{tag}");
        }
        assert_eq!(status("unknown", Some("v0.12.0"))["state"], "unknown");
    }
}
