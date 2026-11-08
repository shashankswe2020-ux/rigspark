#[test]
fn search_matches_trimmed_query_and_existing_wire_shape() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("note.txt"), "data").unwrap();
    let mut workspace = WorkspaceService::new();
    let capability = workspace.register(root.path()).unwrap();
    let result = serde_json::to_value(
        workspace
            .search(&capability.id, " NOTE ", 10, None)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        result,
        serde_json::json!({"results":[{"name":"note.txt","path":"note.txt","size":4}],"nextCursor":null,"scanTruncated":false})
    );
}
#[test]
fn recovery_preserves_user_edits_and_requires_the_recorded_root() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let records = tempfile::tempdir().unwrap();
    for name in ["first.txt", "second.txt"] {
        std::fs::write(root.path().join(name), "before").unwrap();
    }
    let mut workspace = WorkspaceService::new();
    let capability = workspace.register(root.path()).unwrap();
    let operations:Vec<_>=["first.txt","second.txt"].map(|path|json!({"op":"update","path":path,"baseHash":workspace.read(&capability.id,path,None).unwrap().hash,"hunks":[{"start":1,"end":1,"lines":["after"]}]})).into();
    let proposal =
        serde_json::from_value(json!({"workspaceId":capability.id,"operations":operations}))
            .unwrap();
    let review = workspace.review(&proposal).unwrap();
    let result = workspace.apply_with(review, records.path(), "now", |index| {
        if index == 1 {
            std::fs::write(root.path().join("first.txt"), "user changes")?;
            return Err(rigspark_runtime::workspace::WorkspaceError(
                "injected failure".into(),
            ));
        }
        Ok(())
    });
    assert!(result.is_err());
    let mut restarted = WorkspaceService::new();
    let wrong = restarted.register(other.path()).unwrap();
    assert!(
        restarted
            .recover(&wrong.id, records.path())
            .unwrap()
            .is_empty()
    );
    let authorized = restarted.register(root.path()).unwrap();
    let recovered = restarted.recover(&authorized.id, records.path()).unwrap();
    assert_eq!(recovered[0].skipped, vec!["first.txt"]);
    assert_eq!(
        std::fs::read_to_string(root.path().join("first.txt")).unwrap(),
        "user changes"
    );
    assert!(std::fs::read_dir(records.path()).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".pending.json")
    }));
}
#[test]
fn git_context_uses_injected_runner_and_validates_mode() {
    use rigspark_runtime::workspace_git::{GitOutput, GitRunner};
    struct FakeGit;
    impl GitRunner for FakeGit {
        fn output(
            &self,
            _: &std::path::Path,
            mode: &str,
        ) -> Result<GitOutput, rigspark_runtime::workspace::WorkspaceError> {
            assert_eq!(mode, "status");
            Ok(GitOutput {
                success: true,
                bytes: b" M file.txt\n".to_vec(),
            })
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut workspace = WorkspaceService::new();
    let capability = workspace.register(root.path()).unwrap();
    let snapshot = workspace
        .git_context_with(&capability.id, "status", &FakeGit)
        .unwrap();
    assert_eq!(snapshot["content"], " M file.txt\n");
    assert_eq!(snapshot["available"], true);
    assert!(
        workspace
            .git_context_with(&capability.id, "--exec", &FakeGit)
            .is_err()
    );
}
#[test]
fn review_serializes_diff_contract_without_internal_write_bytes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("file.txt"), "before\ncontext").unwrap();
    let mut workspace = WorkspaceService::new();
    let capability = workspace.register(root.path()).unwrap();
    let base = workspace.read(&capability.id, "file.txt", None).unwrap();
    let proposal: EditProposal = serde_json::from_value(json!({"workspaceId":capability.id,"operations":[{"op":"update","path":"file.txt","baseHash":base.hash,"hunks":[{"start":1,"end":1,"lines":["after"]}]}]})).unwrap();
    let review = serde_json::to_value(workspace.review(&proposal).unwrap()).unwrap();
    let file = &review["files"][0];
    assert_eq!(file["added"], 1);
    assert_eq!(file["removed"], 1);
    assert_eq!(file["baseHash"], base.hash);
    assert_eq!(
        file["hunks"][0],
        json!({"header":"@@ -1,1 +1,1 @@","lines":[{"type":"del","text":"before"},{"type":"add","text":"after"},{"type":"context","text":"context"}]})
    );
    assert!(file.get("original").is_none());
    assert!(file.get("newText").is_none());
}
use rigspark_runtime::workspace::{EditProposal, WorkspaceService};
use serde_json::json;
#[test]
fn pending_transaction_recovers_after_restart_without_overwriting_drift() {
    let root = tempfile::tempdir().unwrap();
    let records = tempfile::tempdir().unwrap();
    for name in ["first.txt", "second.txt"] {
        std::fs::write(root.path().join(name), "before").unwrap();
    }
    let mut workspace = WorkspaceService::new();
    let capability = workspace.register(root.path()).unwrap();
    let operations: Vec<_> = ["first.txt", "second.txt"].map(|path| json!({"op":"update","path":path,"baseHash":workspace.read(&capability.id,path,None).unwrap().hash,"hunks":[{"start":1,"end":1,"lines":["after"]}]})).into();
    let proposal =
        serde_json::from_value(json!({"workspaceId":capability.id,"operations":operations}))
            .unwrap();
    let review = workspace.review(&proposal).unwrap();
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        workspace.apply_with(review, records.path(), "now", |index| {
            if index == 1 {
                panic!("simulated interruption");
            }
            Ok(())
        })
    }));
    assert!(interrupted.is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("first.txt")).unwrap(),
        "after"
    );
    let mut restarted = WorkspaceService::new();
    let authorized = restarted.register(root.path()).unwrap();
    let recovery = restarted.recover(&authorized.id, records.path()).unwrap();
    assert_eq!(recovery.len(), 1);
    assert!(recovery[0].skipped.is_empty());
    assert_eq!(
        std::fs::read_to_string(root.path().join("first.txt")).unwrap(),
        "before"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("second.txt")).unwrap(),
        "before"
    );
}
#[test]
fn workspace_reads_require_capability_and_reject_secrets_traversal_and_symlinks() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("file.txt"), "hello\nworld\n").unwrap();
    std::fs::write(root.path().join(".env"), "secret").unwrap();
    let mut workspace = WorkspaceService::new();
    let capability = workspace.register(root.path()).unwrap();
    assert_eq!(
        workspace
            .read(&capability.id, "file.txt", None)
            .unwrap()
            .content,
        "hello\nworld\n"
    );
    for path in [
        "../file.txt",
        ".env",
        "/file.txt",
        ".git/config",
        "nested//file",
    ] {
        assert!(workspace.read(&capability.id, path, None).is_err());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.path().join("file.txt"), root.path().join("linked"))
            .unwrap();
        assert!(workspace.read(&capability.id, "linked", None).is_err());
    }
    workspace.revoke(&capability.id);
    assert!(workspace.read(&capability.id, "file.txt", None).is_err());
}
#[test]
fn reviewed_edits_reject_drift_and_revert_never_overwrites_user_changes() {
    let root = tempfile::tempdir().unwrap();
    let records = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("file.txt"), "before\n").unwrap();
    let mut workspace = WorkspaceService::new();
    let capability = workspace.register(root.path()).unwrap();
    let snapshot = workspace.read(&capability.id, "file.txt", None).unwrap();
    let proposal:EditProposal=serde_json::from_value(json!({"workspaceId":capability.id,"operations":[{"op":"update","path":"file.txt","baseHash":snapshot.hash,"hunks":[{"start":1,"end":1,"lines":["after"]}]}]})).unwrap();
    let review = workspace.review(&proposal).unwrap();
    std::fs::write(root.path().join("file.txt"), "changed\n").unwrap();
    assert!(workspace.apply(review, records.path(), "now").is_err());
    assert_eq!(
        std::fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "changed\n"
    );
    std::fs::write(root.path().join("file.txt"), "before\n").unwrap();
    let review = workspace.review(&proposal).unwrap();
    let result = workspace.apply(review, records.path(), "now").unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "after\n"
    );
    std::fs::write(root.path().join("file.txt"), "user change\n").unwrap();
    let revert = workspace
        .revert(&result.application_id, records.path())
        .unwrap();
    assert_eq!(revert.skipped, vec!["file.txt"]);
    assert_eq!(
        std::fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "user change\n"
    );
}

