//! Native source control for chat sessions: RigSpark talks to GitHub's REST API itself
//! and commits in local repositories the user allowed. It appears as a connector, so
//! every session sees its tools and every call goes through the same approval as MCP
//! tools (reads ask once per session; creating a pull request or a commit asks each time).
//!
//! Access is on a need basis, at two levels:
//! - On GitHub, people install the RigSpark Community GitHub App on the repositories
//!   they choose, so its sign-in can never reach any other repository. The app's own
//!   ceiling is Contents and Pull requests read and write, Issues and Metadata read.
//! - In RigSpark, each of those repositories is set to read-only or read and write
//!   (`RepoAccess`). Nothing is allowed until a repository is listed; pushing and opening
//!   pull requests need read and write.
//!
//! The GitHub credentials live in `<home>/github-token` (owner-only), never in
//! `connectors.json`, and are sent only to github.com. App sign-ins last 8 hours and are
//! renewed with their refresh token, which device sign-ins may do without a client
//! secret. Commits use the same hooks as agents launched by `rigspark connect`, so they
//! carry RigSpark's `Co-Authored-By` and `Signed-off-by` trailers.

use crate::mcp::{ClientFactory, Connection, Connector, McpError, Tool, ToolResult};
use serde::{Deserialize, Serialize};
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
/// Renew a sign-in this long before it expires.
const RENEW_EARLY_SECS: u64 = 300;
const MAX_GRANTS: usize = 200;

/// What RigSpark may do in one GitHub repository.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    /// List and read issues and pull requests.
    Read,
    /// Also push branches and open pull requests.
    Write,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepoAccess {
    pub repository: String,
    pub access: Access,
}

/// At most 200 repositories, each `owner/name` and listed once (GitHub names are
/// case-insensitive).
pub fn validate_access(grants: &[RepoAccess]) -> Result<(), McpError> {
    if grants.len() > MAX_GRANTS {
        return Err(McpError::Invalid);
    }
    let mut seen = std::collections::BTreeSet::new();
    for grant in grants {
        if !valid_repository(&grant.repository) || !seen.insert(grant.repository.to_lowercase()) {
            return Err(McpError::Invalid);
        }
    }
    Ok(())
}

