//! Native source control for chat sessions: RigSpark talks to GitHub's REST API itself
//! and commits in local repositories the user allowed. It appears as a connector, so
//! every session sees its tools and every call goes through the same approval as MCP
//! tools (reads ask once per session; creating a pull request or a commit asks each time).
//!
//! The GitHub token lives in `<home>/github-token` (owner-only), never in
//! `connectors.json`, and is sent only to `https://api.github.com`. Commits use the
//! same hooks as agents launched by `rigspark connect`, so they carry RigSpark's
//! `Co-Authored-By` and `Signed-off-by` trailers.

use crate::mcp::{ClientFactory, Connection, Connector, McpError, Tool, ToolResult};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub const GITHUB_ID: &str = "github";
pub const GITHUB_API: &str = "https://api.github.com";
const TOKEN_FILE: &str = "github-token";
const MAX_RESPONSE: usize = 2 * 1024 * 1024;

/// The GitHub token, kept in an owner-only file under the RigSpark home.
pub struct TokenStore {
    path: PathBuf,
}
impl TokenStore {
    pub fn new(home: &Path) -> Self {
        Self {
            path: home.join(TOKEN_FILE),
        }
    }
    pub fn load(&self) -> Option<String> {
        crate::state::secure_read(&self.path, 4096, true)
            .ok()
            .map(|token| token.trim().to_owned())
            .filter(|token| valid_token(token))
    }
    pub fn save(&self, token: &str) -> Result<(), McpError> {
        let token = token.trim();
        if !valid_token(token) {
            return Err(McpError::Invalid);
        }
        if let Some(parent) = self.path.parent() {
            crate::memory::owned_directory(parent).map_err(|_| McpError::Storage)?;
        }
        crate::memory::atomic_file(&self.path, token.as_bytes()).map_err(|_| McpError::Storage)
    }
    pub fn remove(&self) -> Result<(), McpError> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(McpError::Storage),
        }
    }
}

/// GitHub tokens are printable ASCII without spaces (`ghp_…`, `github_pat_…`, `gho_…`).
pub fn valid_token(token: &str) -> bool {
    (20..=255).contains(&token.len()) && token.bytes().all(|byte| byte.is_ascii_graphic())
}

/// `owner/name`, as GitHub allows them.
pub fn valid_repository(value: &str) -> bool {
    let part = |part: &str| {
        !part.is_empty()
            && part.len() <= 100
            && part != "."
            && part != ".."
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    matches!(value.split_once('/'), Some((owner, name)) if part(owner) && part(name))
}

/// A branch name GitHub and git both accept, conservatively.
pub fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with(['-', '/'])
        && !value.ends_with(['/', '.'])
        && !value.contains("..")
        && !value.contains("//")
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/' | b':')
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpReply {
    pub status: u16,
    pub body: Value,
}

/// The GitHub API transport; tests swap in a recorded one.
#[async_trait::async_trait]
pub trait GithubHttp: Send + Sync {
    async fn send(
        &self,
        method: &str,
        path_and_query: &str,
        token: &str,
        body: Option<&Value>,
    ) -> Result<HttpReply, McpError>;
}

pub struct ReqwestGithub {
    client: reqwest::Client,
}
impl ReqwestGithub {
    pub fn new() -> Result<Self, McpError> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| McpError::Transport)?;
        Ok(Self { client })
    }
}
#[async_trait::async_trait]
impl GithubHttp for ReqwestGithub {
    async fn send(
        &self,
        method: &str,
        path_and_query: &str,
        token: &str,
        body: Option<&Value>,
    ) -> Result<HttpReply, McpError> {
        if !path_and_query.starts_with('/') || path_and_query.contains("//") {
            return Err(McpError::Invalid);
        }
        let method =
            reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| McpError::Invalid)?;
        let mut request = self
            .client
            .request(method, format!("{GITHUB_API}{path_and_query}"))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header(reqwest::header::USER_AGENT, "rigspark-community")
            .bearer_auth(token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let mut response = request.send().await.map_err(|_| McpError::Transport)?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| McpError::Transport)? {
            if bytes.len() + chunk.len() > MAX_RESPONSE {
                return Err(McpError::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).map_err(|_| McpError::Invalid)?
        };
        Ok(HttpReply { status, body })
    }
}