mod boundaries {
    use rigspark_runtime::workspace::{EditProposal, EditReview, WorkspaceService, relative_path};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::path::Path;

    fn sha(text: &str) -> String {
        format!("{:x}", Sha256::digest(text.as_bytes()))
    }
    fn open(root: &Path) -> (WorkspaceService, String) {
        let mut workspace = WorkspaceService::new();
        let id = workspace.register(root).unwrap().id;
        (workspace, id)
    }
    fn review(
        workspace: &WorkspaceService,
        id: &str,
        operations: Value,
    ) -> Result<EditReview, String> {
        let proposal: EditProposal =
            serde_json::from_value(json!({"workspaceId": id, "operations": operations})).unwrap();
        workspace.review(&proposal).map_err(|error| error.0)
    }
    fn update(path: &str, base: &str, hunks: Value) -> Value {
        json!({"op": "update", "path": path, "baseHash": sha(base), "hunks": hunks})
    }

    #[test]
    fn relative_paths_are_bounded_plain_segments() {
        assert_eq!(relative_path("").unwrap(), "");
        assert_eq!(relative_path(".").unwrap(), "");
        assert!(relative_path(&"a".repeat(1024)).is_ok());
        assert!(relative_path(&"a".repeat(1025)).is_err());
        assert!(relative_path(&vec!["a"; 40].join("/")).is_ok());
        assert!(relative_path(&vec!["a"; 41].join("/")).is_err());
        for bad in [
            "a//b", "a/./b", "a/../b", "a/b.", "a/ b", "a/b ", "/a", "a\\b", "a:b", "a\0b",
        ] {
            assert!(relative_path(bad).is_err(), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_replaced_or_symlinked_root_is_refused() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        let (workspace, id) = open(&root);
        assert!(workspace.read(&id, "a.txt", None).is_ok());
        // Same path, different directory.
        std::fs::rename(&root, parent.path().join("moved")).unwrap();
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        assert!(
            workspace
                .read(&id, "a.txt", None)
                .unwrap_err()
                .0
                .contains("root changed")
        );
        // Same path, now a symlink to the original directory.
        std::fs::remove_dir_all(&root).unwrap();
        std::os::unix::fs::symlink(parent.path().join("moved"), &root).unwrap();
        assert!(
            workspace
                .read(&id, "a.txt", None)
                .unwrap_err()
                .0
                .contains("root changed")
        );
    }

    #[cfg(unix)]
    #[test]
    fn paths_never_pass_through_symlinks_or_files_and_only_the_leaf_may_be_missing() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("dir")).unwrap();
        std::fs::write(root.path().join("dir/a.txt"), "a").unwrap();
        std::fs::write(root.path().join("file.txt"), "f").unwrap();
        std::os::unix::fs::symlink(root.path().join("dir"), root.path().join("link")).unwrap();
        let (workspace, id) = open(root.path());
        assert!(workspace.read(&id, "dir/a.txt", None).is_ok());
        assert!(workspace.read(&id, "link/a.txt", None).is_err());
        assert!(workspace.read(&id, "file.txt/a.txt", None).is_err());
        let create = |path: &str| {
            review(
                &workspace,
                &id,
                json!([{"op": "create", "path": path, "text": "x"}]),
            )
        };
        assert!(create("dir/new.txt").is_ok());
        assert!(create("missing/new.txt").is_err());
        assert!(create("link/new.txt").is_err());
    }

