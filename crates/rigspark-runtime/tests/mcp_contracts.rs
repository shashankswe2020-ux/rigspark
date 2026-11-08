use rigspark_runtime::{
    agent::{AgentChat, AgentOptions, run_turn},
    harness::{DeltaSink, HarnessError},
    mcp::{
        ClientFactory, Connection, Connector, ConnectorFile, ConnectorStore, Manager, McpError,
        Tool, ToolResult,
    },
    ollama_inference::{ChatInput, ChatMessage, ChatResult},
    tool_policy::SessionGrants,
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

fn connector(id: &str, command: &str) -> Connector {
    serde_json::from_value(json!({"transport":"stdio","id":id,"name":id,"command":command}))
        .unwrap()
}

fn file(connectors: Vec<Connector>) -> ConnectorFile {
    ConnectorFile {
        schema_version: 1,
        connectors,
    }
}

#[test]
fn documents_accept_only_loopback_http_and_strict_unique_definitions() {
    assert_eq!(ConnectorFile::parse("").unwrap(), ConnectorFile::default());
    assert_eq!(
        ConnectorFile::parse(r#"{"schemaVersion":1,"connectors":[]}"#)
            .unwrap()
            .connectors
            .len(),
        0
    );
    let stdio = ConnectorFile::parse(r#"{"schemaVersion":1,"connectors":[{"transport":"stdio","id":"fs","name":"FS","command":"mcp-fs"}]}"#).unwrap();
    assert!(matches!(&stdio.connectors[0], Connector::Stdio { args, .. } if args.is_empty()));
    for url in [
        "http://127.0.0.1:9000/mcp",
        "https://localhost/mcp",
        "http://[::1]:9000/mcp",
    ] {
        let raw = json!({"schemaVersion":1,"connectors":[{"transport":"http","id":"remote","name":"Remote","url":url}]});
        assert!(ConnectorFile::parse(&raw.to_string()).is_ok(), "{url}");
    }
    for url in [
        "http://10.0.0.1/mcp",
        "https://example.com/mcp",
        "http://user:secret@127.0.0.1/mcp",
        "ftp://127.0.0.1/mcp",
    ] {
        let raw = json!({"schemaVersion":1,"connectors":[{"transport":"http","id":"remote","name":"Remote","url":url}]});
        assert!(ConnectorFile::parse(&raw.to_string()).is_err(), "{url}");
    }
    for invalid in [
        json!({"schemaVersion":2,"connectors":[]}),
        json!({"schemaVersion":1,"connectors":[],"extra":true}),
        json!({"schemaVersion":1,"connectors":[{"transport":"stdio","id":"fs","name":"FS"}]}),
        json!({"schemaVersion":1,"connectors":[{"transport":"stdio","id":"fs","name":"FS","command":"x","bogus":1}]}),
        json!({"schemaVersion":1,"connectors":[{"transport":"stdio","id":"fs","name":"A","command":"x"},{"transport":"stdio","id":"fs","name":"B","command":"y"}]}),
        json!({"schemaVersion":1,"connectors":[{"transport":"stdio","id":"Bad Id","name":"A","command":"x"}]}),
    ] {
        assert!(
            ConnectorFile::parse(&invalid.to_string()).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn the_store_is_owner_only_atomic_and_refuses_unsafe_files() {
    let home = tempfile::tempdir().unwrap();
    let store = ConnectorStore::new(home.path());
    let path = home.path().join("connectors.json");
    assert_eq!(store.load().unwrap(), ConnectorFile::default());
    let saved = file(vec![connector("fs", "mcp-fs")]);
    store.save(&saved).unwrap();
    assert_eq!(store.load().unwrap(), saved);
    let leftovers: Vec<_> = std::fs::read_dir(home.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("connectors") && name != "connectors.json")
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(matches!(store.load(), Err(McpError::Storage)));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::write(&path, "   \n").unwrap();
    assert_eq!(store.load().unwrap(), ConnectorFile::default());
    for invalid in ["{not json", r#"{"schemaVersion":9,"connectors":[]}"#] {
        std::fs::write(&path, invalid).unwrap();
        assert!(store.load().is_err(), "{invalid}");
    }
    let mut duplicate = saved.clone();
    duplicate.connectors.push(connector("fs", "other"));
    std::fs::write(&path, "{}").unwrap();
    assert!(store.save(&duplicate).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{}");
    #[cfg(unix)]
    {
        let target = home.path().join("elsewhere.json");
        std::fs::write(&target, r#"{"schemaVersion":1,"connectors":[]}"#).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(matches!(store.load(), Err(McpError::Storage)));
    }
}

#[derive(Default)]
struct Log(Mutex<Vec<String>>);
impl Log {
    fn push(&self, entry: String) {
        self.0.lock().unwrap().push(entry);
    }
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}
struct Client {
    id: String,
    tools: Vec<&'static str>,
    log: Arc<Log>,
}
#[async_trait::async_trait]
impl Connection for Client {
    async fn tools(&mut self, _: &CancellationToken) -> Result<Vec<Tool>, McpError> {
        Ok(self
            .tools
            .iter()
            .map(|name| Tool {
                name: (*name).into(),
                description: format!("{name} tool"),
                input_schema: json!({"type":"object"}),
            })
            .collect())
    }
    async fn call(
        &mut self,
        name: &str,
        _: Value,
        _: &CancellationToken,
    ) -> Result<ToolResult, McpError> {
        self.log.push(format!("call {}:{name}", self.id));
        Ok(ToolResult {
            content: format!("{} answered", self.id),
            is_error: false,
        })
    }
    async fn close(&mut self) -> Result<(), McpError> {
        self.log.push(format!("close {}", self.id));
        Ok(())
    }
}
struct Factory(Arc<Log>);
#[async_trait::async_trait]
impl ClientFactory for Factory {
    async fn connect(
        &self,
        connector: &Connector,
        _: &CancellationToken,
    ) -> Result<Box<dyn Connection>, McpError> {
        let Connector::Stdio { command, .. } = connector else {
            return Err(McpError::Unavailable);
        };
        if command == "fails" {
            return Err(McpError::Transport);
        }
        let tools = match command.as_str() {
            "alpha" => vec!["search", "shared"],
            _ => vec!["shared", "write"],
        };
        Ok(Box::new(Client {
            id: connector.id().into(),
            tools,
            log: self.0.clone(),
        }))
    }
}

#[tokio::test]
async fn connections_discover_tools_and_track_status_through_their_lifecycle() {
    let home = tempfile::tempdir().unwrap();
    let store = ConnectorStore::new(home.path());
    store
        .save(&file(vec![
            connector("alpha", "alpha"),
            connector("broken", "fails"),
        ]))
        .unwrap();
    let mut manager = Manager::new(store.load().unwrap()).unwrap();
    let log = Arc::new(Log::default());
    let factory = Factory(log.clone());
    let cancel = CancellationToken::new();
    let status = |manager: &Manager| -> Vec<(String, String, usize)> {
        manager
            .list()
            .into_iter()
            .map(|view| (view.id, view.status.to_owned(), view.tools.len()))
            .collect()
    };
    assert_eq!(
        status(&manager),
        [
            ("alpha".into(), "disconnected".into(), 0),
            ("broken".into(), "disconnected".into(), 0)
        ]
    );
    assert!(manager.tools().is_empty());
    manager.connect("alpha", &factory, &cancel).await.unwrap();
    assert!(manager.connect("broken", &factory, &cancel).await.is_err());
    assert!(manager.connect("unknown", &factory, &cancel).await.is_err());
    assert_eq!(
        status(&manager),
        [
            ("alpha".into(), "connected".into(), 2),
            ("broken".into(), "error".into(), 0)
        ]
    );
    assert_eq!(manager.tools().len(), 2);
    manager.disconnect("alpha").await.unwrap();
    assert_eq!(log.take(), ["close alpha"]);
    assert_eq!(
        status(&manager)[0],
        ("alpha".into(), "disconnected".into(), 0)
    );
    assert!(manager.remove("unknown", &store).await.is_err());
    manager.connect("alpha", &factory, &cancel).await.unwrap();
    manager.shutdown().await.unwrap();
    assert_eq!(log.take(), ["close alpha"]);
    assert!(manager.tools().is_empty());
}

#[tokio::test]
async fn replacing_definitions_keeps_unchanged_connections_and_closes_changed_ones() {
    let home = tempfile::tempdir().unwrap();
    let store = ConnectorStore::new(home.path());
    let original = file(vec![connector("alpha", "alpha"), connector("beta", "beta")]);
    let mut manager = Manager::new(ConnectorFile::default()).unwrap();
    manager.replace(original.clone(), &store).await.unwrap();
    let log = Arc::new(Log::default());
    let factory = Factory(log.clone());
    let cancel = CancellationToken::new();
    for id in ["alpha", "beta"] {
        manager.connect(id, &factory, &cancel).await.unwrap();
    }
    let changed = file(vec![
        connector("alpha", "alpha"),
        connector("beta", "beta-v2"),
    ]);
    manager.replace(changed.clone(), &store).await.unwrap();
    assert_eq!(log.take(), ["close beta"]);
    assert_eq!(store.load().unwrap(), changed);
    let connected: Vec<_> = manager
        .list()
        .into_iter()
        .filter(|view| view.status == "connected")
        .map(|view| view.id)
        .collect();
    assert_eq!(connected, ["alpha"]);
    let remote: Connector = serde_json::from_value(
        json!({"transport":"http","id":"remote","name":"Remote","url":"http://127.0.0.1:9000/mcp"}),
    )
    .unwrap();
    let mut invalid = changed.clone();
    invalid.connectors.push(remote);
    if let Some(Connector::Http { url, .. }) = invalid.connectors.last_mut() {
        *url = "http://192.168.1.10/mcp".into();
    }
    assert!(manager.replace(invalid, &store).await.is_err());
    assert_eq!(manager.snapshot(), changed);
    assert_eq!(store.load().unwrap(), changed);
    assert!(log.take().is_empty());
}

struct ToolListing(Mutex<Vec<Vec<String>>>);
#[async_trait::async_trait]
impl AgentChat for ToolListing {
    async fn chat(
        &self,
        input: &ChatInput,
        _: &CancellationToken,
        _: &mut DeltaSink<'_>,
    ) -> Result<ChatResult, HarnessError> {
        self.0
            .lock()
            .unwrap()
            .push(input.tools.iter().map(|tool| tool.name.clone()).collect());
        Ok(ChatResult {
            content: "done".into(),
            tool_calls: vec![],
            timings: None,
        })
    }
}

#[tokio::test]
async fn agents_see_connected_tools_only_and_the_first_connector_wins_a_name_collision() {
    let home = tempfile::tempdir().unwrap();
    let store = ConnectorStore::new(home.path());
    let mut manager = Manager::new(ConnectorFile::default()).unwrap();
    manager
        .replace(
            file(vec![connector("alpha", "alpha"), connector("beta", "beta")]),
            &store,
        )
        .await
        .unwrap();
    let log = Arc::new(Log::default());
    let factory = Factory(log.clone());
    let cancel = CancellationToken::new();
    let model = ToolListing(Mutex::default());
    let mut grants = SessionGrants::default();
    let options = || AgentOptions {
        session_id: "session",
        workspace_id: None,
        model: "test",
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "hi".into(),
            tool_calls: vec![],
            tool_name: None,
        }],
        temperature: None,
        max_steps: 2,
    };
    run_turn(
        &model,
        &mut manager,
        &mut grants,
        None,
        options(),
        &cancel,
        &mut |_| Ok(()),
    )
    .await
    .unwrap();
    for id in ["alpha", "beta"] {
        manager.connect(id, &factory, &cancel).await.unwrap();
    }
    run_turn(
        &model,
        &mut manager,
        &mut grants,
        None,
        options(),
        &cancel,
        &mut |_| Ok(()),
    )
    .await
    .unwrap();
    let seen = model.0.lock().unwrap().clone();
    assert_eq!(seen[0], Vec::<String>::new());
    assert_eq!(seen[1], ["search", "shared", "write"]);
    let reviewed = manager.review("beta", "write", json!({})).unwrap();
    let approval = manager.approve(reviewed).unwrap();
    let result = manager
        .call("beta", "write", json!({}), Some(approval), &cancel)
        .await
        .unwrap();
    assert_eq!(result.content, "beta answered");
    assert!(manager.review("alpha", "write", json!({})).is_err());
    assert_eq!(log.take(), ["call beta:write"]);
}
