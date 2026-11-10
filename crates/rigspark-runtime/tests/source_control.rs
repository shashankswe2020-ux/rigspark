use rigspark_runtime::mcp::{Connection, Connector, ConnectorFile, Manager, McpError};
use rigspark_runtime::source_control::{
    GithubConnection, GithubHttp, HttpReply, TokenStore, valid_branch, valid_repository,
    verify_token,
};
use rigspark_runtime::tool_policy::ToolRisk;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "ghp_0123456789abcdefghijABCDEFGHIJ012345";

type Seen = Vec<(String, String, String, Option<Value>)>;

#[derive(Clone, Default)]
struct Recorded {
    replies: Arc<Mutex<Vec<(String, HttpReply)>>>,
    seen: Arc<Mutex<Seen>>,
}
impl Recorded {
    fn reply(self, path: &str, status: u16, body: Value) -> Self {
        self.replies
            .lock()
            .unwrap()
            .push((path.into(), HttpReply { status, body }));
        self
    }
}
#[async_trait::async_trait]
impl GithubHttp for Recorded {
    async fn send(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<&Value>,
    ) -> Result<HttpReply, McpError> {
        self.seen
            .lock()
            .unwrap()
            .push((method.into(), path.into(), token.into(), body.cloned()));
        let mut replies = self.replies.lock().unwrap();
        let at = replies
            .iter()
            .position(|(expected, _)| expected == path)
            .ok_or(McpError::Transport)?;
        Ok(replies.remove(at).1)
    }
}

fn connection(
    home: &std::path::Path,
    token: Option<&str>,
    folders: &[String],
    http: Recorded,
) -> GithubConnection {
    GithubConnection::new(home, token.map(String::from), folders, Box::new(http))
}

#[test]
fn names_branches_and_tokens_are_checked() {
    assert!(valid_repository("rigspark/rigspark"));
    assert!(valid_repository("a-b/c.d_e"));
    for bad in ["rigspark", "a/b/c", "../x", "a/..", "a /b", "/b", "a/"] {
        assert!(!valid_repository(bad), "{bad}");
    }
    assert!(valid_branch("feat/commit-attribution"));
    for bad in ["", "-x", "a..b", "a b", "a/", "x.", "a//b"] {
        assert!(!valid_branch(bad), "{bad}");
    }
    let home = tempfile::tempdir().unwrap();
    let store = TokenStore::new(home.path());
    assert_eq!(store.load(), None);
    assert!(store.save("short").is_err());
    assert!(store.save("has space in it 0123456789").is_err());
    store.save(&format!("  {TOKEN}\n")).unwrap();
    assert_eq!(store.load().as_deref(), Some(TOKEN));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.path().join("github-token"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }
    store.remove().unwrap();
    store.remove().unwrap();
    assert_eq!(store.load(), None);
}

#[tokio::test]
async fn a_token_is_verified_against_the_user_endpoint() {
    let ok = Recorded::default().reply("/user", 200, json!({"login":"octo"}));
    assert_eq!(verify_token(&ok, TOKEN).await.unwrap(), "octo");
    assert_eq!(ok.seen.lock().unwrap()[0].2, TOKEN);
    let bad = Recorded::default().reply("/user", 401, json!({"message":"Bad credentials"}));
    assert!(matches!(
        verify_token(&bad, TOKEN).await,
        Err(McpError::Approval)
    ));
    assert!(verify_token(&Recorded::default(), "x").await.is_err());
}

#[tokio::test]
async fn tools_follow_what_is_connected() {
    let home = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let names = |tools: Vec<rigspark_runtime::mcp::Tool>| -> Vec<String> {
        tools.into_iter().map(|tool| tool.name).collect()
    };
    let mut none = connection(home.path(), None, &[], Recorded::default());
    assert!(none.tools(&cancel).await.unwrap().is_empty());
    let folder = home.path().display().to_string();
    let mut both = connection(home.path(), Some(TOKEN), &[folder], Recorded::default());
    let tools = both.tools(&cancel).await.unwrap();
    // The existing approval rules: reads ask once per session, writes every time.
    for tool in &tools {
        let risk = rigspark_runtime::tool_policy::classify(&tool.name, &tool.description);
        let expected = if tool.name.contains("list") || tool.name.contains("read") {
            ToolRisk::ReadOnly
        } else {
            ToolRisk::WorkspaceMutation
        };
        assert_eq!(risk, expected, "{}", tool.name);
    }
    assert_eq!(
        names(tools),
        [
            "github_list_issues",
            "github_read_issue",
            "github_create_pull_request",
            "git_commit"
        ]
    );
}