/// A GitHub sign-in: a token, and for GitHub App sign-ins how to renew it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Credentials {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}
impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credentials")
            .field("renewable", &self.refresh_token.is_some())
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}
impl Credentials {
    /// A token that does not expire, such as a fine-grained personal access token.
    pub fn token(token: &str) -> Self {
        Self {
            access_token: token.trim().into(),
            refresh_token: None,
            expires_at: None,
        }
    }
    fn valid(&self) -> bool {
        valid_token(&self.access_token) && self.refresh_token.as_deref().is_none_or(valid_token)
    }
    /// Expires within five minutes (or already has).
    pub fn expiring(&self, now: u64) -> bool {
        self.expires_at
            .is_some_and(|at| now.saturating_add(RENEW_EARLY_SECS) >= at)
    }
    /// A GitHub App sign-in, which only reaches repositories the app is installed on.
    pub fn app_sign_in(&self) -> bool {
        self.access_token.starts_with("ghu_")
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The GitHub credentials, kept in an owner-only file under the RigSpark home.
pub struct TokenStore {
    path: PathBuf,
}
impl TokenStore {
    pub fn new(home: &Path) -> Self {
        Self {
            path: home.join(TOKEN_FILE),
        }
    }
    /// Reads a saved sign-in, or a bare token saved by an earlier version.
    pub fn load(&self) -> Option<Credentials> {
        let raw = crate::state::secure_read(&self.path, 4096, true).ok()?;
        let raw = raw.trim();
        let credentials = if raw.starts_with('{') {
            serde_json::from_str(raw).ok()?
        } else {
            Credentials::token(raw)
        };
        credentials.valid().then_some(credentials)
    }
    pub fn save(&self, credentials: &Credentials) -> Result<(), McpError> {
        let mut credentials = credentials.clone();
        credentials.access_token = credentials.access_token.trim().into();
        if !credentials.valid() {
            return Err(McpError::Invalid);
        }
        let raw = serde_json::to_vec(&credentials).map_err(|_| McpError::Invalid)?;
        if let Some(parent) = self.path.parent() {
            crate::memory::owned_directory(parent).map_err(|_| McpError::Storage)?;
        }
        crate::memory::atomic_file(&self.path, &raw).map_err(|_| McpError::Storage)
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

/// The RigSpark Community GitHub App (device flow; no client secret ships in the app).
/// Its sign-ins reach only repositories the person installed it on.
pub const GITHUB_CLIENT_ID: &str = "Iv23liaO72v1ZQNBqeq2";
pub const GITHUB_APP_SLUG: &str = "rigspark-community";
pub const DEVICE_PAGE: &str = "https://github.com/login/device";
/// Where people choose which repositories the app may reach.
pub const INSTALL_PAGE: &str = "https://github.com/apps/rigspark-community/installations/new";

/// `RIGSPARK_GITHUB_CLIENT_ID` overrides the built-in client for forks and testing.
pub fn client_id() -> String {
    std::env::var("RIGSPARK_GITHUB_CLIENT_ID")
        .ok()
        .map(|id| id.trim().to_owned())
        .filter(|id| valid_client_id(id))
        .unwrap_or_else(|| GITHUB_CLIENT_ID.into())
}

fn valid_client_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.')
}

/// github.com's OAuth endpoints (`/login/device/code`, `/login/oauth/access_token`).
#[async_trait::async_trait]
pub trait GithubSignIn: Send + Sync {
    async fn post_form(&self, path: &str, form: &[(&str, &str)]) -> Result<HttpReply, McpError>;
}

#[async_trait::async_trait]
impl GithubSignIn for ReqwestGithub {
    async fn post_form(&self, path: &str, form: &[(&str, &str)]) -> Result<HttpReply, McpError> {
        if !matches!(path, "/login/device/code" | "/login/oauth/access_token") {
            return Err(McpError::Invalid);
        }
        let mut response = self
            .client
            .post(format!("https://github.com{path}"))
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::USER_AGENT, "rigspark-community")
            .form(form)
            .send()
            .await
            .map_err(|_| McpError::Transport)?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| McpError::Transport)? {
            if bytes.len() + chunk.len() > 65_536 {
                return Err(McpError::Limit);
            }
            bytes.extend_from_slice(&chunk);
        }
        let body = serde_json::from_slice(&bytes).map_err(|_| McpError::Invalid)?;
        Ok(HttpReply { status, body })
    }
}

/// What the person sees: a short code to enter at github.com/login/device.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    /// Not approved yet; poll again after `interval` seconds.
    Pending {
        interval: u64,
    },
    Token(Credentials),
    Denied,
    Expired,
}

/// A token reply from `/login/oauth/access_token`, with its refresh token if any.
fn credentials_from(body: &Value, now: u64) -> Result<Credentials, McpError> {
    let token = body["access_token"].as_str().ok_or(McpError::Invalid)?;
    let credentials = Credentials {
        access_token: token.into(),
        refresh_token: body["refresh_token"].as_str().map(String::from),
        expires_at: body["expires_in"]
            .as_u64()
            .map(|seconds| now.saturating_add(seconds.clamp(60, 366 * 86_400))),
    };
    credentials
        .valid()
        .then_some(credentials)
        .ok_or(McpError::Invalid)
}

/// The app has no permission scopes to ask for: what it may do is set on the app, and
/// where, by the repositories it is installed on.
pub async fn start_sign_in(
    http: &dyn GithubSignIn,
    client_id: &str,
) -> Result<DeviceCode, McpError> {
    let reply = http
        .post_form("/login/device/code", &[("client_id", client_id)])
        .await?;
    let body = &reply.body;
    let text = |key: &str| body[key].as_str().unwrap_or_default().to_owned();
    let code = DeviceCode {
        device_code: text("device_code"),
        user_code: text("user_code"),
        verification_uri: text("verification_uri"),
        interval: body["interval"].as_u64().unwrap_or(5).clamp(1, 60),
        expires_in: body["expires_in"].as_u64().unwrap_or(900).clamp(60, 3600),
    };
    let user_code_ok = (4..=16).contains(&code.user_code.len())
        && code
            .user_code
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-');
    let device_code_ok = (1..=256).contains(&code.device_code.len())
        && code
            .device_code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric());
    // Only ever send the person to GitHub's own device page.
    if reply.status != 200
        || !user_code_ok
        || !device_code_ok
        || code.verification_uri != DEVICE_PAGE
    {
        return Err(McpError::Invalid);
    }
    Ok(code)
}

