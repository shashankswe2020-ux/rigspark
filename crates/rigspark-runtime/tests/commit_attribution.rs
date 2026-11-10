use rigspark_runtime::commit_attribution::{
    RIGSPARK_BOT_ID, SIGNOFF, author_name, coauthor, install,
};
use rigspark_runtime::connect::{Agent, ConnectPlan, attributed, plan};
use std::{path::Path, process::Command};

#[test]
fn trailer_text_matches_the_required_format() {
    // GitHub links a co-author only through a real account's noreply address:
    // 353924278 is rigspark[bot], the RigSpark GitHub App.
    assert_eq!(RIGSPARK_BOT_ID, "353924278");
    assert_eq!(
        coauthor("qwen3:8b"),
        "Co-Authored-By: rigspark-community-qwen3:8b <353924278+rigspark[bot]@users.noreply.github.com>"
    );
    assert_eq!(SIGNOFF, "Signed-off-by: Rigspark <noreply@rigspark.ai>");
    // Characters that would break a trailer or an email are replaced.
    assert_eq!(
        author_name("hf.co/org/Model <x>:Q4_K_M"),
        "rigspark-community-hf.co-org-Model--x-:Q4_K_M"
    );
    assert_eq!(author_name("  "), "rigspark-community-local");
}

#[test]
fn install_is_stable_and_repairs_hooks() {
    let home = tempfile::tempdir().unwrap();
    let first = install(home.path()).unwrap();
    let second = install(home.path()).unwrap();
    assert_eq!(first, second);
    std::fs::write(first.hooks.join("commit-msg"), "tampered").unwrap();
    install(home.path()).unwrap();
    assert!(
        std::fs::read_to_string(first.hooks.join("commit-msg"))
            .unwrap()
            .contains("interpret-trailers")
    );
}

#[test]
fn launches_carry_attribution_and_instructions_do_not() {
    let home = tempfile::tempdir().unwrap();
    let installed = install(home.path()).unwrap();
    let endpoint = "http://127.0.0.1:11434";
    let claude = plan(Agent::ClaudeCode, endpoint, "ollama", "qwen3:8b").unwrap();
    let ConnectPlan::Launch { args, env, .. } =
        attributed(claude, Agent::ClaudeCode, "qwen3:8b", &installed, Some("2"))
    else {
        panic!("claude launches");
    };
    let settings = &args[args.iter().position(|a| a == "--settings").unwrap() + 1];
    let settings: serde_json::Value = serde_json::from_str(settings).unwrap();
    assert_eq!(
        settings["attribution"]["commit"],
        format!("{}\n{SIGNOFF}", coauthor("qwen3:8b"))
    );
    // An inherited GIT_CONFIG_COUNT is extended, not replaced.
    let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    assert_eq!(get("GIT_CONFIG_COUNT"), Some("3"));
    assert_eq!(get("GIT_CONFIG_KEY_2"), Some("core.hooksPath"));
    assert_eq!(get("RIGSPARK_GIT_CONFIG_COUNT"), Some("2"));

    let codex = plan(Agent::Codex, endpoint, "ollama", "qwen3:8b").unwrap();
    let ConnectPlan::Launch { args, env, .. } =
        attributed(codex, Agent::Codex, "qwen3:8b", &installed, None)
    else {
        panic!("codex launches");
    };
    assert!(!args.contains(&"--settings".to_string()));
    assert!(
        env.iter()
            .any(|(k, v)| k == "GIT_CONFIG_VALUE_0" && Path::new(v) == installed.hooks)
    );

    let cline = plan(Agent::Cline, endpoint, "ollama", "qwen3:8b").unwrap();
    assert_eq!(
        attributed(cline.clone(), Agent::Cline, "qwen3:8b", &installed, None),
        cline
    );
}

#[cfg(unix)]
mod with_git {
    use super::*;

    fn git(repo: &Path, env: &[(String, String)], args: &[&str]) -> std::process::Output {
        Command::new("git")
            .current_dir(repo)
            .env_remove("GIT_CONFIG_COUNT")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .envs(env.iter().cloned())
            .args(args)
            .output()
            .unwrap()
    }

