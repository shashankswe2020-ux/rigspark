use crate::{catalog::require, sizing::ValidationError};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::sync::OnceLock;

pub const MAX_INDEX_HTML_BYTES: usize = 4 * 1024 * 1024;

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
        models.len() <= 10_000,
        "Artificial Analysis inventory has too many models",
    )?;
    for model in &models {
        model.validate()?;
    }
    Ok(models)
}
