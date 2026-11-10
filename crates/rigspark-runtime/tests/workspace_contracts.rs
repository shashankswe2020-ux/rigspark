use rigspark_runtime::{
    workspace::{EditProposal, WorkspaceError, WorkspaceService, denied, relative_path},
    workspace_git::{GitOutput, GitRunner},
};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn fixture() -> (tempfile::TempDir, WorkspaceService, String) {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("src/index.ts"),
        "line one\nline two\nline three\n",
    )
    .unwrap();
    fs::write(root.path().join("src/app.ts"), "one\ntwo\nthree\nfour\n").unwrap();
    fs::write(root.path().join("README.md"), "# hello\n").unwrap();
    fs::create_dir(root.path().join("node_modules")).unwrap();
    fs::write(root.path().join("node_modules/junk.js"), "noise").unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    fs::write(root.path().join(".env"), "SECRET=1\n").unwrap();
    let mut workspace = WorkspaceService::new();
    let id = workspace.register(root.path()).unwrap().id;
    (root, workspace, id)
}

fn proposal(id: &str, operations: Value) -> EditProposal {
    serde_json::from_value(json!({"workspaceId": id, "operations": operations})).unwrap()
}

fn review_error(workspace: &WorkspaceService, id: &str, operations: Value) -> String {
    workspace
        .review(&proposal(id, operations))
        .expect_err("review must fail")
        .0
}

fn hash(workspace: &WorkspaceService, id: &str, path: &str) -> String {
    workspace.read(id, path, None).unwrap().hash
}

#[test]
fn relative_paths_normalize_the_root_and_reject_escapes() {
    assert_eq!(relative_path("").unwrap(), "");
    assert_eq!(relative_path(".").unwrap(), "");
    assert_eq!(
        relative_path("src/gui/server.ts").unwrap(),
        "src/gui/server.ts"
    );
    let deep = vec!["x"; 60].join("/");
    let long = "a".repeat(2000);
    for bad in [
        "../etc/passwd",
        "src/../../secret",
        "/etc/passwd",
        "C:\\Windows",
        "src\\gui",
        "a\0b",
        "..",
        deep.as_str(),
        long.as_str(),
    ] {
        assert!(relative_path(bad).is_err(), "{bad}");
    }
}

#[test]
fn relative_path_limits_are_exact_and_each_part_rule_stands_alone() {
    // Length: 1024 bytes pass, 1025 fail.
    assert!(relative_path(&"a".repeat(1024)).is_ok());
    assert!(relative_path(&"a".repeat(1025)).is_err());
    // Depth: 40 segments pass, 41 fail.
    assert!(relative_path(&vec!["x"; 40].join("/")).is_ok());
    assert!(relative_path(&vec!["x"; 41].join("/")).is_err());
    // Each segment rule rejects on its own.
    for bad in [
        "src//main.rs",
        "./main.rs",
        "src/trailing.",
        "src/ leading",
        "src/trailing ",
        "drive:file",
    ] {
        assert!(relative_path(bad).is_err(), "{bad}");
    }
    for good in ["src/.hidden", "a.b/c.d", "with space/inside"] {
        assert_eq!(relative_path(good).unwrap(), good);
    }
}

#[test]
fn secret_paths_are_denied_and_ordinary_sources_are_allowed() {
    for path in [
        ".git/config",
        ".ssh/id_rsa",
        ".aws/credentials",
        ".env",
        "app/.env.local",
        "id_rsa",
        "server.pem",
        "cert.key",
        ".npmrc",
        ".netrc",
    ] {
        assert!(denied(path), "{path}");
    }
    for path in ["src/index.ts", "README.md", "package.json", "docs/plan.md"] {
        assert!(!denied(path), "{path}");
    }
}

#[test]
fn registration_requires_an_existing_directory_and_returns_an_opaque_id() {
    let (root, mut workspace, id) = fixture();
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    for bad in [root.path().join("missing"), root.path().join("README.md")] {
        assert!(workspace.register(&bad).is_err(), "{}", bad.display());
    }
    assert!(workspace.register(Path::new("")).is_err());
    let created = root.path().join("calculator-workspace");
    let made = workspace.create_root(&created).unwrap();
    assert_eq!(made.name, "calculator-workspace");
    assert!(workspace.tree(&made.id, "").unwrap().is_empty());
}