/// Checks a token and returns the GitHub login it belongs to.
pub async fn verify_token(http: &dyn GithubHttp, token: &str) -> Result<String, McpError> {
    if !valid_token(token) {
        return Err(McpError::Invalid);
    }
    let reply = http.send("GET", "/user", token, None).await?;
    match (reply.status, reply.body["login"].as_str()) {
        (200, Some(login)) if !login.is_empty() && login.len() <= 100 => Ok(login.into()),
        (401 | 403, _) => Err(McpError::Approval),
        _ => Err(McpError::Invalid),
    }
}

/// Folders the user allowed for commits: absolute, existing directories, at most 16.
pub fn validate_repositories(repositories: &[String]) -> Result<(), McpError> {
    if repositories.len() > 16
        || repositories.iter().any(|folder| {
            folder.len() > 1024 || folder.contains('\0') || !Path::new(folder).is_absolute()
        })
    {
        return Err(McpError::Invalid);
    }
    Ok(())
}

pub struct GithubConnection {
    home: PathBuf,
    token: Option<String>,
    repositories: Vec<PathBuf>,
    http: Box<dyn GithubHttp>,
}

impl GithubConnection {
    pub fn new(
        home: &Path,
        token: Option<String>,
        repositories: &[String],
        http: Box<dyn GithubHttp>,
    ) -> Self {
        Self {
            home: home.into(),
            token,
            repositories: repositories
                .iter()
                .filter_map(|folder| std::fs::canonicalize(folder).ok())
                .collect(),
            http,
        }
    }

    fn token(&self) -> Result<&str, ToolResult> {
        self.token.as_deref().ok_or_else(|| {
            error("GitHub isn't connected. Add a token under Connectors > Source control.")
        })
    }

    async fn github(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, ToolResult> {
        let token = self.token()?;
        let reply = self
            .http
            .send(method, path, token, body)
            .await
            .map_err(|_| error("GitHub could not be reached."))?;
        match reply.status {
            200..=299 => Ok(reply.body),
            401 => Err(error(
                "GitHub rejected the token. Reconnect GitHub under Connectors.",
            )),
            403 | 429 => Err(error(&format!(
                "GitHub refused the request: {}",
                message(&reply.body)
            ))),
            404 => Err(error("Not found, or the token can't see that repository.")),
            status => Err(error(&format!(
                "GitHub returned {status}: {}",
                message(&reply.body)
            ))),
        }
    }

    async fn list_issues(&self, args: &Value) -> Result<String, ToolResult> {
        let repository = repository(args)?;
        let state = match args["state"].as_str().unwrap_or("open") {
            state @ ("open" | "closed" | "all") => state,
            _ => return Err(error("state must be open, closed or all")),
        };
        let issues = self
            .github(
                "GET",
                &format!("/repos/{repository}/issues?state={state}&per_page=30"),
                None,
            )
            .await?;
        let lines: Vec<String> = issues
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|issue| {
                format!(
                    "#{} {}{} ({}, by {})",
                    issue["number"],
                    text(&issue["title"], 200),
                    if issue.get("pull_request").is_some() {
                        " [pull request]"
                    } else {
                        ""
                    },
                    text(&issue["state"], 20),
                    text(&issue["user"]["login"], 100),
                )
            })
            .collect();
        Ok(if lines.is_empty() {
            format!("No {state} issues in {repository}.")
        } else {
            lines.join("\n")
        })
    }

    async fn read_issue(&self, args: &Value) -> Result<String, ToolResult> {
        let repository = repository(args)?;
        let number = args["number"]
            .as_u64()
            .filter(|number| (1..=100_000_000).contains(number))
            .ok_or_else(|| error("number must be an issue number"))?;
        let issue = self
            .github("GET", &format!("/repos/{repository}/issues/{number}"), None)
            .await?;
        let comments = self
            .github(
                "GET",
                &format!("/repos/{repository}/issues/{number}/comments?per_page=30"),
                None,
            )
            .await?;
        let mut out = format!(
            "#{number} {} ({}, by {})\n{}\n\n{}",
            text(&issue["title"], 300),
            text(&issue["state"], 20),
            text(&issue["user"]["login"], 100),
            text(&issue["html_url"], 300),
            text(&issue["body"], 20_000),
        );
        for comment in comments.as_array().map(Vec::as_slice).unwrap_or_default() {
            out.push_str(&format!(
                "\n\n--- {}:\n{}",
                text(&comment["user"]["login"], 100),
                text(&comment["body"], 5_000)
            ));
        }
        Ok(out)
    }