    #[test]
    fn reads_take_up_to_256_kib_of_text_and_valid_line_ranges() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("full.txt"), "a".repeat(256 * 1024)).unwrap();
        std::fs::write(root.path().join("over.txt"), "a".repeat(256 * 1024 + 1)).unwrap();
        std::fs::write(root.path().join("nul.txt"), "a\0b").unwrap();
        std::fs::write(root.path().join("lines.txt"), "one\ntwo\nthree").unwrap();
        std::fs::write(root.path().join("image.png"), "text").unwrap();
        let (workspace, id) = open(root.path());
        assert_eq!(
            workspace.read(&id, "full.txt", None).unwrap().size,
            256 * 1024
        );
        assert!(workspace.read(&id, "over.txt", None).is_err());
        assert!(workspace.read(&id, "nul.txt", None).is_err());
        assert!(workspace.read(&id, "image.png", None).is_err());
        assert!(workspace.read(&id, "", None).is_err());
        let range = |start: usize, end: usize| {
            workspace
                .read(
                    &id,
                    "lines.txt",
                    Some(
                        serde_json::from_value(json!({"startLine": start, "endLine": end}))
                            .unwrap(),
                    ),
                )
                .map(|snapshot| snapshot.content)
        };
        assert_eq!(range(2, 2).unwrap(), "two");
        assert_eq!(range(2, 99).unwrap(), "two\nthree");
        assert!(range(0, 1).is_err());
        assert!(range(3, 2).is_err());
    }

    #[test]
    fn git_context_reports_on_the_real_repository_state() {
        let root = tempfile::tempdir().unwrap();
        let (workspace, id) = open(root.path());
        let context = workspace.git_context(&id, "status").unwrap();
        assert_eq!(context["kind"], "git");
        assert_eq!(context["mode"], "status");
        assert_eq!(context["available"], false);
    }

    #[test]
    fn git_output_is_kept_whole_up_to_102400_characters() {
        use rigspark_runtime::workspace_git::{GitOutput, GitRunner};
        struct Long(usize);
        impl GitRunner for Long {
            fn output(
                &self,
                _: &Path,
                _: &str,
            ) -> Result<GitOutput, rigspark_runtime::workspace::WorkspaceError> {
                Ok(GitOutput {
                    success: true,
                    bytes: "x".repeat(self.0).into_bytes(),
                })
            }
        }
        let root = tempfile::tempdir().unwrap();
        let (workspace, id) = open(root.path());
        for (length, truncated) in [(102_400, false), (102_401, true)] {
            let context = workspace
                .git_context_with(&id, "diff", &Long(length))
                .unwrap();
            assert_eq!(context["truncated"], truncated, "{length}");
            assert_eq!(context["size"], 102_400.min(length));
        }
    }

    #[test]
    fn search_pages_and_stops_at_500_matches() {
        let root = tempfile::tempdir().unwrap();
        for index in 0..3 {
            std::fs::write(root.path().join(format!("note-{index}.txt")), "x").unwrap();
        }
        let (workspace, id) = open(root.path());
        let page = workspace.search(&id, "note", 3, None).unwrap();
        assert_eq!((page.results.len(), page.next_cursor), (3, None));
        let page = workspace.search(&id, "note", 2, None).unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("2"));
        assert!(!page.scan_truncated);
        let many = tempfile::tempdir().unwrap();
        for index in 0..501 {
            std::fs::write(many.path().join(format!("hit-{index:03}.txt")), "x").unwrap();
        }
        let (workspace, id) = open(many.path());
        let page = workspace.search(&id, "hit", 100, None).unwrap();
        assert!(page.scan_truncated);
        let few = tempfile::tempdir().unwrap();
        for index in 0..499 {
            std::fs::write(few.path().join(format!("hit-{index:03}.txt")), "x").unwrap();
        }
        let (workspace, id) = open(few.path());
        assert!(
            !workspace
                .search(&id, "hit", 100, None)
                .unwrap()
                .scan_truncated
        );
    }

    #[test]
    fn search_stops_scanning_after_20000_entries() {
        let root = tempfile::tempdir().unwrap();
        for directory in 0..11 {
            let path = root.path().join(format!("d{directory}"));
            std::fs::create_dir(&path).unwrap();
            for file in 0..1900 {
                std::fs::write(path.join(format!("f{file}")), "").unwrap();
            }
        }
        let (workspace, id) = open(root.path());
        assert!(
            workspace
                .search(&id, "nothing-matches", 10, None)
                .unwrap()
                .scan_truncated
        );
    }

    #[test]
    fn reviews_bound_operations_hunks_and_content() {
        let root = tempfile::tempdir().unwrap();
        let lines: Vec<String> = (1..=10).map(|n| format!("line{n}")).collect();
        let base = lines.join("\n");
        std::fs::write(root.path().join("file.txt"), &base).unwrap();
        std::fs::write(root.path().join(".env"), "x").unwrap();
        let (workspace, id) = open(root.path());
        let one = |hunk: Value| {
            review(
                &workspace,
                &id,
                json!([update("file.txt", &base, json!([hunk]))]),
            )
        };
        // Context: up to three lines before and after a hunk.
        let reviewed =
            serde_json::to_value(one(json!({"start": 5, "end": 5, "lines": ["five"]})).unwrap())
                .unwrap();
        let types: Vec<_> = reviewed["files"][0]["hunks"][0]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line["type"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            types,
            [
                "context", "context", "context", "del", "add", "context", "context", "context"
            ]
        );
        // Appending after the last line and inserting without removing are both valid.
        assert!(one(json!({"start": 11, "end": 10, "lines": ["eleven"]})).is_ok());
        assert!(one(json!({"start": 4, "end": 3, "lines": ["inserted"]})).is_ok());
        for bad in [
            json!({"start": 0, "end": 0, "lines": []}),
            json!({"start": 12, "end": 11, "lines": []}),
            json!({"start": 5, "end": 3, "lines": []}),
            json!({"start": 5, "end": 11, "lines": []}),
            json!({"start": 1, "end": 1, "lines": ["a\nb"]}),
            json!({"start": 1, "end": 1, "lines": ["a\rb"]}),
            json!({"start": 1, "end": 1, "lines": ["a\0b"]}),
        ] {
            assert!(one(bad.clone()).is_err(), "{bad}");
        }
        assert!(one(json!({"start": 1, "end": 1, "lines": vec!["x"; 2000]})).is_ok());
        assert!(one(json!({"start": 1, "end": 1, "lines": vec!["x"; 2001]})).is_err());
        let hunks = |count: usize| {
            json!(
                (0..count)
                    .map(|n| json!({"start": n % 10 + 1, "end": n % 10 + 1, "lines": ["x"]}))
                    .collect::<Vec<_>>()
            )
        };
        assert!(
            review(
                &workspace,
                &id,
                json!([update("file.txt", &base, json!([]))])
            )
            .is_err()
        );
        assert!(
            review(
                &workspace,
                &id,
                json!([update("file.txt", &base, hunks(101))])
            )
            .is_err()
        );
        // Operation count, unsafe paths and content size.
        let create = |path: &str, text: &str| json!({"op": "create", "path": path, "text": text});
        let creates = |count: usize| {
            json!(
                (0..count)
                    .map(|n| create(&format!("new{n}.txt"), "x"))
                    .collect::<Vec<_>>()
            )
        };
        assert!(review(&workspace, &id, creates(20)).is_ok());
        assert!(review(&workspace, &id, creates(21)).is_err());
        assert!(review(&workspace, &id, json!([])).is_err());
        assert!(review(&workspace, &id, json!([create(".env.local", "x")])).is_err());
        assert!(review(&workspace, &id, json!([create("blob.png", "x")])).is_err());
        assert!(
            review(
                &workspace,
                &id,
                json!([create("a.txt", "x"), create("A.TXT", "y")])
            )
            .is_err()
        );
        assert!(
            review(
                &workspace,
                &id,
                json!([create("big.txt", &"x".repeat(512 * 1024))])
            )
            .is_ok()
        );
        assert!(
            review(
                &workspace,
                &id,
                json!([create("big.txt", &"x".repeat(512 * 1024 + 1))])
            )
            .is_err()
        );
        assert!(review(&workspace, &id, json!([create("nul.txt", "a\0b")])).is_err());
        assert!(review(&workspace, &id, json!([create("exists.txt", "x")])).is_ok());
        assert!(review(&workspace, &id, json!([create("file.txt", "x")])).is_err());
    }

    #[test]
    fn only_large_updates_carry_a_warning() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file.txt"), "one").unwrap();
        let (workspace, id) = open(root.path());
        let warnings = |added: usize| {
            let reviewed = review(
                &workspace,
                &id,
                json!([update(
                    "file.txt",
                    "one",
                    json!([{"start": 1, "end": 1, "lines": vec!["x"; added]}])
                )]),
            )
            .unwrap();
            let value = serde_json::to_value(reviewed).unwrap();
            (
                value["files"][0]["warnings"].as_array().unwrap().len(),
                value["warnings"].as_array().unwrap().len(),
            )
        };
        // One line removed plus the added lines.
        assert_eq!(warnings(1999), (0, 0));
        assert_eq!(warnings(2000), (1, 1));
        let created = review(
            &workspace,
            &id,
            json!([{"op": "create", "path": "new.txt", "text": vec!["x"; 2500].join("\n")}]),
        )
        .unwrap();
        let value = serde_json::to_value(created).unwrap();
        assert!(value["files"][0]["warnings"].as_array().unwrap().is_empty());
        assert!(value["warnings"].as_array().unwrap().is_empty());
    }

    /// Applies an update to `name` that fails on its second file, leaving a pending journal.
    fn interrupted(root: &Path, records: &Path, original: &str) -> (WorkspaceService, String) {
        std::fs::write(root.join("a.txt"), original).unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();
        let (workspace, id) = open(root);
        let reviewed = review(
            &workspace,
            &id,
            json!([
                update(
                    "a.txt",
                    original,
                    json!([{"start": 1, "end": 1, "lines": ["changed"]}])
                ),
                update(
                    "b.txt",
                    "b",
                    json!([{"start": 1, "end": 1, "lines": ["changed"]}])
                ),
            ]),
        )
        .unwrap();
        let result = workspace.apply_with(reviewed, records, "now", |index| {
            if index == 1 {
                return Err(rigspark_runtime::workspace::WorkspaceError("stop".into()));
            }
            Ok(())
        });
        assert!(result.is_err());
        (workspace, id)
    }

    #[test]
    fn edits_of_files_larger_than_a_kib_apply_and_revert() {
        let root = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir().unwrap();
        let original = "x".repeat(4000);
        std::fs::write(root.path().join("big.txt"), &original).unwrap();
        let (workspace, id) = open(root.path());
        let reviewed = review(
            &workspace,
            &id,
            json!([update(
                "big.txt",
                &original,
                json!([{"start": 1, "end": 1, "lines": ["y".repeat(4000)]}])
            )]),
        )
        .unwrap();
        let applied = workspace.apply(reviewed, records.path(), "now").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("big.txt")).unwrap(),
            "y".repeat(4000)
        );
        let reverted = workspace
            .revert(&applied.application_id, records.path())
            .unwrap();
        assert_eq!(reverted.reverted, ["big.txt"]);
        assert_eq!(
            std::fs::read_to_string(root.path().join("big.txt")).unwrap(),
            original
        );
        // A failure mid-apply rolls back a large first file too.
        let (_, _) = interrupted(root.path(), records.path(), &"z".repeat(4000));
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "z".repeat(4000)
        );
    }

    fn pending(records: &Path) -> std::path::PathBuf {
        std::fs::read_dir(records)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.to_string_lossy().ends_with(".pending.json"))
            .expect("a pending journal")
    }

    /// A pending journal for an update of a.txt, as left by a crash after the first write.
    fn crashed(root: &Path, records: &Path, original: &str) -> (std::path::PathBuf, Value) {
        std::fs::write(root.join("a.txt"), original).unwrap();
        std::fs::write(root.join("b.txt"), "b").unwrap();
        let (workspace, id) = open(root);
        let reviewed = review(
            &workspace,
            &id,
            json!([
                update(
                    "a.txt",
                    original,
                    json!([{"start": 1, "end": 1, "lines": ["changed"]}])
                ),
                update(
                    "b.txt",
                    "b",
                    json!([{"start": 1, "end": 1, "lines": ["changed"]}])
                ),
            ]),
        )
        .unwrap();
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            workspace.apply_with(reviewed, records, "now", |index| {
                if index == 1 {
                    panic!("simulated crash");
                }
                Ok(())
            })
        }));
        assert!(crash.is_err());
        let path = pending(records);
        let record: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        (path, record)
    }

    fn recover(
        root: &Path,
        records: &Path,
    ) -> Result<Vec<rigspark_runtime::workspace::RevertResult>, String> {
        let (workspace, id) = open(root);
        workspace.recover(&id, records).map_err(|error| error.0)
    }

    #[test]
    fn recovery_restores_large_originals_and_rejects_invalid_journals() {
        // A real crash with a 4 KB original recovers it.
        let root = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir().unwrap();
        crashed(root.path(), records.path(), &"o".repeat(4000));
        let recovered = recover(root.path(), records.path()).unwrap();
        assert_eq!(recovered[0].reverted, ["a.txt"]);
        assert_eq!(
            std::fs::read_to_string(root.path().join("a.txt")).unwrap(),
            "o".repeat(4000)
        );

        let broken: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
            ("schema", Box::new(|r| r["schemaVersion"] = json!(2))),
            (
                "id",
                Box::new(|r| r["applicationId"] = json!(uuid::Uuid::new_v4().to_string())),
            ),
            ("empty", Box::new(|r| r["files"] = json!([]))),
            (
                "too many",
                Box::new(|r| {
                    let file = r["files"][0].clone();
                    r["files"] = json!(vec![file; 21]);
                }),
            ),
            ("op", Box::new(|r| r["files"][0]["op"] = json!("delete"))),
            (
                "no original",
                Box::new(|r| {
                    r["files"][0].as_object_mut().unwrap().remove("original");
                }),
            ),
            (
                "wrong original",
                Box::new(|r| r["files"][0]["original"] = json!("tampered")),
            ),
            (
                "oversized original",
                Box::new(|r| {
                    let big = "o".repeat(512 * 1024 + 1);
                    r["files"][0]["beforeHash"] = json!(sha(&big));
                    r["files"][0]["original"] = json!(big);
                }),
            ),
        ];
        for (label, corrupt) in broken {
            let root = tempfile::tempdir().unwrap();
            let records = tempfile::tempdir().unwrap();
            let (path, mut record) = crashed(root.path(), records.path(), "o");
            corrupt(&mut record);
            std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
            assert!(recover(root.path(), records.path()).is_err(), "{label}");
        }
        // Twenty files and a 512 KiB original are within the limits.
        let root = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir().unwrap();
        let (path, mut record) = crashed(root.path(), records.path(), "o");
        let first = record["files"][0].clone();
        let mut files = vec![first.clone(); 19];
        let big = "o".repeat(512 * 1024);
        let mut sized = first;
        sized["beforeHash"] = json!(sha(&big));
        sized["original"] = json!(big);
        files.push(sized);
        record["files"] = json!(files);
        std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(recover(root.path(), records.path()).is_ok());
    }

    #[test]
    fn recovery_ignores_a_created_file_that_is_already_gone() {
        let root = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir().unwrap();
        let (workspace, id) = open(root.path());
        std::fs::write(root.path().join("b.txt"), "b").unwrap();
        let reviewed = review(
            &workspace,
            &id,
            json!([
                {"op": "create", "path": "new.txt", "text": "made"},
                update("b.txt", "b", json!([{"start": 1, "end": 1, "lines": ["changed"]}])),
            ]),
        )
        .unwrap();
        let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            workspace.apply_with(reviewed, records.path(), "now", |index| {
                if index == 1 {
                    panic!("simulated crash");
                }
                Ok(())
            })
        }));
        assert!(crash.is_err());
        std::fs::remove_file(root.path().join("new.txt")).unwrap();
        let recovered = recover(root.path(), records.path()).unwrap();
        assert!(
            recovered[0].reverted.is_empty() && recovered[0].skipped.is_empty(),
            "{recovered:?}"
        );
    }

    #[test]
    fn revert_records_must_match_their_id_schema_and_size() {
        let root = tempfile::tempdir().unwrap();
        let records = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.txt"), "a").unwrap();
        let (workspace, id) = open(root.path());
        let applied = workspace
            .apply(
                review(
                    &workspace,
                    &id,
                    json!([update(
                        "a.txt",
                        "a",
                        json!([{"start": 1, "end": 1, "lines": ["b"]}])
                    )]),
                )
                .unwrap(),
                records.path(),
                "now",
            )
            .unwrap();
        let path = records
            .path()
            .join(format!("{}.json", applied.application_id));
        let original: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for (label, change) in [
            ("schema", json!({"schemaVersion": 2})),
            (
                "id",
                json!({"applicationId": uuid::Uuid::new_v4().to_string()}),
            ),
            (
                "files",
                json!({"files": vec![original["files"][0].clone(); 21]}),
            ),
        ] {
            let mut record = original.clone();
            for (key, value) in change.as_object().unwrap() {
                record[key] = value.clone();
            }
            std::fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
            assert!(
                workspace
                    .revert(&applied.application_id, records.path())
                    .is_err(),
                "{label}"
            );
        }
        std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
        assert_eq!(
            workspace
                .revert(&applied.application_id, records.path())
                .unwrap()
                .reverted,
            ["a.txt"]
        );
    }
}