#[test]
fn tree_lists_one_level_with_directories_first_and_hides_noise_and_secrets() {
    let (_root, workspace, id) = fixture();
    for noisy in ["dist", "build", "coverage", "__pycache__"] {
        fs::create_dir(_root.path().join(noisy)).unwrap();
    }
    let entries = workspace.tree(&id, "").unwrap();
    let names: Vec<_> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(names, ["src", "README.md"]);
    assert_eq!(entries[0].kind, "dir");
    assert!(workspace.tree("no-such-id", "").is_err());
}

#[test]
#[cfg_attr(not(unix), allow(unused_variables))]
fn reads_are_hashed_ranged_and_fail_closed() {
    let (root, mut workspace, id) = fixture();
    let snapshot = workspace.read(&id, "src/index.ts", None).unwrap();
    assert_eq!(snapshot.content, "line one\nline two\nline three\n");
    assert!(
        snapshot.hash.len() == 64 && snapshot.hash.bytes().all(|byte| byte.is_ascii_hexdigit())
    );
    assert_eq!(snapshot.hash, hash(&workspace, &id, "src/index.ts"));
    assert!(snapshot.size > 0);
    let range = serde_json::from_value(json!({"startLine": 2, "endLine": 3})).unwrap();
    let ranged =
        serde_json::to_value(workspace.read(&id, "src/index.ts", Some(range)).unwrap()).unwrap();
    assert_eq!(ranged["content"], "line two\nline three");
    assert_eq!(ranged["range"], json!({"startLine": 2, "endLine": 3}));
    for path in ["../secret", ".env", ".git/config", "src/missing.ts"] {
        assert!(workspace.read(&id, path, None).is_err(), "{path}");
    }
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("target.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("target.txt"),
            root.path().join("link.txt"),
        )
        .unwrap();
        assert!(workspace.read(&id, "link.txt", None).is_err());
    }
    workspace.revoke(&id);
    assert!(workspace.read(&id, "README.md", None).is_err());
}

#[test]
fn search_excludes_hidden_files_sorts_and_pages_with_a_cursor() {
    let (root, workspace, id) = fixture();
    fs::write(root.path().join("src/logo.png"), "binarydata").unwrap();
    fs::create_dir(root.path().join("pkg")).unwrap();
    for index in 0..5 {
        fs::write(root.path().join(format!("pkg/mod{index}.ts")), "x").unwrap();
    }
    let paths = |query: &str| {
        workspace
            .search(&id, query, 100, None)
            .unwrap()
            .results
            .into_iter()
            .map(|result| result.path)
            .collect::<Vec<_>>()
    };
    assert_eq!(paths("index"), ["src/index.ts"]);
    let all = paths("");
    for present in ["src/index.ts", "README.md"] {
        assert!(all.contains(&present.to_owned()), "{present}");
    }
    for hidden in ["node_modules/junk.js", ".env", "src/logo.png"] {
        assert!(!all.contains(&hidden.to_owned()), "{hidden}");
    }
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(sorted, all);
    let first = workspace.search(&id, "mod", 2, None).unwrap();
    assert_eq!(
        (first.results.len(), first.next_cursor.as_deref()),
        (2, Some("2"))
    );
    let second = workspace
        .search(&id, "mod", 2, first.next_cursor.as_deref())
        .unwrap();
    assert_eq!(second.results.len(), 2);
    assert_ne!(second.results[0].path, first.results[0].path);
    assert!(workspace.search("no-such-id", "x", 10, None).is_err());
}

struct Git(Result<(bool, String), ()>);
impl GitRunner for Git {
    fn output(&self, _: &Path, _: &str) -> Result<GitOutput, WorkspaceError> {
        match &self.0 {
            Ok((success, text)) => Ok(GitOutput {
                success: *success,
                bytes: text.clone().into_bytes(),
            }),
            Err(()) => Err(WorkspaceError("git unavailable".into())),
        }
    }
}