    async fn create_pull_request(&self, args: &Value) -> Result<String, ToolResult> {
        let repository = repository(args)?;
        let branch = |key: &str| {
            args[key]
                .as_str()
                .filter(|value| valid_branch(value))
                .ok_or_else(|| error(&format!("{key} must be a branch name")))
        };
        let (head, base) = (branch("head")?, branch("base")?);
        let title = args["title"]
            .as_str()
            .map(str::trim)
            .filter(|title| !title.is_empty() && title.len() <= 256)
            .ok_or_else(|| error("title is required (at most 256 characters)"))?;
        let body = args["body"].as_str().unwrap_or("");
        if body.len() > 65_536 {
            return Err(error("body is too long"));
        }
        let created = self
            .github(
                "POST",
                &format!("/repos/{repository}/pulls"),
                Some(&json!({"title": title, "head": head, "base": base, "body": body})),
            )
            .await?;
        Ok(format!(
            "Opened pull request #{}: {}",
            created["number"],
            text(&created["html_url"], 300)
        ))
    }

    async fn commit(&self, args: &Value, cancel: &CancellationToken) -> Result<String, ToolResult> {
        let folder = args["folder"]
            .as_str()
            .ok_or_else(|| error("folder is required"))?;
        let folder = std::fs::canonicalize(folder)
            .ok()
            .filter(|folder| self.repositories.contains(folder))
            .ok_or_else(|| {
                error(
                    "That folder isn't allowed. Add it under Connectors > Source control > GitHub.",
                )
            })?;
        let message = args["message"]
            .as_str()
            .map(str::trim)
            .filter(|message| !message.is_empty() && message.len() <= 10_000)
            .ok_or_else(|| error("message is required"))?;
        let model = crate::state::StateStore::new(
            crate::state::Config::from_home(&self.home)
                .map_err(|_| error("RigSpark home unavailable"))?,
        )
        .read()
        .ok()
        .and_then(|state| state.active)
        .map(|active| active.runtime_model_id.unwrap_or(active.model_id))
        .unwrap_or_else(|| "local".into());
        let attribution = crate::commit_attribution::install(&self.home)
            .map_err(|_| error("Could not set up commit attribution."))?;
        let mut env = crate::process_control::minimal_env();
        env.extend(attribution.env(&model, None));
        let git = |args: Vec<&str>| {
            let env = env.clone();
            let folder = folder.clone();
            let args: Vec<String> = args.into_iter().map(String::from).collect();
            async move {
                let binary = crate::process_control::resolve_binary("git")
                    .map_err(|_| error("git is not installed"))?;
                let run = tokio::process::Command::new(binary)
                    .arg("-C")
                    .arg(&folder)
                    .args(&args)
                    .env_clear()
                    .envs(env)
                    .stdin(std::process::Stdio::null())
                    .kill_on_drop(true)
                    .output();
                let output = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(error("cancelled")),
                    output = tokio::time::timeout(Duration::from_secs(60), run) => output
                        .map_err(|_| error("git timed out"))?
                        .map_err(|_| error("git could not run"))?,
                };
                let combined = format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                if output.status.success() {
                    Ok(combined)
                } else {
                    Err(error(&format!(
                        "git {} failed: {}",
                        args[0],
                        clip(combined.trim(), 2000)
                    )))
                }
            }
        };
        git(vec!["add", "--all"]).await?;
        git(vec!["commit", "--quiet", "--message", message]).await?;
        let summary = git(vec!["log", "-1", "--format=%h %s%n%n%b"]).await?;
        Ok(format!(
            "Committed in {}:\n{}",
            folder.display(),
            summary.trim()
        ))
    }
}