pub async fn poll_sign_in(
    http: &dyn GithubSignIn,
    client_id: &str,
    device_code: &str,
) -> Result<SignIn, McpError> {
    if device_code.is_empty()
        || device_code.len() > 256
        || !device_code.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Err(McpError::Invalid);
    }
    let reply = http
        .post_form(
            "/login/oauth/access_token",
            &[
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ],
        )
        .await?;
    if reply.body.get("access_token").is_some() {
        return credentials_from(&reply.body, now()).map(SignIn::Token);
    }
    match reply.body["error"].as_str() {
        Some("authorization_pending") => Ok(SignIn::Pending { interval: 5 }),
        Some("slow_down") => Ok(SignIn::Pending {
            interval: reply.body["interval"].as_u64().unwrap_or(10).clamp(1, 60),
        }),
        Some("expired_token") => Ok(SignIn::Expired),
        Some("access_denied") => Ok(SignIn::Denied),
        _ => Err(McpError::Invalid),
    }
}

/// Renews an app sign-in. GitHub replaces the refresh token on every renewal.
pub async fn renew_sign_in(
    http: &dyn GithubSignIn,
    client_id: &str,
    refresh_token: &str,
    now: u64,
) -> Result<Credentials, McpError> {
    if !valid_token(refresh_token) {
        return Err(McpError::Invalid);
    }
    let reply = http
        .post_form(
            "/login/oauth/access_token",
            &[
                ("client_id", client_id),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
            ],
        )
        .await?;
    match reply.body["error"].as_str() {
        // Revoked, expired after six months, or already used: sign in again.
        Some(_) => Err(McpError::Approval),
        None => credentials_from(&reply.body, now),
    }
}

/// The saved sign-in, renewed (and saved again) when it is about to expire. `None` when
/// nothing is saved; `Err(Approval)` when it expired and cannot be renewed.
pub async fn fresh_credentials(
    tokens: &TokenStore,
    http: &dyn GithubSignIn,
    client_id: &str,
    now: u64,
) -> Result<Option<Credentials>, McpError> {
    let Some(credentials) = tokens.load() else {
        return Ok(None);
    };
    if !credentials.expiring(now) {
        return Ok(Some(credentials));
    }
    let Some(refresh_token) = credentials.refresh_token.as_deref() else {
        return Err(McpError::Approval);
    };
    let renewed = renew_sign_in(http, client_id, refresh_token, now).await?;
    tokens.save(&renewed)?;
    Ok(Some(renewed))
}

/// A repository the sign-in can reach, and whether the person may push to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantedRepository {
    pub repository: String,
    pub private: bool,
    pub can_write: bool,
}

/// Repositories this sign-in can reach. For an app sign-in: those the app is installed
/// on that the person can see. For a personal access token: those GitHub lists for it.
pub async fn granted_repositories(
    http: &dyn GithubHttp,
    credentials: &Credentials,
) -> Result<Vec<GrantedRepository>, McpError> {
    const PAGES: u32 = 10;
    let token = credentials.access_token.as_str();
    let get = |path: String| async move {
        let reply = http.send("GET", &path, token, None).await?;
        match reply.status {
            200 => Ok(reply.body),
            401 | 403 => Err(McpError::Approval),
            _ => Err(McpError::Invalid),
        }
    };
    let mut pages = Vec::new();
    if credentials.app_sign_in() {
        let installations = get("/user/installations?per_page=100".into()).await?;
        let ids: Vec<u64> = installations["installations"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|installation| installation["id"].as_u64())
            .take(100)
            .collect();
        for id in ids {
            for page in 1..=PAGES {
                let body = get(format!(
                    "/user/installations/{id}/repositories?per_page=100&page={page}"
                ))
                .await?;
                let repositories = body["repositories"].as_array().cloned().unwrap_or_default();
                let full = repositories.len() == 100;
                pages.extend(repositories);
                if !full {
                    break;
                }
            }
        }
    } else {
        for page in 1..=PAGES {
            let body = get(format!("/user/repos?per_page=100&page={page}")).await?;
            let repositories = body.as_array().cloned().unwrap_or_default();
            let full = repositories.len() == 100;
            pages.extend(repositories);
            if !full {
                break;
            }
        }
    }
    let mut granted: Vec<GrantedRepository> = pages
        .iter()
        .filter_map(|repository| {
            let name = repository["full_name"].as_str()?;
            valid_repository(name).then(|| GrantedRepository {
                repository: name.into(),
                private: repository["private"].as_bool().unwrap_or(true),
                can_write: repository["permissions"]["push"].as_bool().unwrap_or(false),
            })
        })
        .collect();
    granted.sort_by_key(|entry| entry.repository.to_lowercase());
    granted.dedup_by_key(|entry| entry.repository.to_lowercase());
    Ok(granted)
}

