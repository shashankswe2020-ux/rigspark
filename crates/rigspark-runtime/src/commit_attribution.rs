//! Marks every git commit made by a coding agent that RigSpark launches.
//!
//! Each launched agent gets git's `core.hooksPath` pointed at a RigSpark hooks folder,
//! set through `GIT_CONFIG_*` for that process only, so no repository or user config
//! is edited. Every hook in the folder first runs the repository's own hook of the same
//! name (honouring the user's `core.hooksPath`), and `commit-msg` then adds:
//!
//! ```text
//! Co-Authored-By: rigspark-community-<model> <<salt id>+rigspark[bot]@users.noreply.github.com>
//! Signed-off-by: Rigspark <noreply@rigspark.ai>
//! ```
//!
//! The salt id is a number derived from a random salt kept in the RigSpark home, so
//! commits from one install share an id without revealing anything about the machine.
//! `git commit --no-verify` skips hooks, and so skips these trailers.

use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
};

pub const SIGNOFF: &str = "Signed-off-by: Rigspark <noreply@rigspark.ai>";
const SALT_FILE: &str = "commit-salt";
const HOOKS_DIR: &str = "git-hooks";

/// Client-side hooks git may run during commits, merges, rebases, checkouts and pushes.
/// Each one delegates to the repository's own hook, so pointing `core.hooksPath` here
/// never disables a hook the repository relies on.
const HOOKS: [&str; 18] = [
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "post-rewrite",
    "reference-transaction",
    "push-to-checkout",
    "pre-auto-gc",
    "post-index-change",
    "sendemail-validate",
];

const HOOK: &str = r#"#!/bin/sh
# Installed by RigSpark for coding agents it launches. Runs the repository's own hook,
# then, for commit-msg, adds RigSpark's commit trailers.
hook=$(basename "$0")
own=$(cd "$(dirname "$0")" && pwd -P)
original=$(GIT_CONFIG_COUNT="${RIGSPARK_GIT_CONFIG_COUNT:-0}" git rev-parse --git-path hooks 2>/dev/null)
original=$(cd "$original" 2>/dev/null && pwd -P)
if [ -n "$original" ] && [ "$original" != "$own" ] && [ -f "$original/$hook" ] && [ -x "$original/$hook" ]; then
  "$original/$hook" "$@" || exit $?
fi
if [ "$hook" = commit-msg ] && [ -n "$RIGSPARK_COMMIT_COAUTHOR" ]; then
  git interpret-trailers --in-place --if-exists addIfDifferent \
    --trailer "$RIGSPARK_COMMIT_COAUTHOR" \
    --trailer "${RIGSPARK_COMMIT_SIGNOFF:-Signed-off-by: Rigspark <noreply@rigspark.ai>}" \
    "$1" || exit $?
fi
exit 0
"#;

/// The hooks folder and salt id of one RigSpark home, ready to attribute launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub hooks: PathBuf,
    pub salt_id: String,
}

/// Creates (or repairs) the salt and hooks folder under `home`.
pub fn install(home: &Path) -> io::Result<Installed> {
    std::fs::create_dir_all(home)?;
    let salt_path = home.join(SALT_FILE);
    let salt = match std::fs::read_to_string(&salt_path) {
        Ok(salt) if valid_salt(salt.trim()) => salt.trim().to_owned(),
        _ => {
            let salt = uuid::Uuid::new_v4().simple().to_string();
            write_atomic(&salt_path, salt.as_bytes(), 0o600)?;
            salt
        }
    };
    let hooks = home.join(HOOKS_DIR);
    std::fs::create_dir_all(&hooks)?;
    for name in HOOKS {
        let path = hooks.join(name);
        if std::fs::read(&path).ok().as_deref() != Some(HOOK.as_bytes()) || !executable(&path) {
            write_atomic(&path, HOOK.as_bytes(), 0o755)?;
        }
    }
    Ok(Installed {
        hooks,
        salt_id: salt_id(&salt),
    })
}

fn valid_salt(salt: &str) -> bool {
    salt.len() == 32 && salt.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A ten-digit number from the salt; stable for one install.
pub fn salt_id(salt: &str) -> String {
    let digest = Sha256::digest(format!("rigspark-commit-attribution:{salt}"));
    let number = u64::from_be_bytes(digest[..8].try_into().unwrap_or_default());
    format!("{:010}", number % 10_000_000_000)
}

/// `rigspark-community-<model>`, with characters git trailers or emails mishandle
/// replaced by `-`.
pub fn author_name(model: &str) -> String {
    let model: String = model
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let model = model.trim_matches('-');
    format!(
        "rigspark-community-{}",
        if model.is_empty() { "local" } else { model }
    )
}

pub fn coauthor(model: &str, salt_id: &str) -> String {
    format!(
        "Co-Authored-By: {} <{salt_id}+rigspark[bot]@users.noreply.github.com>",
        author_name(model)
    )
}

impl Installed {
    /// Environment for one agent process. `inherited` is the launching process's
    /// environment, so an existing `GIT_CONFIG_COUNT` is extended rather than replaced.
    pub fn env(&self, model: &str, inherited_count: Option<&str>) -> Vec<(String, String)> {
        let base: usize = inherited_count
            .and_then(|count| count.trim().parse().ok())
            .unwrap_or(0);
        vec![
            ("GIT_CONFIG_COUNT".into(), (base + 1).to_string()),
            (format!("GIT_CONFIG_KEY_{base}"), "core.hooksPath".into()),
            (
                format!("GIT_CONFIG_VALUE_{base}"),
                self.hooks.display().to_string(),
            ),
            ("RIGSPARK_GIT_CONFIG_COUNT".into(), base.to_string()),
            (
                "RIGSPARK_COMMIT_COAUTHOR".into(),
                coauthor(model, &self.salt_id),
            ),
            ("RIGSPARK_COMMIT_SIGNOFF".into(), SIGNOFF.into()),
        ]
    }
    /// The same two lines, as Claude Code's own commit attribution, so it writes them
    /// instead of its default co-author line.
    pub fn claude_settings(&self, model: &str) -> String {
        serde_json::json!({"attribution": {
            "commit": format!("{}\n{SIGNOFF}", coauthor(model, &self.salt_id)),
        }})
        .to_string()
    }
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| io::Error::other("no parent"))?;
    let temporary = parent.join(format!(".rigspark-{}.tmp", uuid::Uuid::new_v4().simple()));
    std::fs::write(&temporary, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}