#[test]
fn git_context_reports_availability_honestly_and_bounds_output() {
    let (_root, workspace, id) = fixture();
    let status = workspace
        .git_context_with(
            &id,
            "status",
            &Git(Ok((true, " M src/index.ts\n?? new.ts\n".into()))),
        )
        .unwrap();
    assert_eq!(
        (status["available"].as_bool(), status["label"].as_str()),
        (Some(true), Some("git status"))
    );
    assert!(status["content"].as_str().unwrap().contains("src/index.ts"));
    assert_eq!(status["hash"].as_str().unwrap().len(), 64);
    // Native reports both a missing binary and a non-repository root as git-failed.
    for runner in [
        Git(Err(())),
        Git(Ok((false, "fatal: not a git repository".into()))),
    ] {
        let failed = workspace.git_context_with(&id, "diff", &runner).unwrap();
        assert_eq!(
            (failed["available"].as_bool(), failed["reason"].as_str()),
            (Some(false), Some("git-failed"))
        );
        assert_eq!(failed["content"], "");
    }
    let clean = workspace
        .git_context_with(&id, "diff", &Git(Ok((true, "\n".into()))))
        .unwrap();
    assert_eq!(
        (clean["available"].as_bool(), clean["reason"].as_str()),
        (Some(false), Some("no-changes"))
    );
    let big = format!("{}\n", "x".repeat(200 * 1024));
    let truncated = workspace
        .git_context_with(&id, "diff", &Git(Ok((true, big.clone()))))
        .unwrap();
    assert_eq!(
        (
            truncated["available"].as_bool(),
            truncated["truncated"].as_bool()
        ),
        (Some(true), Some(true))
    );
    assert!(truncated["content"].as_str().unwrap().len() < big.len());
    assert!(
        workspace
            .git_context_with("no-such-id", "status", &Git(Ok((true, "x".into()))))
            .is_err()
    );
}

#[test]
fn update_reviews_count_changes_without_mutating_the_file() {
    let (_root, workspace, id) = fixture();
    let before = hash(&workspace, &id, "src/app.ts");
    let review = workspace
        .review(&proposal(&id, json!([{"op":"update","path":"src/app.ts","baseHash":before,"hunks":[{"start":2,"end":2,"lines":["TWO","TWO.5"]}]}])))
        .unwrap();
    let file = &review.reviewed_files[0];
    assert_eq!(
        (file.op.as_str(), file.added, file.removed),
        ("update", 2, 1)
    );
    assert_eq!(file.result_hash.len(), 64);
    assert!(
        file.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == "add" && line.text == "TWO")
    );
    assert_eq!(hash(&workspace, &id, "src/app.ts"), before);
}

#[test]
fn reviews_reject_stale_overlapping_out_of_range_denied_and_duplicate_edits() {
    let (_root, workspace, id) = fixture();
    let base = hash(&workspace, &id, "src/app.ts");
    assert!(workspace
        .review(&proposal(&id, json!([{"op":"update","path":"src/app.ts","baseHash":"deadbeef","hunks":[{"start":1,"end":1,"lines":["x"]}]}])))
        .is_err());
    let overlapping = review_error(
        &workspace,
        &id,
        json!([{"op":"update","path":"src/app.ts","baseHash":base,"hunks":[{"start":1,"end":2,"lines":["a"]},{"start":2,"end":3,"lines":["b"]}]}]),
    );
    assert_eq!(overlapping, "overlapping or invalid hunk");
    let range = review_error(
        &workspace,
        &id,
        json!([{"op":"update","path":"src/app.ts","baseHash":base,"hunks":[{"start":99,"end":99,"lines":["z"]}]}]),
    );
    assert_eq!(range, "overlapping or invalid hunk");
    assert!(workspace
        .review(&proposal(&id, json!([{"op":"update","path":".env","baseHash":"x","hunks":[{"start":1,"end":1,"lines":["a"]}]}])))
        .is_err());
    let duplicate = review_error(
        &workspace,
        &id,
        json!([
            {"op":"update","path":"src/app.ts","baseHash":base,"hunks":[{"start":1,"end":1,"lines":["a"]}]},
            {"op":"update","path":"src/app.ts","baseHash":base,"hunks":[{"start":2,"end":2,"lines":["b"]}]}
        ]),
    );
    assert_eq!(duplicate, "unsafe or duplicate edit path");
    assert!(workspace.review(&proposal(&id, json!([]))).is_err());
    assert!(serde_json::from_value::<EditProposal>(json!({"operations":[]})).is_err());
    assert!(workspace
        .review(&proposal(&id, json!([{"op":"update","path":"src/app.ts","baseHash":base,"hunks":[{"start":2,"end":0,"lines":["x"]}]}])))
        .is_err());
}