/// Keeps the requested access only for repositories the sign-in can reach, and read and
/// write only where the person may push. Returns the access to save and what was refused.
pub fn settle_access(
    requested: &[RepoAccess],
    granted: &[GrantedRepository],
) -> (Vec<RepoAccess>, Vec<String>) {
    let mut kept = Vec::new();
    let mut refused = Vec::new();
    for want in requested {
        match granted
            .iter()
            .find(|entry| entry.repository.eq_ignore_ascii_case(&want.repository))
        {
            Some(entry) if want.access == Access::Read || entry.can_write => {
                kept.push(RepoAccess {
                    repository: entry.repository.clone(),
                    access: want.access,
                })
            }
            _ => refused.push(want.repository.clone()),
        }
    }
    kept.sort_by_key(|entry| entry.repository.to_lowercase());
    kept.dedup_by_key(|entry| entry.repository.to_lowercase());
    (kept, refused)
}

/// `owner/name` of a github.com remote URL (HTTPS or SSH), or `None` for anything else.
pub fn github_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let path = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else {
        let parsed = url::Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "https" | "ssh")
            || parsed.host_str()? != "github.com"
            || parsed.port().is_some()
        {
            return None;
        }
        return github_remote_path(parsed.path().trim_start_matches('/'));
    };
    github_remote_path(path)
}

