use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("invalid MCP connector or response")]
    Invalid,
    #[error("MCP connector unavailable")]
    Unavailable,
    #[error("MCP request failed")]
    Transport,
    #[error("MCP request cancelled")]
    Cancelled,
    #[error("MCP resource limit exceeded")]
    Limit,
    #[error("MCP approval missing, stale, or mismatched")]
    Approval,
    #[error("MCP persistence failed")]
    Storage,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase", deny_unknown_fields)]
pub enum Connector {
    Stdio {
        id: String,
        name: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        env: Option<BTreeMap<String, String>>,
    },
    Http {
        id: String,
        name: String,
        url: String,
    },
    /// RigSpark's native GitHub integration (see `source_control`). The token is kept
    /// outside this file; `repositories` are local folders the user allowed for commits,
    /// and `access` the GitHub repositories RigSpark may use (none until chosen).
    Github {
        id: String,
        name: String,
        #[serde(default)]
        repositories: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        access: Vec<crate::source_control::RepoAccess>,
    },
}
impl std::fmt::Debug for Connector {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Connector")
            .field("id", &self.id())
            .finish_non_exhaustive()
    }
}
pub fn loopback_url(raw: &str) -> Result<url::Url, McpError> {
    let url = url::Url::parse(raw).map_err(|_| McpError::Invalid)?;
    let local = match url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        Some(url::Host::Domain(domain)) => domain == "localhost",
        None => false,
    };
    if !local
        || !["http", "https"].contains(&url.scheme())
        || !url.username().is_empty()
        || url.password().is_some()
        || raw.len() > 2048
        || raw.chars().any(char::is_control)
    {
        return Err(McpError::Invalid);
    }
    Ok(url)
}
impl Connector {
    pub fn id(&self) -> &str {
        match self {
            Self::Stdio { id, .. } | Self::Http { id, .. } | Self::Github { id, .. } => id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            Self::Stdio { name, .. } | Self::Http { name, .. } | Self::Github { name, .. } => name,
        }
    }
    pub fn validate(&self) -> Result<(), McpError> {
        let id = self.id();
        if id.is_empty()
            || id.len() > 64
            || !id.as_bytes()[0].is_ascii_alphanumeric()
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || self.name().trim().is_empty()
            || self.name().len() > 120
        {
            return Err(McpError::Invalid);
        }
        match self {
            Self::Http { url, .. } => {
                loopback_url(url)?;
            }
            Self::Github {
                id,
                repositories,
                access,
                ..
            } => {
                if id != crate::source_control::GITHUB_ID {
                    return Err(McpError::Invalid);
                }
                crate::source_control::validate_repositories(repositories)?;
                crate::source_control::validate_access(access)?;
            }
            Self::Stdio {
                command, args, env, ..
            } => {
                if command.trim().is_empty()
                    || command.len() > 512
                    || command.contains('\0')
                    || args.len() > 64
                    || args.iter().any(|arg| arg.len() > 512 || arg.contains('\0'))
                    || env.as_ref().is_some_and(|env| {
                        env.len() > 64
                            || env.iter().any(|(key, value)| {
                                key.is_empty()
                                    || key.len() > 256
                                    || key.contains(['\0', '='])
                                    || value.len() > 4096
                                    || value.contains('\0')
                            })
                    })
                {
                    return Err(McpError::Invalid);
                }
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectorFile {
    pub schema_version: u8,
    pub connectors: Vec<Connector>,
}
impl Default for ConnectorFile {
    fn default() -> Self {
        Self {
            schema_version: 1,
            connectors: Vec::new(),
        }
    }
}
impl ConnectorFile {
    pub fn parse(raw: &str) -> Result<Self, McpError> {
        if raw.len() > 65536 {
            return Err(McpError::Limit);
        }
        if raw.trim().is_empty() {
            return Ok(Self::default());
        }
        let file: Self = serde_json::from_str(raw).map_err(|_| McpError::Invalid)?;
        file.validate()?;
        Ok(file)
    }
    pub fn validate(&self) -> Result<(), McpError> {
        if self.schema_version != 1 || self.connectors.len() > 32 {
            return Err(McpError::Invalid);
        }
        let mut ids = HashSet::new();
        for connector in &self.connectors {
            connector.validate()?;
            if !ids.insert(connector.id()) {
                return Err(McpError::Invalid);
            }
        }
        Ok(())
    }
}
pub struct ConnectorStore {
    home: PathBuf,
}
impl ConnectorStore {
    pub fn new(home: &Path) -> Self {
        Self { home: home.into() }
    }
    pub fn load(&self) -> Result<ConnectorFile, McpError> {
        match crate::state::secure_read(&self.home.join("connectors.json"), 65536, false) {
            Ok(raw) => ConnectorFile::parse(&raw),
            Err(error) if error.kind == "absent" => Ok(ConnectorFile::default()),
            Err(_) => Err(McpError::Storage),
        }
    }
    pub fn save(&self, file: &ConnectorFile) -> Result<(), McpError> {
        file.validate()?;
        let bytes = format!(
            "{}\n",
            serde_json::to_string_pretty(file).map_err(|_| McpError::Invalid)?
        );
        if bytes.len() > 65536 {
            return Err(McpError::Limit);
        }
        crate::memory::owned_directory(&self.home).map_err(|_| McpError::Storage)?;
        let store = crate::state::StateStore::new(
            crate::state::Config::from_home(&self.home).map_err(|_| McpError::Storage)?,
        );
        let guard = store
            .lock(Duration::from_secs(10))
            .map_err(|_| McpError::Storage)?;
        crate::memory::atomic_file(&self.home.join("connectors.json"), bytes.as_bytes())
            .map_err(|_| McpError::Storage)?;
        guard.release().map_err(|_| McpError::Storage)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}
#[async_trait::async_trait]
pub trait Connection: Send + Sync {
    async fn tools(&mut self, cancel: &CancellationToken) -> Result<Vec<Tool>, McpError>;
    async fn call(
        &mut self,
        name: &str,
        args: Value,
        cancel: &CancellationToken,
    ) -> Result<ToolResult, McpError>;
    async fn close(&mut self) -> Result<(), McpError>;
}
#[async_trait::async_trait]
pub trait ClientFactory: Send + Sync {
    async fn connect(
        &self,
        connector: &Connector,
        cancel: &CancellationToken,
    ) -> Result<Box<dyn Connection>, McpError>;
}
struct Entry {
    connection: Box<dyn Connection>,
    tools: Vec<Tool>,
    generation: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewedCall {
    pub connector_id: String,
    pub tool: String,
    #[serde(serialize_with = "crate::tool_policy::serialize_arguments")]
    pub arguments: Value,
    pub risk: crate::tool_policy::ToolRisk,
    #[serde(skip)]
    tool_identity: String,
    #[serde(skip)]
    fingerprint: String,
    #[serde(skip)]
    generation: String,
}
pub struct Approval {
    reviewed: ReviewedCall,
    manager: String,
}
pub struct Manager {
    definitions: ConnectorFile,
    entries: BTreeMap<String, Entry>,
    identity: String,
    failures: BTreeMap<String, String>,
}
#[derive(Serialize)]
pub struct ConnectorView {
    pub id: String,
    pub name: String,
    pub transport: &'static str,
    pub target: String,
    pub status: &'static str,
    pub tools: Vec<Tool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
fn fingerprint(value: &impl Serialize) -> Result<String, McpError> {
    let value = serde_json::to_value(value).map_err(|_| McpError::Invalid)?;
    let bytes = serde_json::to_vec(&value).map_err(|_| McpError::Invalid)?;
    if bytes.len() > 1048576 {
        return Err(McpError::Limit);
    }
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
impl Manager {
    pub fn new(definitions: ConnectorFile) -> Result<Self, McpError> {
        definitions.validate()?;
        Ok(Self {
            definitions,
            entries: BTreeMap::new(),
            identity: uuid::Uuid::new_v4().to_string(),
            failures: BTreeMap::new(),
        })
    }
    pub fn snapshot(&self) -> ConnectorFile {
        self.definitions.clone()
    }
    pub fn list(&self) -> Vec<ConnectorView> {
        self.definitions
            .connectors
            .iter()
            .map(|connector| {
                let entry = self.entries.get(connector.id());
                let error = self.failures.get(connector.id()).cloned();
                let (transport, target) = match connector {
                    Connector::Stdio { command, .. } => ("stdio", command.clone()),
                    Connector::Http { url, .. } => ("http", url.clone()),
                    Connector::Github {
                        repositories,
                        access,
                        ..
                    } => (
                        "github",
                        format!(
                            "api.github.com · {} repo(s) · {} folder(s)",
                            access.len(),
                            repositories.len()
                        ),
                    ),
                };
                ConnectorView {
                    id: connector.id().into(),
                    name: connector.name().into(),
                    transport,
                    target,
                    status: if entry.is_some() {
                        "connected"
                    } else if error.is_some() {
                        "error"
                    } else {
                        "disconnected"
                    },
                    tools: entry.map(|entry| entry.tools.clone()).unwrap_or_default(),
                    error,
                }
            })
            .collect()
    }
    pub async fn add(
        &mut self,
        connector: Connector,
        store: &ConnectorStore,
    ) -> Result<(), McpError> {
        connector.validate()?;
        if self
            .definitions
            .connectors
            .iter()
            .any(|entry| entry.id() == connector.id())
        {
            return Err(McpError::Invalid);
        }
        let mut file = self.snapshot();
        file.connectors.push(connector);
        self.replace(file, store).await
    }
    pub async fn remove(&mut self, id: &str, store: &ConnectorStore) -> Result<(), McpError> {
        if !self
            .definitions
            .connectors
            .iter()
            .any(|entry| entry.id() == id)
        {
            return Err(McpError::Invalid);
        }
        let mut file = self.snapshot();
        file.connectors.retain(|entry| entry.id() != id);
        self.replace(file, store).await
    }
    pub fn tools(&self) -> Vec<(String, Tool)> {
        self.definitions
            .connectors
            .iter()
            .filter_map(|connector| {
                self.entries
                    .get(connector.id())
                    .map(|entry| (connector.id(), entry))
            })
            .flat_map(|(id, entry)| {
                entry
                    .tools
                    .iter()
                    .map(move |tool| (id.to_owned(), tool.clone()))
            })
            .collect()
    }
    pub async fn connect(
        &mut self,
        id: &str,
        factory: &dyn ClientFactory,
        cancel: &CancellationToken,
    ) -> Result<(), McpError> {
        let connector = self
            .definitions
            .connectors
            .iter()
            .find(|connector| connector.id() == id)
            .ok_or(McpError::Invalid)?;
        let connection = tokio::select! {biased;_=cancel.cancelled()=>Err(McpError::Cancelled),result=tokio::time::timeout(Duration::from_secs(15),factory.connect(connector,cancel))=>result.map_err(|_|McpError::Transport).and_then(|result|result)};
        let result = match connection {
            Ok(connection) => self.attach(id, connection, cancel).await,
            Err(error) => Err(error),
        };
        if let Err(error) = &result {
            self.failures.insert(id.into(), error.to_string());
        }
        result
    }
    pub async fn attach(
        &mut self,
        id: &str,
        mut connection: Box<dyn Connection>,
        cancel: &CancellationToken,
    ) -> Result<(), McpError> {
        if !self
            .definitions
            .connectors
            .iter()
            .any(|connector| connector.id() == id)
        {
            let _ = tokio::time::timeout(Duration::from_secs(5), connection.close()).await;
            return Err(McpError::Invalid);
        }
        let tools = tokio::select! {biased;_=cancel.cancelled()=>Err(McpError::Cancelled),result=tokio::time::timeout(Duration::from_secs(15),connection.tools(cancel))=>result.map_err(|_|McpError::Transport).and_then(|result|result)};
        let tools = match tools {
            Ok(tools) => tools,
            Err(error) => {
                let _ = tokio::time::timeout(Duration::from_secs(5), connection.close()).await;
                return Err(error);
            }
        };
        let mut names = HashSet::new();
        if tools.len() > 1024
            || tools.iter().any(|tool| {
                tool.name.is_empty()
                    || tool.name.len() > 256
                    || tool.description.len() > 65536
                    || !tool.input_schema.is_object()
                    || !names.insert(&tool.name)
            })
            || serde_json::to_vec(&tools)
                .map_err(|_| McpError::Invalid)?
                .len()
                > 1048576
        {
            let _ = tokio::time::timeout(Duration::from_secs(5), connection.close()).await;
            return Err(McpError::Limit);
        }
        if let Err(error) = self.disconnect(id).await {
            let _ = tokio::time::timeout(Duration::from_secs(5), connection.close()).await;
            return Err(error);
        }
        self.entries.insert(
            id.into(),
            Entry {
                connection,
                tools,
                generation: uuid::Uuid::new_v4().to_string(),
            },
        );
        self.failures.remove(id);
        Ok(())
    }
    pub fn review(&self, id: &str, name: &str, arguments: Value) -> Result<ReviewedCall, McpError> {
        if !arguments.is_object() {
            return Err(McpError::Invalid);
        }
        let connector = self
            .definitions
            .connectors
            .iter()
            .find(|connector| connector.id() == id)
            .ok_or(McpError::Invalid)?;
        let entry = self.entries.get(id).ok_or(McpError::Unavailable)?;
        let tool = entry
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .ok_or(McpError::Invalid)?;
        Ok(ReviewedCall {
            connector_id: id.into(),
            tool: name.into(),
            risk: crate::tool_policy::classify(&tool.name, &tool.description),
            tool_identity: fingerprint(&(1, &self.identity, &entry.generation, connector, tool))?,
            fingerprint: fingerprint(&(connector, tool, &arguments))?,
            arguments,
            generation: entry.generation.clone(),
        })
    }
    pub fn approve(&self, reviewed: ReviewedCall) -> Result<Approval, McpError> {
        let current = self.review(
            &reviewed.connector_id,
            &reviewed.tool,
            reviewed.arguments.clone(),
        )?;
        if current.fingerprint != reviewed.fingerprint || current.generation != reviewed.generation
        {
            return Err(McpError::Approval);
        }
        Ok(Approval {
            reviewed,
            manager: self.identity.clone(),
        })
    }
    pub fn approve_session(
        &self,
        reviewed: ReviewedCall,
        grants: &mut crate::tool_policy::SessionGrants,
    ) -> Result<Approval, McpError> {
        let approval = self.approve(reviewed)?;
        if !grants.grant(approval.reviewed.tool_identity.clone()) {
            return Err(McpError::Approval);
        }
        Ok(approval)
    }
    pub fn session_approval(
        &self,
        reviewed: ReviewedCall,
        grants: &crate::tool_policy::SessionGrants,
    ) -> Result<Approval, McpError> {
        let approval = self.approve(reviewed)?;
        if !grants.allows(&approval.reviewed.tool_identity) {
            return Err(McpError::Approval);
        }
        Ok(approval)
    }
    pub async fn call(
        &mut self,
        id: &str,
        name: &str,
        arguments: Value,
        approval: Option<Approval>,
        cancel: &CancellationToken,
    ) -> Result<ToolResult, McpError> {
        let approval = approval.ok_or(McpError::Approval)?;
        let current = self.review(id, name, arguments.clone())?;
        if approval.manager != self.identity
            || approval.reviewed.fingerprint != current.fingerprint
            || approval.reviewed.generation != current.generation
        {
            return Err(McpError::Approval);
        }
        let entry = self.entries.get_mut(id).ok_or(McpError::Unavailable)?;
        let result = tokio::select! {biased;_=cancel.cancelled()=>Err(McpError::Cancelled),result=tokio::time::timeout(Duration::from_secs(60),entry.connection.call(name,arguments,cancel))=>result.map_err(|_|McpError::Transport).and_then(|result|result)};
        let result = result.and_then(|result| {
            if result.content.len() > 1048576 {
                Err(McpError::Limit)
            } else {
                Ok(result)
            }
        });
        if result.is_err() {
            let _ = self.disconnect(id).await;
        }
        result
    }
    pub async fn disconnect(&mut self, id: &str) -> Result<(), McpError> {
        self.failures.remove(id);
        if let Some(mut entry) = self.entries.remove(id) {
            tokio::time::timeout(Duration::from_secs(5), entry.connection.close())
                .await
                .map_err(|_| McpError::Transport)??;
        }
        Ok(())
    }
    pub async fn replace(
        &mut self,
        file: ConnectorFile,
        store: &ConnectorStore,
    ) -> Result<(), McpError> {
        file.validate()?;
        store.save(&file)?;
        let removed: Vec<_> = self
            .definitions
            .connectors
            .iter()
            .filter(|old| !file.connectors.contains(old))
            .map(|old| old.id().to_owned())
            .collect();
        self.definitions = file;
        let mut failed = false;
        for id in removed {
            failed |= self.disconnect(&id).await.is_err();
        }
        if failed {
            Err(McpError::Transport)
        } else {
            Ok(())
        }
    }
    pub async fn shutdown(&mut self) -> Result<(), McpError> {
        let ids: Vec<_> = self.entries.keys().cloned().collect();
        let mut failure = false;
        for id in ids {
            failure |= self.disconnect(&id).await.is_err();
        }
        if failure {
            Err(McpError::Transport)
        } else {
            Ok(())
        }
    }
}