#[test]
#[cfg_attr(not(unix), allow(unused_variables))]
fn create_and_delete_reviews_have_honest_counts_and_never_clobber() {
    let (root, workspace, id) = fixture();
    let created = workspace
        .review(&proposal(
            &id,
            json!([{"op":"create","path":"src/new.ts","text":"hello\nworld\n"}]),
        ))
        .unwrap();
    let file = &created.reviewed_files[0];
    assert_eq!(
        (file.op.as_str(), file.added, file.removed),
        ("create", 3, 0)
    );
    let clobber = review_error(
        &workspace,
        &id,
        json!([{"op":"create","path":"src/app.ts","text":"x"}]),
    );
    assert_eq!(clobber, "create target exists");
    let deleted = workspace
        .review(&proposal(&id, json!([{"op":"delete","path":"src/app.ts","baseHash":hash(&workspace, &id, "src/app.ts")}])))
        .unwrap();
    let file = &deleted.reviewed_files[0];
    assert_eq!(
        (file.op.as_str(), file.removed, file.result_hash.as_str()),
        ("delete", 5, "")
    );
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("target.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("target.txt"),
            root.path().join("link.txt"),
        )
        .unwrap();
        assert!(workspace
            .review(&proposal(&id, json!([{"op":"update","path":"link.txt","baseHash":"x","hunks":[{"start":1,"end":1,"lines":["a"]}]}])))
            .is_err());
    }
}

#[test]
fn applies_write_reviewed_bytes_record_hashes_and_revert_safely() {
    let (root, workspace, id) = fixture();
    let records = tempfile::tempdir().unwrap();
    let app = root.path().join("src/app.ts");
    let update = json!([{"op":"update","path":"src/app.ts","baseHash":hash(&workspace, &id, "src/app.ts"),"hunks":[{"start":2,"end":2,"lines":["TWO"]}]}]);
    let review = workspace.review(&proposal(&id, update.clone())).unwrap();
    let applied = workspace.apply(review, records.path(), "now").unwrap();
    assert_eq!(fs::read_to_string(&app).unwrap(), "one\nTWO\nthree\nfour\n");
    assert_eq!(applied.files, ["src/app.ts"]);
    let record: Value = serde_json::from_str(
        &fs::read_to_string(
            records
                .path()
                .join(format!("{}.json", applied.application_id)),
        )
        .unwrap(),
    )
    .unwrap();
    for field in ["beforeHash", "afterHash"] {
        assert_eq!(
            record["files"][0][field].as_str().unwrap().len(),
            64,
            "{field}"
        );
    }
    let reverted = workspace
        .revert(&applied.application_id, records.path())
        .unwrap();
    assert_eq!(reverted.reverted, ["src/app.ts"]);
    assert_eq!(fs::read_to_string(&app).unwrap(), "one\ntwo\nthree\nfour\n");

    let create = workspace
        .review(&proposal(
            &id,
            json!([{"op":"create","path":"src/new.ts","text":"hi\n"}]),
        ))
        .unwrap();
    let created = workspace.apply(create, records.path(), "now").unwrap();
    assert_eq!(
        fs::read_to_string(root.path().join("src/new.ts")).unwrap(),
        "hi\n"
    );
    workspace
        .revert(&created.application_id, records.path())
        .unwrap();
    assert!(!root.path().join("src/new.ts").exists());
    assert!(workspace.revert("does-not-exist", records.path()).is_err());

    let delete = workspace
        .review(&proposal(&id, json!([{"op":"delete","path":"src/app.ts","baseHash":hash(&workspace, &id, "src/app.ts")}])))
        .unwrap();
    let refused = workspace
        .apply(delete, records.path(), "now")
        .unwrap_err()
        .0;
    assert!(refused.contains("delete apply is not enabled"), "{refused}");
    assert!(app.exists());
}

#[test]
fn a_mid_transaction_failure_restores_every_replaced_file() {
    let (root, workspace, id) = fixture();
    let records = tempfile::tempdir().unwrap();
    fs::write(root.path().join("src/b.ts"), "b1\nb2\n").unwrap();
    let review = workspace
        .review(&proposal(&id, json!([
            {"op":"update","path":"src/app.ts","baseHash":hash(&workspace, &id, "src/app.ts"),"hunks":[{"start":1,"end":1,"lines":["X"]}]},
            {"op":"update","path":"src/b.ts","baseHash":hash(&workspace, &id, "src/b.ts"),"hunks":[{"start":1,"end":1,"lines":["Y"]}]}
        ])))
        .unwrap();
    let calls = std::cell::RefCell::new(Vec::new());
    let failed = workspace.apply_with(review, records.path(), "now", |index| {
        calls.borrow_mut().push(index);
        if index == 1 {
            return Err(WorkspaceError("injected mid-transaction failure".into()));
        }
        Ok(())
    });
    assert!(failed.unwrap_err().0.contains("injected"));
    assert_eq!(*calls.borrow(), [0, 1]);
    assert_eq!(
        fs::read_to_string(root.path().join("src/app.ts")).unwrap(),
        "one\ntwo\nthree\nfour\n"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("src/b.ts")).unwrap(),
        "b1\nb2\n"
    );
}
