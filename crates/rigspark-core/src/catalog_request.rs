use crate::sizing::ValidationError;
use serde_json::Value;
use url::Url;

const MAX_EVENT_BYTES: usize = 64 * 1024;
const MODEL_URL_HEADING: &str = "### Hugging Face model URL";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository(String);

impl Repository {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        let raw = raw.trim();
        let authority = raw
            .strip_prefix("https://")
            .and_then(|rest| rest.split('/').next())
            .ok_or_else(|| invalid("canonical Hugging Face URL required"))?;
        let url = Url::parse(raw).map_err(|_| invalid("invalid Hugging Face URL"))?;
        if url.scheme() != "https"
            || authority != "huggingface.co"
            || url.host_str() != Some("huggingface.co")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid("canonical Hugging Face URL required"));
        }
        let parts: Vec<_> = url
            .path_segments()
            .ok_or_else(|| invalid("repository path required"))?
            .collect();
        if parts.len() != 2 || !parts.iter().all(|part| valid_part(part)) {
            return Err(invalid("Hugging Face owner/repository path required"));
        }
        Ok(Self(format!("{}/{}", parts[0], parts[1])))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn url(&self) -> String {
        format!("https://huggingface.co/{}", self.0)
    }
}

fn valid_part(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 96
        && part.as_bytes()[0].is_ascii_alphanumeric()
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelRequest {
    pub issue_number: u64,
    pub issue_url: String,
    pub repository: Repository,
}

impl ModelRequest {
    pub fn from_event(raw: &str) -> Result<Self, ValidationError> {
        if raw.len() > MAX_EVENT_BYTES {
            return Err(invalid("GitHub event exceeds 64 KiB"));
        }
        let event: Value =
            serde_json::from_str(raw).map_err(|_| invalid("invalid GitHub event"))?;
        let issue = event
            .get("issue")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("GitHub issue missing"))?;
        let issue_number = issue
            .get("number")
            .and_then(Value::as_u64)
            .filter(|number| *number > 0)
            .ok_or_else(|| invalid("GitHub issue number missing"))?;
        let issue_url = issue
            .get("html_url")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("GitHub issue URL missing"))?;
        validate_issue_url(issue_url, issue_number)?;
        let body = issue
            .get("body")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("GitHub issue body missing"))?;
        let repository = Repository::parse(extract_field(body)?)?;
        Ok(Self {
            issue_number,
            issue_url: issue_url.to_owned(),
            repository,
        })
    }
}

fn extract_field(body: &str) -> Result<&str, ValidationError> {
    let (_, after) = body
        .split_once(MODEL_URL_HEADING)
        .ok_or_else(|| invalid("model URL field missing"))?;
    let value = after
        .split("\n### ")
        .next()
        .unwrap_or(after)
        .trim();
    if value.is_empty() || value.lines().count() != 1 {
        return Err(invalid("model URL field must contain one URL"));
    }
    Ok(value)
}

fn validate_issue_url(raw: &str, issue_number: u64) -> Result<(), ValidationError> {
    let url = Url::parse(raw).map_err(|_| invalid("invalid GitHub issue URL"))?;
    let expected = format!(
        "/shashankswe2020-ux/rigspark/issues/{issue_number}"
    );
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.path() != expected
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("unexpected GitHub issue URL"));
    }
    Ok(())
}

fn invalid(message: &str) -> ValidationError {
    ValidationError(message.into())
}
