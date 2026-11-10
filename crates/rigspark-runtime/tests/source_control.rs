use rigspark_runtime::mcp::{Connection, Connector, ConnectorFile, Manager, McpError};
use rigspark_runtime::source_control::{
    Access, Credentials, GithubConnection, GithubHttp, HttpReply, RepoAccess, TokenStore,
    valid_branch, valid_repository, verify_token,
};
use rigspark_runtime::tool_policy::ToolRisk;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "ghp_0123456789abcdefghijABCDEFGHIJ012345";
const APP_TOKEN: &str = "ghu_0123456789abcdefghijABCDEFGHIJ012345";
const REFRESH: &str = "ghr_0123456789abcdefghijABCDEFGHIJ012345abcdefghijABCDEFGHIJ0123456789";

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

fn grant(repository: &str, access: Access) -> RepoAccess {
    RepoAccess {
        repository: repository.into(),
        access,
    }
}

fn connection(
    home: &std::path::Path,
    token: Option<&str>,
    folders: &[String],
    access: &[RepoAccess],
    http: Recorded,
) -> GithubConnection {
    GithubConnection::new(
        home,
        token.map(Credentials::token),
        folders,
        access,
        Box::new(http),
    )
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
    assert!(store.save(&Credentials::token("short")).is_err());
    assert!(
        store
            .save(&Credentials::token("has space in it 0123456789"))
            .is_err()
    );
    store
        .save(&Credentials::token(&format!("  {TOKEN}\n")))
        .unwrap();
    assert_eq!(store.load(), Some(Credentials::token(TOKEN)));
    // A bare token saved by an earlier version still loads.
    std::fs::write(home.path().join("github-token"), format!("{TOKEN}\n")).unwrap();
    assert_eq!(store.load(), Some(Credentials::token(TOKEN)));
    // App sign-ins keep their refresh token and expiry, and never print them.
    let app = Credentials {
        access_token: APP_TOKEN.into(),
        refresh_token: Some(REFRESH.into()),
        expires_at: Some(1_000),
    };
    store.save(&app).unwrap();
    assert_eq!(store.load(), Some(app.clone()));
    assert!(!format!("{app:?}").contains("ghu_") && !format!("{app:?}").contains("ghr_"));
    assert!(app.app_sign_in() && !Credentials::token(TOKEN).app_sign_in());
    assert!(!app.expiring(600) && app.expiring(700) && app.expiring(2_000));
    assert!(!Credentials::token(TOKEN).expiring(u64::MAX));
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
    let mut none = connection(home.path(), None, &[], &[], Recorded::default());
    assert!(none.tools(&cancel).await.unwrap().is_empty());
    let folder = home.path().display().to_string();
    // Signed in but no repository chosen: no GitHub tools at all.
    let mut unchosen = connection(
        home.path(),
        Some(TOKEN),
        std::slice::from_ref(&folder),
        &[],
        Recorded::default(),
    );
    assert_eq!(
        names(unchosen.tools(&cancel).await.unwrap()),
        ["git_commit", "git_push"]
    );
    // Read-only repositories can be read, not written.
    let mut reader = connection(
        home.path(),
        Some(TOKEN),
        &[],
        &[grant("a/read", Access::Read)],
        Recorded::default(),
    );
    let tools = reader.tools(&cancel).await.unwrap();
    assert_eq!(
        tools[0].input_schema["properties"]["repository"]["enum"],
        json!(["a/read"])
    );
    assert_eq!(names(tools), ["github_list_issues", "github_read_issue"]);
    let mut both = connection(
        home.path(),
        Some(TOKEN),
        &[folder],
        &[
            grant("a/read", Access::Read),
            grant("a/write", Access::Write),
        ],
        Recorded::default(),
    );
    let tools = both.tools(&cancel).await.unwrap();
    // Each tool offers only the repositories it may use.
    let offered = |name: &str| {
        tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap()
            .input_schema["properties"]["repository"]["enum"]
            .clone()
    };
    assert_eq!(offered("github_list_issues"), json!(["a/read", "a/write"]));
    assert_eq!(offered("github_create_pull_request"), json!(["a/write"]));
    // The existing approval rules: reads ask once per session, writes every time.
    for tool in &tools {
        let risk = rigspark_runtime::tool_policy::classify(&tool.name, &tool.description);
        let expected = if tool.name.contains("list") || tool.name.contains("read") {
            ToolRisk::ReadOnly
        } else if tool.name == "git_push" {
            ToolRisk::ProcessNetwork
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
            "git_commit",
            "git_push"
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
    let mut github = connection(
        home.path(),
        Some(TOKEN),
        &[],
        &[grant("Rigspark/Rigspark", Access::Write)],
        http.clone(),
    );
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
    let mut github = connection(
        home.path(),
        Some(TOKEN),
        &[],
        &[grant("a/b", Access::Write), grant("a/c", Access::Read)],
        http.clone(),
    );
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
    let mut offline = connection(
        home.path(),
        None,
        &[],
        &[grant("a/b", Access::Write)],
        Recorded::default(),
    );
    let result = offline
        .call("github_list_issues", json!({"repository":"a/b"}), &cancel)
        .await
        .unwrap();
    assert!(result.is_error && result.content.contains("isn't connected"));
}

#[test]
fn the_github_connector_is_validated_and_listed() {
    let file: ConnectorFile = serde_json::from_value(json!({"schemaVersion":1,"connectors":[
        {"transport":"github","id":"github","name":"GitHub","repositories":["/tmp"],
         "access":[{"repository":"a/b","access":"write"},{"repository":"a/c","access":"read"}]}
    ]}))
    .unwrap();
    file.validate().unwrap();
    let manager = Manager::new(file).unwrap();
    let view = &manager.list()[0];
    assert_eq!(
        (view.transport, view.target.as_str()),
        ("github", "api.github.com · 2 repo(s) · 1 folder(s)")
    );
    // A file from before per-repository access still loads, with no access.
    let old: Connector = serde_json::from_value(
        json!({"transport":"github","id":"github","name":"GitHub","repositories":[]}),
    )
    .unwrap();
    old.validate().unwrap();
    assert!(!serde_json::to_string(&old).unwrap().contains("access"));
    for bad in [
        json!({"transport":"github","id":"other","name":"GitHub"}),
        json!({"transport":"github","id":"github","name":"GitHub","repositories":["relative"]}),
        json!({"transport":"github","id":"github","name":"GitHub","token":"x"}),
        json!({"transport":"github","id":"github","name":"GitHub",
               "access":[{"repository":"../x","access":"read"}]}),
        json!({"transport":"github","id":"github","name":"GitHub",
               "access":[{"repository":"a/b","access":"read"},{"repository":"A/B","access":"write"}]}),
        json!({"transport":"github","id":"github","name":"GitHub",
               "access":[{"repository":"a/b","access":"admin"}]}),
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
            &[],
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

        assert!(
            message.starts_with("hi from rigspark community\n\n"),
            "{message}"
        );
        assert!(message.contains("Co-Authored-By: rigspark-community-local <353924278+rigspark[bot]@users.noreply.github.com>"), "{message}");
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

    #[tokio::test]
    async fn pushes_go_to_a_new_branch_never_the_default_one() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let origin = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        git(origin.path(), &["init", "-q", "--bare"]);
        git(repo.path(), &["init", "-q"]);
        for (key, value) in [
            ("user.name", "Person"),
            ("user.email", "person@example.com"),
            ("commit.gpgsign", "false"),
        ] {
            git(repo.path(), &["config", key, value]);
        }
        git(
            repo.path(),
            &[
                "remote",
                "add",
                "origin",
                &origin.path().display().to_string(),
            ],
        );
        std::fs::write(repo.path().join("hello.txt"), "hi\n").unwrap();
        let cancel = CancellationToken::new();
        let folder = repo.path().display().to_string();
        // With a token connected: the token header is only for https://github.com.
        let mut github = connection(
            home.path(),
            Some(TOKEN),
            std::slice::from_ref(&folder),
            &[grant("a/read", Access::Read)],
            Recorded::default(),
        );
        let commit = github
            .call(
                "git_commit",
                json!({"folder": folder, "message":"hi from rigspark community"}),
                &cancel,
            )
            .await
            .unwrap();
        assert!(!commit.is_error, "{}", commit.content);
        for branch in ["main", "master", "HEAD", "a b", "-x"] {
            let refused = github
                .call(
                    "git_push",
                    json!({"folder": folder, "branch": branch}),
                    &cancel,
                )
                .await
                .unwrap();
            assert!(refused.is_error, "{branch}");
        }
        let pushed = github
            .call(
                "git_push",
                json!({"folder": folder, "branch":"rigspark/hello"}),
                &cancel,
            )
            .await
            .unwrap();
        assert!(!pushed.is_error, "{}", pushed.content);
        assert!(!pushed.content.contains(TOKEN));
        let message = git(
            origin.path(),
            &["log", "-1", "--format=%B", "rigspark/hello"],
        );
        assert!(
            message.starts_with("hi from rigspark community"),
            "{message}"
        );
        assert!(
            message.contains("Signed-off-by: Rigspark <noreply@rigspark.ai>"),
            "{message}"
        );
        // A GitHub origin must be a repository set to read and write, over HTTPS or SSH,
        // and the check runs before git contacts anything.
        for (url, refusal) in [
            ("https://github.com/a/read.git", "read-only"),
            ("git@github.com:a/read.git", "read-only"),
            ("https://github.com/other/repo", "no access"),
            ("ssh://git@github.com/other/repo.git", "no access"),
            ("https://github.com:8443/a/read", "can't check"),
        ] {
            git(repo.path(), &["remote", "set-url", "origin", url]);
            let refused = github
                .call(
                    "git_push",
                    json!({"folder": folder, "branch":"rigspark/hello"}),
                    &cancel,
                )
                .await
                .unwrap();
            assert!(
                refused.is_error && refused.content.contains(refusal),
                "{url}: {}",
                refused.content
            );
        }
    }
}

mod access {
    use super::*;
    use rigspark_runtime::source_control::{
        GrantedRepository, github_remote, granted_repositories, settle_access, validate_access,
    };

    #[tokio::test]
    async fn github_tools_refuse_repositories_rigspark_was_not_given() {
        let home = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        let http = Recorded::default();
        let mut github = connection(
            home.path(),
            Some(TOKEN),
            &[],
            &[grant("a/read", Access::Read)],
            http.clone(),
        );
        for (name, args, refusal) in [
            (
                "github_list_issues",
                json!({"repository":"a/other"}),
                "no access to a/other",
            ),
            (
                "github_read_issue",
                json!({"repository":"b/read","number":1}),
                "no access to b/read",
            ),
            (
                "github_create_pull_request",
                json!({"repository":"a/read","head":"x","base":"main","title":"t"}),
                "a/read is read-only",
            ),
            (
                "github_create_pull_request",
                json!({"repository":"a/other","head":"x","base":"main","title":"t"}),
                "no access to a/other",
            ),
        ] {
            let result = github.call(name, args, &cancel).await.unwrap();
            assert!(
                result.is_error && result.content.contains(refusal),
                "{name}: {}",
                result.content
            );
        }
        assert!(
            http.seen.lock().unwrap().is_empty(),
            "a refused repository never reaches GitHub"
        );
    }

    #[test]
    fn access_is_listed_once_per_repository() {
        assert!(validate_access(&[grant("a/b", Access::Read)]).is_ok());
        assert!(
            validate_access(&[grant("a/b", Access::Read), grant("A/b", Access::Write)]).is_err()
        );
        assert!(validate_access(&[grant("nope", Access::Read)]).is_err());
        let many: Vec<RepoAccess> = (0..201)
            .map(|index| grant(&format!("a/r{index}"), Access::Read))
            .collect();
        assert!(validate_access(&many).is_err());
    }

    #[test]
    fn only_reachable_repositories_keep_access_and_write_needs_push() {
        let granted = [
            GrantedRepository {
                repository: "Owner/App".into(),
                private: true,
                can_write: true,
            },
            GrantedRepository {
                repository: "owner/docs".into(),
                private: false,
                can_write: false,
            },
        ];
        let (kept, refused) = settle_access(
            &[
                grant("owner/app", Access::Write),
                grant("owner/docs", Access::Read),
                grant("owner/docs", Access::Write),
                grant("owner/gone", Access::Read),
            ],
            &granted,
        );
        assert_eq!(
            kept,
            [
                grant("Owner/App", Access::Write),
                grant("owner/docs", Access::Read)
            ]
        );
        assert_eq!(refused, ["owner/docs", "owner/gone"]);
    }

    #[test]
    fn remotes_are_read_as_github_repositories() {
        for (url, expected) in [
            ("https://github.com/a/b.git", Some("a/b")),
            ("https://github.com/a/b", Some("a/b")),
            ("https://github.com/a/b/", Some("a/b")),
            ("https://user@github.com/a/b.git\n", Some("a/b")),
            ("git@github.com:a/b.git", Some("a/b")),
            ("ssh://git@github.com/a/b.git", Some("a/b")),
            ("https://gitlab.com/a/b.git", None),
            ("https://github.com.evil.example/a/b", None),
            ("http://github.com/a/b", None),
            ("https://github.com/a/b/c", None),
            ("/srv/git/b.git", None),
        ] {
            assert_eq!(github_remote(url).as_deref(), expected, "{url}");
        }
    }

    #[tokio::test]
    async fn app_sign_ins_list_only_repositories_the_app_is_installed_on() {
        let app = Credentials {
            access_token: APP_TOKEN.into(),
            refresh_token: Some(REFRESH.into()),
            expires_at: None,
        };
        let full: Vec<Value> = (0..100)
            .map(|index| {
                json!({"full_name": format!("big/r{index:03}"), "private": true,
                                "permissions": {"push": index % 2 == 0}})
            })
            .collect();
        let http = Recorded::default()
            .reply(
                "/user/installations?per_page=100",
                200,
                json!({"installations":[{"id":11},{"id":12}]}),
            )
            .reply(
                "/user/installations/11/repositories?per_page=100&page=1",
                200,
                json!({"repositories": full}),
            )
            .reply(
                "/user/installations/11/repositories?per_page=100&page=2",
                200,
                json!({"repositories":[{"full_name":"big/last","private":false,"permissions":{"push":false}}]}),
            )
            .reply(
                "/user/installations/12/repositories?per_page=100&page=1",
                200,
                json!({"repositories":[{"full_name":"Me/App","private":true,"permissions":{"push":true}},
                                       {"full_name":"bad name"}]}),
            );
        let listed = granted_repositories(&http, &app).await.unwrap();
        assert_eq!(listed.len(), 102);
        assert_eq!(
            listed.iter().find(|entry| entry.repository == "Me/App"),
            Some(&GrantedRepository {
                repository: "Me/App".into(),
                private: true,
                can_write: true
            })
        );
        assert!(
            listed
                .iter()
                .any(|entry| entry.repository == "big/last" && !entry.can_write)
        );
        assert!(
            http.seen
                .lock()
                .unwrap()
                .iter()
                .all(|(_, _, token, _)| token == APP_TOKEN)
        );
        // A personal access token lists what GitHub shows for it.
        let http = Recorded::default().reply(
            "/user/repos?per_page=100&page=1",
            200,
            json!([{"full_name":"me/one","private":false,"permissions":{"push":true}}]),
        );
        let listed = granted_repositories(&http, &Credentials::token(TOKEN))
            .await
            .unwrap();
        assert_eq!(listed[0].repository, "me/one");
        let rejected = Recorded::default().reply("/user/repos?per_page=100&page=1", 401, json!({}));
        assert!(matches!(
            granted_repositories(&rejected, &Credentials::token(TOKEN)).await,
            Err(McpError::Approval)
        ));
    }
}

mod sign_in {
    use super::*;
    use rigspark_runtime::source_control::{
        DEVICE_PAGE, GITHUB_APP_SLUG, GITHUB_CLIENT_ID, GithubSignIn, INSTALL_PAGE, Renewal,
        SignIn, client_id, fresh_credentials, poll_sign_in, renew_sign_in, start_sign_in,
    };

    type Forms = Vec<(String, Vec<(String, String)>)>;

    #[derive(Default)]
    struct Recorded {
        replies: Mutex<Vec<HttpReply>>,
        seen: Mutex<Forms>,
    }
    impl Recorded {
        fn with(replies: Vec<Value>) -> Self {
            Self {
                replies: Mutex::new(
                    replies
                        .into_iter()
                        .rev()
                        .map(|body| HttpReply { status: 200, body })
                        .collect(),
                ),
                seen: Mutex::default(),
            }
        }
    }
    #[async_trait::async_trait]
    impl GithubSignIn for Recorded {
        async fn post_form(
            &self,
            path: &str,
            form: &[(&str, &str)],
        ) -> Result<HttpReply, McpError> {
            self.seen.lock().unwrap().push((
                path.into(),
                form.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ));
            self.replies
                .lock()
                .unwrap()
                .pop()
                .ok_or(McpError::Transport)
        }
    }

    fn device(uri: &str) -> Value {
        json!({"device_code":"3584d83530557fdd1f46af8289938c8ef79f9dc5","user_code":"WDJB-MJHT",
               "verification_uri":uri,"expires_in":900,"interval":5})
    }

    #[test]
    fn the_built_in_client_is_rigspark_community() {
        // The RigSpark Community GitHub App, not an OAuth app with a repo-wide scope.
        assert_eq!(GITHUB_CLIENT_ID, "Iv23liaO72v1ZQNBqeq2");
        assert_eq!(GITHUB_APP_SLUG, "rigspark-community");
        assert_eq!(
            INSTALL_PAGE,
            "https://github.com/apps/rigspark-community/installations/new"
        );
        if std::env::var_os("RIGSPARK_GITHUB_CLIENT_ID").is_none() {
            assert_eq!(client_id(), GITHUB_CLIENT_ID);
        }
    }

    #[tokio::test]
    async fn sign_in_starts_with_a_code_for_githubs_own_device_page() {
        let http = Recorded::with(vec![device(DEVICE_PAGE)]);
        let code = start_sign_in(&http, GITHUB_CLIENT_ID).await.unwrap();
        assert_eq!(code.user_code, "WDJB-MJHT");
        assert_eq!(code.verification_uri, "https://github.com/login/device");
        assert_eq!((code.interval, code.expires_in), (5, 900));
        let first = http.seen.lock().unwrap()[0].clone();
        assert_eq!(first.0, "/login/device/code");
        // No scope: what the app may do is fixed on the app, and where, by the person.
        assert_eq!(first.1, [("client_id".into(), GITHUB_CLIENT_ID.into())]);
        // A response pointing anywhere but GitHub's device page is refused.
        for uri in [
            "https://evil.example/login/device",
            "http://github.com/login/device",
        ] {
            let http = Recorded::with(vec![device(uri)]);
            assert!(
                start_sign_in(&http, GITHUB_CLIENT_ID).await.is_err(),
                "{uri}"
            );
        }
        let mut odd = device(DEVICE_PAGE);
        odd["user_code"] = json!("<script>");
        assert!(
            start_sign_in(&Recorded::with(vec![odd]), GITHUB_CLIENT_ID)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn polling_maps_every_github_answer() {
        let http = Recorded::with(vec![
            json!({"error":"authorization_pending"}),
            json!({"error":"slow_down","interval":10}),
            json!({"error":"expired_token"}),
            json!({"error":"access_denied"}),
            json!({"access_token": APP_TOKEN, "token_type":"bearer","expires_in":28800,
                   "refresh_token": REFRESH, "refresh_token_expires_in":15897600}),
            json!({"access_token": TOKEN, "token_type":"bearer"}),
            json!({"access_token":"bad token"}),
            json!({"error":"incorrect_client_credentials"}),
        ]);
        let code = "3584d83530557fdd1f46af8289938c8ef79f9dc5";
        let poll = || poll_sign_in(&http, GITHUB_CLIENT_ID, code);
        assert_eq!(poll().await.unwrap(), SignIn::Pending { interval: 5 });
        assert_eq!(poll().await.unwrap(), SignIn::Pending { interval: 10 });
        assert_eq!(poll().await.unwrap(), SignIn::Expired);
        assert_eq!(poll().await.unwrap(), SignIn::Denied);
        let before = rigspark_runtime::source_control::now();
        let SignIn::Token(app) = poll().await.unwrap() else {
            panic!("an app sign-in");
        };
        assert_eq!(app.access_token, APP_TOKEN);
        assert_eq!(app.refresh_token.as_deref(), Some(REFRESH));
        let expires = app.expires_at.unwrap();
        assert!((before + 28_800..=before + 28_810).contains(&expires));
        assert_eq!(
            poll().await.unwrap(),
            SignIn::Token(Credentials::token(TOKEN))
        );
        assert!(poll().await.is_err());
        assert!(poll().await.is_err());
        let seen = http.seen.lock().unwrap().clone();
        assert!(
            seen.iter()
                .all(|(path, form)| path == "/login/oauth/access_token"
                    && form.contains(&(
                        "grant_type".into(),
                        "urn:ietf:params:oauth:grant-type:device_code".into()
                    ))
                    && form.contains(&("device_code".into(), code.into())))
        );
        // A malformed device code never reaches GitHub.
        let before = http.seen.lock().unwrap().len();
        assert!(
            poll_sign_in(&http, GITHUB_CLIENT_ID, "a&b=c")
                .await
                .is_err()
        );
        assert!(poll_sign_in(&http, GITHUB_CLIENT_ID, "").await.is_err());
        assert_eq!(http.seen.lock().unwrap().len(), before);
    }

    #[tokio::test]
    async fn app_sign_ins_renew_before_they_expire() {
        let renewed = json!({"access_token": "ghu_renewedrenewedrenewedrenewed01234",
                             "expires_in": 28800, "refresh_token": "ghr_nextnextnextnextnextnextnext0123"});
        let http = Recorded::with(vec![renewed.clone(), json!({"error":"bad_refresh_token"})]);
        let fresh = renew_sign_in(&http, GITHUB_CLIENT_ID, REFRESH, 1_000)
            .await
            .unwrap();
        assert_eq!(fresh.expires_at, Some(29_800));
        assert_eq!(
            fresh.refresh_token.as_deref(),
            Some("ghr_nextnextnextnextnextnextnext0123")
        );
        let form = http.seen.lock().unwrap()[0].clone();
        assert_eq!(form.0, "/login/oauth/access_token");
        // No client secret: device sign-ins renew without one.
        assert_eq!(
            form.1,
            [
                ("client_id".into(), GITHUB_CLIENT_ID.into()),
                ("grant_type".into(), "refresh_token".into()),
                ("refresh_token".into(), REFRESH.into())
            ]
        );
        assert!(matches!(
            renew_sign_in(&http, GITHUB_CLIENT_ID, REFRESH, 1_000).await,
            Err(McpError::Approval)
        ));

        // The saved sign-in is renewed only when it is about to expire, and saved again.
        let home = tempfile::tempdir().unwrap();
        let tokens = TokenStore::new(home.path());
        assert_eq!(
            fresh_credentials(&tokens, &Recorded::default(), GITHUB_CLIENT_ID, 0)
                .await
                .unwrap(),
            None
        );
        let saved = Credentials {
            access_token: APP_TOKEN.into(),
            refresh_token: Some(REFRESH.into()),
            expires_at: Some(10_000),
        };
        tokens.save(&saved).unwrap();
        let quiet = Recorded::default();
        assert_eq!(
            fresh_credentials(&tokens, &quiet, GITHUB_CLIENT_ID, 9_000)
                .await
                .unwrap(),
            Some(saved.clone())
        );
        assert!(quiet.seen.lock().unwrap().is_empty());
        let http = Recorded::with(vec![renewed.clone()]);
        let now = fresh_credentials(&tokens, &http, GITHUB_CLIENT_ID, 9_800)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(now.access_token, "ghu_renewedrenewedrenewedrenewed01234");
        assert_eq!(tokens.load(), Some(now));

        // A tool call renews an expired sign-in first and uses the new token.
        tokens.save(&saved).unwrap();
        let api = super::Recorded::default().reply(
            "/repos/a/b/issues?state=open&per_page=30",
            200,
            json!([]),
        );
        let mut github = GithubConnection::new(
            home.path(),
            Some(saved.clone()),
            &[],
            &[grant("a/b", Access::Read)],
            Box::new(api.clone()),
        )
        .with_renewal(Renewal {
            sign_in: Box::new(Recorded::with(vec![renewed])),
            client_id: GITHUB_CLIENT_ID.into(),
        });
        let cancel = CancellationToken::new();
        let listed = github
            .call("github_list_issues", json!({"repository":"a/b"}), &cancel)
            .await
            .unwrap();
        assert!(!listed.is_error, "{}", listed.content);
        assert_eq!(
            api.seen.lock().unwrap()[0].2,
            "ghu_renewedrenewedrenewedrenewed01234"
        );
        assert_eq!(
            tokens.load().unwrap().access_token,
            "ghu_renewedrenewedrenewedrenewed01234"
        );
        // Without a way to renew, an expired sign-in asks to sign in again.
        let mut stale = GithubConnection::new(
            home.path(),
            Some(saved),
            &[],
            &[grant("a/b", Access::Read)],
            Box::new(super::Recorded::default()),
        );
        let result = stale
            .call("github_list_issues", json!({"repository":"a/b"}), &cancel)
            .await
            .unwrap();
        assert!(result.is_error && result.content.contains("Sign in again"));
    }
}