#[async_trait::async_trait]
impl Connection for GithubConnection {
    async fn tools(&mut self, _: &CancellationToken) -> Result<Vec<Tool>, McpError> {
        let repository =
            json!({"type":"string","description":"owner/name, for example rigspark/rigspark"});
        let mut tools = Vec::new();
        if self.token.is_some() {
            tools.push(Tool {
                name: "github_list_issues".into(),
                description: "List issues and pull requests in a GitHub repository (newest 30)."
                    .into(),
                input_schema: json!({"type":"object","required":["repository"],"properties":{
                    "repository": repository,
                    "state": {"type":"string","enum":["open","closed","all"],"default":"open"},
                }}),
            });
            tools.push(Tool {
                name: "github_read_issue".into(),
                description: "Read one GitHub issue (or pull) with its comments.".into(),
                input_schema: json!({"type":"object","required":["repository","number"],"properties":{
                    "repository": repository,
                    "number": {"type":"integer","minimum":1},
                }}),
            });
            tools.push(Tool {
                name: "github_create_pull_request".into(),
                description: "Create a pull request on GitHub from an existing pushed branch.".into(),
                input_schema: json!({"type":"object","required":["repository","head","base","title"],"properties":{
                    "repository": repository,
                    "head": {"type":"string","description":"branch with the changes"},
                    "base": {"type":"string","description":"branch to merge into, usually main"},
                    "title": {"type":"string"},
                    "body": {"type":"string"},
                }}),
            });
        }
        if !self.repositories.is_empty() {
            let folders: Vec<String> = self
                .repositories
                .iter()
                .map(|folder| folder.display().to_string())
                .collect();
            tools.push(Tool {
                name: "git_commit".into(),
                description: format!(
                    "Create a git commit of every change in a local repository folder. Allowed folders: {}",
                    folders.join(", ")
                ),
                input_schema: json!({"type":"object","required":["folder","message"],"properties":{
                    "folder": {"type":"string","enum": folders},
                    "message": {"type":"string","description":"commit message"},
                }}),
            });
        }
        Ok(tools)
    }

    async fn call(
        &mut self,
        name: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> Result<ToolResult, McpError> {
        let result = match name {
            "github_list_issues" => self.list_issues(&args).await,
            "github_read_issue" => self.read_issue(&args).await,
            "github_create_pull_request" => self.create_pull_request(&args).await,
            "git_commit" => self.commit(&args, cancel).await,
            _ => return Err(McpError::Invalid),
        };
        Ok(result.map_or_else(
            |failure| failure,
            |content| ToolResult {
                content,
                is_error: false,
            },
        ))
    }

    async fn close(&mut self) -> Result<(), McpError> {
        Ok(())
    }
}

/// Connects native connectors in-process and hands MCP servers to the SDK.
pub struct NativeFactory {
    pub home: PathBuf,
}
#[async_trait::async_trait]
impl ClientFactory for NativeFactory {
    async fn connect(
        &self,
        connector: &Connector,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn Connection>, McpError> {
        match connector {
            Connector::Github { repositories, .. } => {
                connector.validate()?;
                Ok(Box::new(GithubConnection::new(
                    &self.home,
                    TokenStore::new(&self.home).load(),
                    repositories,
                    Box::new(ReqwestGithub::new()?),
                )))
            }
            _ => crate::mcp_sdk::SdkFactory.connect(connector, cancel).await,
        }
    }
}

fn repository(args: &Value) -> Result<&str, ToolResult> {
    args["repository"]
        .as_str()
        .filter(|value| valid_repository(value))
        .ok_or_else(|| error("repository must be owner/name"))
}

fn error(message: &str) -> ToolResult {
    ToolResult {
        content: message.into(),
        is_error: true,
    }
}

fn message(body: &Value) -> String {
    text(&body["message"], 300)
}

fn clip(value: &str, limit: usize) -> String {
    if value.chars().count() > limit {
        format!("{}…", value.chars().take(limit).collect::<String>())
    } else {
        value.into()
    }
}

fn text(value: &Value, limit: usize) -> String {
    let raw = match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    // Keep line breaks; drop other control and bidi characters.
    let lines: Vec<String> = raw
        .lines()
        .map(rigspark_core::reports::strip_control)
        .collect();
    clip(&lines.join("\n"), limit)
}