#[tokio::test]
async fn issues_and_pull_requests_go_to_the_github_api() {
    let home = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let http = Recorded::default()
        .reply(
            "/repos/rigspark/rigspark/issues?state=open&per_page=30",
            200,
            json!([{"number":7,"title":"Fit check\u{1b}[31m","state":"open","user":{"login":"a"}},
                   {"number":8,"title":"Docs","state":"open","user":{"login":"b"},"pull_request":{}}]),
        )
        .reply(
            "/repos/rigspark/rigspark/issues/7",
            200,
            json!({"title":"Fit check","state":"open","user":{"login":"a"},"html_url":"https://github.com/rigspark/rigspark/issues/7","body":"line one\nline two"}),
        )
        .reply(
            "/repos/rigspark/rigspark/issues/7/comments?per_page=30",
            200,
            json!([{"user":{"login":"c"},"body":"+1"}]),
        )
        .reply(
            "/repos/rigspark/rigspark/pulls",
            201,
            json!({"number":12,"html_url":"https://github.com/rigspark/rigspark/pull/12"}),
        );
    let mut github = connection(home.path(), Some(TOKEN), &[], http.clone());
    let listed = github
        .call(
            "github_list_issues",
            json!({"repository":"rigspark/rigspark"}),
            &cancel,
        )
        .await
        .unwrap();
    assert!(!listed.is_error);
    assert_eq!(
        listed.content,
        "#7 Fit check (open, by a)\n#8 Docs [pull request] (open, by b)"
    );
    let read = github
        .call(
            "github_read_issue",
            json!({"repository":"rigspark/rigspark","number":7}),
            &cancel,
        )
        .await
        .unwrap();
    assert!(
        read.content.contains("line one\nline two"),
        "{}",
        read.content
    );
    assert!(read.content.ends_with("--- c:\n+1"), "{}", read.content);
    let opened = github
        .call(
            "github_create_pull_request",
            json!({"repository":"rigspark/rigspark","head":"feat/x","base":"main","title":" Add x ","body":"Why"}),
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(
        opened.content,
        "Opened pull request #12: https://github.com/rigspark/rigspark/pull/12"
    );
    let seen = http.seen.lock().unwrap();
    let post = seen.iter().find(|(method, ..)| method == "POST").unwrap();
    assert_eq!(
        post.3,
        Some(json!({"title":"Add x","head":"feat/x","base":"main","body":"Why"}))
    );
    assert!(seen.iter().all(|(_, _, token, _)| token == TOKEN));
}

#[tokio::test]
async fn bad_input_and_github_errors_come_back_as_tool_errors() {
    let home = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let http = Recorded::default()
        .reply(
            "/repos/a/b/issues?state=open&per_page=30",
            401,
            json!({"message":"Bad credentials"}),
        )
        .reply(
            "/repos/a/c/issues?state=open&per_page=30",
            404,
            json!({"message":"Not Found"}),
        );
    let mut github = connection(home.path(), Some(TOKEN), &[], http.clone());
    for (name, args) in [
        ("github_list_issues", json!({"repository":"../etc"})),
        (
            "github_list_issues",
            json!({"repository":"a/b","state":"merged"}),
        ),
        ("github_read_issue", json!({"repository":"a/b","number":0})),
        (
            "github_create_pull_request",
            json!({"repository":"a/b","head":"a b","base":"main","title":"t"}),
        ),
        (
            "github_create_pull_request",
            json!({"repository":"a/b","head":"x","base":"main","title":"  "}),
        ),
    ] {
        let result = github.call(name, args, &cancel).await.unwrap();
        assert!(result.is_error, "{name}");
    }
    assert!(
        http.seen.lock().unwrap().is_empty(),
        "no request for invalid input"
    );
    let rejected = github
        .call("github_list_issues", json!({"repository":"a/b"}), &cancel)
        .await
        .unwrap();
    assert!(rejected.is_error && rejected.content.contains("rejected the token"));
    let missing = github
        .call("github_list_issues", json!({"repository":"a/c"}), &cancel)
        .await
        .unwrap();
    assert!(missing.is_error && missing.content.contains("Not found"));
    assert!(github.call("unknown", json!({}), &cancel).await.is_err());
    let mut offline = connection(home.path(), None, &[], Recorded::default());
    let result = offline
        .call("github_list_issues", json!({"repository":"a/b"}), &cancel)
        .await
        .unwrap();
    assert!(result.is_error && result.content.contains("isn't connected"));
}

#[test]
fn the_github_connector_is_validated_and_listed() {
    let file: ConnectorFile = serde_json::from_value(json!({"schemaVersion":1,"connectors":[
        {"transport":"github","id":"github","name":"GitHub","repositories":["/tmp"]}
    ]}))
    .unwrap();
    file.validate().unwrap();
    let manager = Manager::new(file).unwrap();
    let view = &manager.list()[0];
    assert_eq!(
        (view.transport, view.target.as_str()),
        ("github", "api.github.com · 1 folder(s)")
    );
    for bad in [
        json!({"transport":"github","id":"other","name":"GitHub"}),
        json!({"transport":"github","id":"github","name":"GitHub","repositories":["relative"]}),
        json!({"transport":"github","id":"github","name":"GitHub","token":"x"}),
    ] {
        let parsed: Result<Connector, _> = serde_json::from_value(bad);
        assert!(parsed.map_or(true, |connector| connector.validate().is_err()));
    }
}

#[cfg(unix)]
mod commits {
    use super::*;
    use std::process::Command;

    fn git(repo: &std::path::Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    #[tokio::test]
    async fn commits_only_in_allowed_folders_and_carry_rigspark_trailers() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "-q"]);
        git(repo.path(), &["config", "user.name", "Person"]);
        git(repo.path(), &["config", "user.email", "person@example.com"]);
        git(repo.path(), &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.path().join("hello.txt"), "hi\n").unwrap();
        let cancel = CancellationToken::new();
        let folder = repo.path().display().to_string();
        let mut github = connection(
            home.path(),
            None,
            std::slice::from_ref(&folder),
            Recorded::default(),
        );
        let refused = github
            .call(
                "git_commit",
                json!({"folder": other.path(), "message":"x"}),
                &cancel,
            )
            .await
            .unwrap();
        assert!(refused.is_error && refused.content.contains("isn't allowed"));
        let empty = github
            .call(
                "git_commit",
                json!({"folder": folder, "message":"  "}),
                &cancel,
            )
            .await
            .unwrap();
        assert!(empty.is_error);
        let done = github
            .call(
                "git_commit",
                json!({"folder": folder, "message":"hi from rigspark community"}),
                &cancel,
            )
            .await
            .unwrap();
        assert!(!done.is_error, "{}", done.content);
        let message = git(repo.path(), &["log", "-1", "--format=%B"]);
        let salt = rigspark_runtime::commit_attribution::install(home.path())
            .unwrap()
            .salt_id;
        assert!(
            message.starts_with("hi from rigspark community\n\n"),
            "{message}"
        );
        assert!(message.contains(&format!(
            "Co-Authored-By: rigspark-community-local <{salt}+rigspark[bot]@users.noreply.github.com>"
        )), "{message}");
        assert!(
            message.contains("Signed-off-by: Rigspark <noreply@rigspark.ai>"),
            "{message}"
        );
        assert_eq!(
            git(repo.path(), &["log", "-1", "--format=%an"]).trim(),
            "Person"
        );
        // Nothing left to commit is reported, not hidden.
        let again = github
            .call(
                "git_commit",
                json!({"folder": folder, "message":"again"}),
                &cancel,
            )
            .await
            .unwrap();
        assert!(again.is_error, "{}", again.content);
    }
}