    fn repo() -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().unwrap();
        if Command::new("git").arg("--version").output().is_err() {
            return None;
        }
        for args in [
            &["init", "-q"][..],
            &["config", "user.name", "Person"],
            &["config", "user.email", "person@example.com"],
            &["config", "commit.gpgsign", "false"],
        ] {
            assert!(git(dir.path(), &[], args).status.success());
        }
        Some(dir)
    }

    fn hook(dir: &Path, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn commit(repo: &Path, env: &[(String, String)], message: &str) -> std::process::Output {
        std::fs::write(repo.join("file"), uuid::Uuid::new_v4().to_string()).unwrap();
        assert!(git(repo, env, &["add", "file"]).status.success());
        git(repo, env, &["commit", "-q", "-m", message])
    }

    fn last_message(repo: &Path) -> String {
        String::from_utf8(git(repo, &[], &["log", "-1", "--format=%B"]).stdout).unwrap()
    }

    #[test]
    fn every_commit_gets_both_trailers_once() {
        let Some(repo) = repo() else { return };
        let home = tempfile::tempdir().unwrap();
        let installed = install(home.path()).unwrap();
        let env = installed.env("qwen3:8b", None);
        assert!(commit(repo.path(), &env, "Add a file").status.success());
        let message = last_message(repo.path());
        let co = coauthor("qwen3:8b");
        assert!(
            message.contains(&format!("\n\n{co}\n{SIGNOFF}\n")),
            "{message}"
        );
        // A message that already has the lines (Claude Code's attribution) is not doubled.
        let already = format!("Change\n\n{co}\n{SIGNOFF}");
        assert!(commit(repo.path(), &env, &already).status.success());
        let message = last_message(repo.path());
        assert_eq!(message.matches(&co).count(), 1, "{message}");
        assert_eq!(message.matches(SIGNOFF).count(), 1, "{message}");
        // Without RigSpark's environment, commits are untouched.
        assert!(commit(repo.path(), &[], "Plain").status.success());
        assert_eq!(last_message(repo.path()).trim(), "Plain");
    }

    #[test]
    fn the_repositorys_own_hooks_still_run_and_can_refuse() {
        let Some(repo) = repo() else { return };
        let home = tempfile::tempdir().unwrap();
        let installed = install(home.path()).unwrap();
        let env = installed.env("m", None);
        let hooks = repo.path().join(".git/hooks");
        hook(&hooks, "commit-msg", "echo 'Repo-Hook: ran' >> \"$1\"");
        assert!(commit(repo.path(), &env, "One").status.success());
        let message = last_message(repo.path());
        assert!(message.contains("Repo-Hook: ran"), "{message}");
        assert!(message.contains(SIGNOFF), "{message}");
        hook(&hooks, "pre-commit", "exit 1");
        assert!(!commit(repo.path(), &env, "Refused").status.success());
    }

    #[test]
    fn a_configured_hooks_path_and_inherited_git_config_are_respected() {
        let Some(repo) = repo() else { return };
        let custom = repo.path().join("custom-hooks");
        hook(&custom, "commit-msg", "echo 'Custom-Hook: ran' >> \"$1\"");
        assert!(
            git(
                repo.path(),
                &[],
                &["config", "core.hooksPath", "custom-hooks"]
            )
            .status
            .success()
        );
        let home = tempfile::tempdir().unwrap();
        let installed = install(home.path()).unwrap();
        // The launching process already passes one config entry through the environment.
        let mut env = vec![
            ("GIT_CONFIG_COUNT".to_string(), "1".to_string()),
            ("GIT_CONFIG_KEY_0".to_string(), "user.name".to_string()),
            ("GIT_CONFIG_VALUE_0".to_string(), "Inherited".to_string()),
        ];
        env.extend(installed.env("m", Some("1")));
        assert!(commit(repo.path(), &env, "Two").status.success());
        let message = last_message(repo.path());
        assert!(message.contains("Custom-Hook: ran"), "{message}");
        assert!(message.contains(SIGNOFF), "{message}");
        let author = git(repo.path(), &[], &["log", "-1", "--format=%an"]).stdout;
        assert_eq!(String::from_utf8(author).unwrap().trim(), "Inherited");
    }
}