fn github_remote_path(path: &str) -> Option<String> {
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    valid_repository(path).then(|| path.to_owned())
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

const MANAGE: &str = "Connectors > Source control > GitHub > Manage";

/// Renews the connection's app sign-in before it expires.
pub struct Renewal {
    pub sign_in: Box<dyn GithubSignIn>,
    pub client_id: String,
}

pub struct GithubConnection {
    home: PathBuf,
    credentials: Option<Credentials>,
    repositories: Vec<PathBuf>,
    access: Vec<RepoAccess>,
    http: Box<dyn GithubHttp>,
    renewal: Option<Renewal>,
}

impl GithubConnection {
    /// `repositories` are local folders for commits; `access` the GitHub repositories
    /// RigSpark may use, and how.
    pub fn new(
        home: &Path,
        credentials: Option<Credentials>,
        repositories: &[String],
        access: &[RepoAccess],
        http: Box<dyn GithubHttp>,
    ) -> Self {
        Self {
            home: home.into(),
            credentials,
            repositories: repositories
                .iter()
                .filter_map(|folder| std::fs::canonicalize(folder).ok())
                .collect(),
            access: access.to_vec(),
            http,
            renewal: None,
        }
    }

    pub fn with_renewal(mut self, renewal: Renewal) -> Self {
        self.renewal = Some(renewal);
        self
    }

    fn token(&self) -> Result<&str, ToolResult> {
        self.credentials
            .as_ref()
            .map(|credentials| credentials.access_token.as_str())
            .ok_or_else(|| {
                error(&format!(
                    "GitHub isn't connected. Connect it under {MANAGE}."
                ))
            })
    }

    /// Renews an expiring app sign-in and saves it, so later sessions get the new one.
    async fn renew(&mut self) -> Result<(), ToolResult> {
        let Some(credentials) = &self.credentials else {
            return Ok(());
        };
        let now = now();
        if !credentials.expiring(now) {
            return Ok(());
        }
        let expired = || {
            error(&format!(
                "The GitHub sign-in expired. Sign in again under {MANAGE}."
            ))
        };
        let (Some(renewal), Some(refresh)) = (&self.renewal, credentials.refresh_token.clone())
        else {
            return Err(expired());
        };
        let renewed = renew_sign_in(renewal.sign_in.as_ref(), &renewal.client_id, &refresh, now)
            .await
            .map_err(|_| expired())?;
        TokenStore::new(&self.home)
            .save(&renewed)
            .map_err(|_| error("Could not save the renewed GitHub sign-in."))?;
        self.credentials = Some(renewed);
        Ok(())
    }

    fn level(&self, repository: &str) -> Option<Access> {
        self.access
            .iter()
            .find(|grant| grant.repository.eq_ignore_ascii_case(repository))
            .map(|grant| grant.access)
    }

    /// Refuses a repository RigSpark was not given, or one that is read-only when the
    /// call needs to write.
    fn require(&self, repository: &str, needed: Access) -> Result<(), ToolResult> {
        match self.level(repository) {
            None => Err(error(&format!(
                "RigSpark has no access to {repository}. Add it under {MANAGE}."
            ))),
            Some(Access::Read) if needed == Access::Write => Err(error(&format!(
                "{repository} is read-only in RigSpark. Set it to Read & write under {MANAGE} to push or open pull requests."
            ))),
            Some(_) => Ok(()),
        }
    }

    fn granted(&self, needed: Access) -> Vec<String> {
        self.access
            .iter()
            .filter(|grant| grant.access >= needed)
            .map(|grant| grant.repository.clone())
            .collect()
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
        self.require(repository, Access::Read)?;
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
        self.require(repository, Access::Read)?;
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
        self.require(repository, Access::Write)?;
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

    fn allowed_folder(&self, args: &Value) -> Result<PathBuf, ToolResult> {
        let folder = args["folder"]
            .as_str()
            .ok_or_else(|| error("folder is required"))?;
        std::fs::canonicalize(folder)
            .ok()
            .filter(|folder| self.repositories.contains(folder))
            .ok_or_else(|| {
                error(
                    "That folder isn't allowed. Add it under Connectors > Source control > GitHub.",
                )
            })
    }

    async fn git(
        &self,
        folder: &Path,
        env: &std::collections::BTreeMap<String, String>,
        args: &[&str],
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<String, ToolResult> {
        let binary = crate::process_control::resolve_binary("git")
            .map_err(|_| error("git is not installed"))?;
        let run = tokio::process::Command::new(binary)
            .arg("-C")
            .arg(folder)
            .args(args)
            .env_clear()
            .envs(env)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(error("cancelled")),
            output = tokio::time::timeout(timeout, run) => output
                .map_err(|_| error("git timed out"))?
                .map_err(|_| error("git could not run"))?,
        };
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        // Never echo the token back, whatever git prints.
        let combined = match &self.credentials {
            Some(credentials) => combined.replace(credentials.access_token.as_str(), "••••••"),
            None => combined,
        };
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

    async fn commit(&self, args: &Value, cancel: &CancellationToken) -> Result<String, ToolResult> {
        let folder = self.allowed_folder(args)?;
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
        let minute = Duration::from_secs(60);
        self.git(&folder, &env, &["add", "--all"], minute, cancel)
            .await?;
        self.git(
            &folder,
            &env,
            &["commit", "--quiet", "--message", message],
            minute,
            cancel,
        )
        .await?;
        let summary = self
            .git(
                &folder,
                &env,
                &["log", "-1", "--format=%h %s%n%n%b"],
                minute,
                cancel,
            )
            .await?;
        Ok(format!(
            "Committed in {}:\n{}",
            folder.display(),
            summary.trim()
        ))
    }

    async fn push(&self, args: &Value, cancel: &CancellationToken) -> Result<String, ToolResult> {
        let folder = self.allowed_folder(args)?;
        // `owner:branch` is only for a pull request's head; in a push a colon would be
        // read as part of the refspec, and git refuses it in branch names anyway.
        let branch = args["branch"]
            .as_str()
            .filter(|branch| valid_branch(branch) && !branch.contains(':'))
            .ok_or_else(|| error("branch must be a branch name"))?;
        // Changes reach the default branch through a pull request, never a direct push.
        if ["main", "master", "HEAD"].contains(&branch) {
            return Err(error(
                "Push to a new branch and open a pull request instead of pushing to the default branch.",
            ));
        }
        let mut env = crate::process_control::minimal_env();
        env.insert("GIT_TERMINAL_PROMPT".into(), "0".into());
        // Where git will actually push: every push URL (`pushurl` wins over `url`), with
        // `insteadOf` and `pushInsteadOf` rewrites applied.
        let destinations = self
            .git(
                &folder,
                &env,
                &["remote", "get-url", "--push", "--all", "origin"],
                Duration::from_secs(10),
                cancel,
            )
            .await?;
        // A GitHub remote (HTTPS or SSH) must be a repository set to read and write, so
        // the person's own SSH keys or saved logins can't reach past what they chose.
        let mut github = false;
        for url in destinations.lines().filter(|url| !url.trim().is_empty()) {
            if let Some(repository) = github_remote(url) {
                self.require(&repository, Access::Write)?;
                github = true;
            } else if url.contains("github.com") {
                return Err(error(
                    "The origin remote points at GitHub in a form RigSpark can't check. Use https://github.com/owner/name or git@github.com:owner/name.",
                ));
            }
        }
        if let (true, Some(credentials)) = (github, &self.credentials) {
            let token = &credentials.access_token;
            // HTTPS remotes on github.com authenticate with the connected token; it goes
            // through the environment, never the command line.
            env.insert("GIT_CONFIG_COUNT".into(), "1".into());
            env.insert(
                "GIT_CONFIG_KEY_0".into(),
                "http.https://github.com/.extraheader".into(),
            );
            env.insert(
                "GIT_CONFIG_VALUE_0".into(),
                format!(
                    "AUTHORIZATION: basic {}",
                    base64(format!("x-access-token:{token}").as_bytes())
                ),
            );
        }
        let target = format!("HEAD:refs/heads/{branch}");
        let output = self
            .git(
                &folder,
                &env,
                &["push", "--porcelain", "origin", &target],
                Duration::from_secs(120),
                cancel,
            )
            .await?;
        Ok(format!(
            "Pushed to branch {branch} on origin.\n{}",
            clip(output.trim(), 2000)
        ))
    }
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(TABLE[(value >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[async_trait::async_trait]
impl Connection for GithubConnection {
    async fn tools(&mut self, _: &CancellationToken) -> Result<Vec<Tool>, McpError> {
        let mut tools = Vec::new();
        let readable = self.granted(Access::Read);
        let writable = self.granted(Access::Write);
        let choice = |names: &[String]| json!({"type":"string","enum": names,"description":"owner/name of a repository RigSpark may use"});
        if self.credentials.is_some() && !readable.is_empty() {
            let repository = choice(&readable);
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
        }
        if self.credentials.is_some() && !writable.is_empty() {
            let repository = choice(&writable);
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
            tools.push(Tool {
                name: "git_push".into(),
                description: "Upload the current commit of an allowed local folder to a new branch on its origin remote, so a pull request can be opened from it. Not for main or master.".into(),
                input_schema: json!({"type":"object","required":["folder","branch"],"properties":{
                    "folder": {"type":"string","enum": folders},
                    "branch": {"type":"string","description":"branch to create or update, for example rigspark/hello"},
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
        if (name.starts_with("github_") || name == "git_push")
            && let Err(failure) = self.renew().await
        {
            return Ok(failure);
        }
        let result = match name {
            "github_list_issues" => self.list_issues(&args).await,
            "github_read_issue" => self.read_issue(&args).await,
            "github_create_pull_request" => self.create_pull_request(&args).await,
            "git_commit" => self.commit(&args, cancel).await,
            "git_push" => self.push(&args, cancel).await,
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
            Connector::Github {
                repositories,
                access,
                ..
            } => {
                connector.validate()?;
                Ok(Box::new(
                    GithubConnection::new(
                        &self.home,
                        TokenStore::new(&self.home).load(),
                        repositories,
                        access,
                        Box::new(ReqwestGithub::new()?),
                    )
                    .with_renewal(Renewal {
                        sign_in: Box::new(ReqwestGithub::new()?),
                        client_id: client_id(),
                    }),
                ))
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

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        for (input, expected) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("x-access-token:ghp_abc", "eC1hY2Nlc3MtdG9rZW46Z2hwX2FiYw=="),
        ] {
            assert_eq!(super::base64(input.as_bytes()), expected, "{input}");
        }
    }
}
