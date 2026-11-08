use crate::memory::MemoryError;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("invalid session data")]
    Invalid,
    #[error("session was modified by another writer")]
    Conflict,
    #[error("session storage failed")]
    Storage,
}
impl From<MemoryError> for SessionError {
    fn from(_: MemoryError) -> Self {
        Self::Storage
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub hash: String,
    pub size: u64,
    pub truncated: bool,
    pub included: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<crate::workspace::LineRange>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<Attachment>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDoc {
    pub schema_version: u8,
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
    pub archived: bool,
    pub messages: Vec<StoredMessage>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    pub revision: u64,
    pub archived: bool,
    pub message_count: usize,
}
fn chars(text: &str) -> usize {
    text.encode_utf16().count()
}
pub(crate) fn cap(text: &str, maximum: usize) -> String {
    let mut count = 0;
    text.chars()
        .take_while(|character| {
            count += character.len_utf16();
            count <= maximum
        })
        .collect()
}
pub fn gui_text(text: &str) -> String {
    GuiTextStream::default().push(text)
}
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Escape {
    #[default]
    None,
    Started,
    Csi,
    Command {
        osc: bool,
        terminating: bool,
    },
}
/// Incremental form of [`gui_text`] that keeps escape and CRLF state across chunks.
#[derive(Default)]
pub struct GuiTextStream {
    escape: Escape,
    after_cr: bool,
}
impl GuiTextStream {
    pub fn push(&mut self, text: &str) -> String {
        let mut output = String::with_capacity(text.len());
        for character in text.chars() {
            self.feed(character, &mut output);
        }
        output
    }
    fn feed(&mut self, character: char, output: &mut String) {
        let after_cr = std::mem::take(&mut self.after_cr);
        match self.escape {
            Escape::Started => {
                self.escape = match character {
                    '[' => Escape::Csi,
                    ']' => Escape::Command {
                        osc: true,
                        terminating: false,
                    },
                    'P' | 'X' | '^' | '_' => Escape::Command {
                        osc: false,
                        terminating: false,
                    },
                    '@'..='Z' | '\\'..='_' => Escape::None,
                    _ => {
                        self.escape = Escape::None;
                        return self.feed(character, output);
                    }
                };
                return;
            }
            Escape::Csi => {
                self.escape = match character {
                    ' '..='?' => Escape::Csi,
                    '@'..='~' => Escape::None,
                    _ => {
                        self.escape = Escape::None;
                        return self.feed(character, output);
                    }
                };
                return;
            }
            Escape::Command { osc, terminating } => {
                let ended = character == '\u{9c}'
                    || (osc && character == '\u{7}')
                    || (terminating && character == '\\');
                self.escape = if ended {
                    Escape::None
                } else {
                    Escape::Command {
                        osc,
                        terminating: character == '\u{1b}',
                    }
                };
                return;
            }
            Escape::None => {}
        }
        match character {
            '\n' if after_cr => {}
            '\r' => {
                output.push('\n');
                self.after_cr = true;
            }
            '\n' | '\t' => output.push(character),
            '\u{1b}' => self.escape = Escape::Started,
            '\u{9b}' => self.escape = Escape::Csi,
            '\u{9d}' => {
                self.escape = Escape::Command {
                    osc: true,
                    terminating: false,
                }
            }
            '\u{90}' | '\u{98}' | '\u{9e}' | '\u{9f}' => {
                self.escape = Escape::Command {
                    osc: false,
                    terminating: false,
                }
            }
            _ if character.is_control() || invisible(character) => {}
            _ => output.push(character),
        }
    }
}
fn invisible(character: char) -> bool {
    matches!(
        character,
        '\u{ad}'
            | '\u{61c}'
            | '\u{200b}'..='\u{200f}'
            | '\u{2060}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{feff}'
    )
}
fn title(text: &str) -> String {
    cap(
        &rigspark_core::reports::strip_control(text)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        200,
    )
}
fn timestamp(value: &str) -> bool {
    !value.is_empty() && value.len() <= 40
}
impl SessionDoc {
    pub fn validate(&self) -> Result<(), SessionError> {
        if self.schema_version != 1
            || uuid::Uuid::parse_str(&self.id).is_err()
            || self.id.len() != 36
            || self.id != self.id.to_lowercase()
            || chars(&self.title) > 200
            || !timestamp(&self.created_at)
            || !timestamp(&self.updated_at)
            || self.revision > 9007199254740991
            || self.messages.len() > 2000
        {
            return Err(SessionError::Invalid);
        }
        for message in &self.messages {
            if !["user", "assistant", "system"].contains(&message.role.as_str())
                || chars(&message.content) > 32768
                || !timestamp(&message.at)
            {
                return Err(SessionError::Invalid);
            }
            if let Some(attachments) = &message.attachments {
                if attachments.len() > 20 {
                    return Err(SessionError::Invalid);
                }
                for attachment in attachments {
                    if attachment.kind.as_ref().is_some_and(|kind| {
                        !["file", "terminal", "diagnostics", "git"].contains(&kind.as_str())
                    }) || attachment.hash.is_empty()
                        || chars(&attachment.hash) > 128
                        || attachment.size > 9007199254740991
                        || attachment
                            .label
                            .as_ref()
                            .is_some_and(|value| value.is_empty() || chars(value) > 200)
                        || attachment
                            .path
                            .as_ref()
                            .is_some_and(|value| value.is_empty() || chars(value) > 1024)
                        || attachment
                            .range
                            .as_ref()
                            .is_some_and(|range| range.start_line == 0 || range.end_line == 0)
                    {
                        return Err(SessionError::Invalid);
                    }
                }
            }
        }
        Ok(())
    }
    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            revision: self.revision,
            archived: self.archived,
            message_count: self.messages.len(),
        }
    }
}
pub struct SessionRepository {
    home: PathBuf,
    pub runs: crate::runs::RunCoordinator,
}
impl SessionRepository {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.into(),
            runs: crate::runs::RunCoordinator::default(),
        }
    }
    fn path(&self, id: &str) -> Result<PathBuf, SessionError> {
        self.check_home()?;
        if uuid::Uuid::parse_str(id).is_err() || id.len() != 36 || id != id.to_lowercase() {
            return Err(SessionError::Invalid);
        }
        let directory = self.home.join("gui-sessions");
        if let Ok(metadata) = fs::symlink_metadata(&directory)
            && (!metadata.is_dir() || metadata.file_type().is_symlink())
        {
            return Err(SessionError::Storage);
        }
        Ok(directory.join(format!("{id}.json")))
    }
    fn lock(&self) -> Result<crate::state::LockGuard, SessionError> {
        crate::state::StateStore::new(
            crate::state::Config::from_home(&self.home).map_err(|_| SessionError::Storage)?,
        )
        .lock(Duration::from_secs(10))
        .map_err(|_| SessionError::Storage)
    }
    fn ids(&self) -> Result<Vec<String>, SessionError> {
        self.check_home()?;
        let directory = self.home.join("gui-sessions");
        if !directory.try_exists().map_err(|_| SessionError::Storage)? {
            return Ok(Vec::new());
        }
        if fs::symlink_metadata(&directory)
            .map_err(|_| SessionError::Storage)?
            .file_type()
            .is_symlink()
        {
            return Err(SessionError::Storage);
        }
        let mut ids = Vec::new();
        for (index, entry) in fs::read_dir(directory)
            .map_err(|_| SessionError::Storage)?
            .enumerate()
        {
            if index > 10000 {
                return Err(SessionError::Storage);
            }
            let entry = entry.map_err(|_| SessionError::Storage)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".json")
                && self.path(id).is_ok()
            {
                ids.push(id.into());
            }
        }
        Ok(ids)
    }
    fn check_home(&self) -> Result<(), SessionError> {
        match fs::symlink_metadata(&self.home) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(SessionError::Storage),
        }
    }
    pub fn get(&self, id: &str) -> Result<Option<SessionDoc>, SessionError> {
        let path = self.path(id)?;
        let read = (|| {
            let directory = crate::secure_fs::Directory::open(&self.home)?;
            directory.read(
                path.strip_prefix(&self.home)
                    .map_err(|_| std::io::Error::other("invalid session path"))?,
                8 * 1024 * 1024,
                true,
            )
        })();
        let raw = match read {
            Ok(raw) => String::from_utf8(raw).map_err(|_| SessionError::Invalid)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(SessionError::Storage),
        };
        let doc: SessionDoc = serde_json::from_str(&raw).map_err(|_| SessionError::Invalid)?;
        doc.validate()?;
        if doc.id != id {
            return Err(SessionError::Invalid);
        }
        Ok(Some(doc))
    }
    fn write(&self, doc: &SessionDoc) -> Result<(), SessionError> {
        doc.validate()?;
        let bytes = format!(
            "{}\n",
            serde_json::to_string_pretty(doc).map_err(|_| SessionError::Invalid)?
        );
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(SessionError::Invalid);
        }
        let directory =
            crate::secure_fs::Directory::open(&self.home).map_err(|_| SessionError::Storage)?;
        directory
            .ensure_dir(Path::new("gui-sessions"))
            .map_err(|_| SessionError::Storage)?;
        let path = self.path(&doc.id)?;
        directory
            .write(
                path.strip_prefix(&self.home)
                    .map_err(|_| SessionError::Storage)?,
                bytes.as_bytes(),
                false,
                false,
            )
            .map_err(|_| SessionError::Storage)?;
        Ok(())
    }
    pub fn create(&self, name: &str, at: &str) -> Result<SessionDoc, SessionError> {
        let guard = self.lock()?;
        if self.ids()?.len() >= 500 {
            return Err(SessionError::Invalid);
        }
        let name = title(name);
        let doc = SessionDoc {
            schema_version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            title: if name.is_empty() {
                "New chat".into()
            } else {
                name
            },
            created_at: at.into(),
            updated_at: at.into(),
            revision: 0,
            archived: false,
            messages: Vec::new(),
        };
        self.write(&doc)?;
        guard.release().map_err(|_| SessionError::Storage)?;
        Ok(doc)
    }
    fn mutate(
        &self,
        id: &str,
        expected: Option<u64>,
        at: &str,
        change: impl FnOnce(&mut SessionDoc),
    ) -> Result<SessionDoc, SessionError> {
        let guard = self.lock()?;
        let mut doc = self.get(id)?.ok_or(SessionError::Invalid)?;
        if expected.is_some_and(|revision| revision != doc.revision) {
            return Err(SessionError::Conflict);
        }
        change(&mut doc);
        doc.revision = doc.revision.checked_add(1).ok_or(SessionError::Invalid)?;
        doc.updated_at = at.into();
        self.write(&doc)?;
        guard.release().map_err(|_| SessionError::Storage)?;
        Ok(doc)
    }
    pub fn append(
        &self,
        id: &str,
        mut message: StoredMessage,
        expected: Option<u64>,
    ) -> Result<SessionDoc, SessionError> {
        message.content = cap(&gui_text(&message.content), 32768);
        if let Some(attachments) = &mut message.attachments {
            attachments.truncate(20);
        }
        let at = message.at.clone();
        self.mutate(id, expected, &at, |doc| {
            if doc.title == "New chat"
                && message.role == "user"
                && !message.content.trim().is_empty()
            {
                doc.title = title(&cap(&message.content, 60));
            }
            doc.messages.push(message);
            if doc.messages.len() > 2000 {
                doc.messages.drain(..doc.messages.len() - 2000);
            }
        })
    }
    pub fn append_exchange(
        &self,
        id: &str,
        mut user: StoredMessage,
        mut assistant: StoredMessage,
        expected: u64,
    ) -> Result<SessionDoc, SessionError> {
        if user.role != "user" || assistant.role != "assistant" {
            return Err(SessionError::Invalid);
        }
        for message in [&mut user, &mut assistant] {
            message.content = cap(&gui_text(&message.content), 32768);
            if let Some(attachments) = &mut message.attachments {
                attachments.truncate(20);
            }
        }
        let at = assistant.at.clone();
        self.mutate(id, Some(expected), &at, |doc| {
            if doc.title == "New chat" && !user.content.trim().is_empty() {
                doc.title = title(&cap(&user.content, 60));
            }
            doc.messages.extend([user, assistant]);
            if doc.messages.len() > 2000 {
                doc.messages.drain(..doc.messages.len() - 2000);
            }
        })
    }
    pub fn rename(
        &self,
        id: &str,
        name: &str,
        expected: Option<u64>,
        at: &str,
    ) -> Result<SessionDoc, SessionError> {
        let name = title(name);
        if name.is_empty() {
            return Err(SessionError::Invalid);
        }
        self.mutate(id, expected, at, |doc| doc.title = name)
    }
    pub fn archive(
        &self,
        id: &str,
        archived: bool,
        expected: Option<u64>,
        at: &str,
    ) -> Result<SessionDoc, SessionError> {
        self.mutate(id, expected, at, |doc| doc.archived = archived)
    }
    pub fn remove(&self, id: &str) -> Result<(), SessionError> {
        let guard = self.lock()?;
        let directory =
            crate::secure_fs::Directory::open(&self.home).map_err(|_| SessionError::Storage)?;
        match directory.remove(
            self.path(id)?
                .strip_prefix(&self.home)
                .map_err(|_| SessionError::Storage)?,
        ) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(SessionError::Storage),
        }
        guard.release().map_err(|_| SessionError::Storage)
    }
    pub fn list(
        &self,
        archived: bool,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<SessionSummary>, Option<String>), SessionError> {
        let mut docs: Vec<_> = self
            .ids()?
            .iter()
            .filter_map(|id| self.get(id).ok().flatten())
            .filter(|doc| archived || !doc.archived)
            .collect();
        docs.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then(right.id.cmp(&left.id))
        });
        let limit = limit.clamp(1, 200);
        let next = (offset.saturating_add(limit) < docs.len())
            .then(|| offset.saturating_add(limit).to_string());
        Ok((
            docs.into_iter()
                .skip(offset)
                .take(limit)
                .map(|doc| doc.summary())
                .collect(),
            next,
        ))
    }
    pub fn messages(
        &self,
        id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<StoredMessage>, Option<String>), SessionError> {
        let doc = self.get(id)?.ok_or(SessionError::Invalid)?;
        let limit = limit.clamp(1, 500);
        let next = (offset.saturating_add(limit) < doc.messages.len())
            .then(|| offset.saturating_add(limit).to_string());
        Ok((
            doc.messages.into_iter().skip(offset).take(limit).collect(),
            next,
        ))
    }
    pub fn search(
        &self,
        query: &str,
        archived: bool,
    ) -> Result<Vec<(SessionSummary, String)>, SessionError> {
        let query = rigspark_core::reports::strip_control(query)
            .trim()
            .to_lowercase();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let mut matches = Vec::new();
        for id in self.ids()? {
            if let Ok(Some(doc)) = self.get(&id) {
                if doc.archived && !archived {
                    continue;
                }
                if doc.title.to_lowercase().contains(&query) {
                    matches.push((doc.summary(), doc.title.clone()));
                } else if let Some(message) = doc
                    .messages
                    .iter()
                    .find(|message| message.content.to_lowercase().contains(&query))
                {
                    let lower = message.content.to_lowercase();
                    let index = lower.find(&query).unwrap_or(0);
                    let start = lower[..index].encode_utf16().count().saturating_sub(24);
                    let snippet = String::from_utf16_lossy(
                        &message
                            .content
                            .encode_utf16()
                            .skip(start)
                            .take(80)
                            .collect::<Vec<_>>(),
                    );
                    matches.push((doc.summary(), snippet));
                }
            }
        }
        matches.sort_by(|left, right| right.0.updated_at.cmp(&left.0.updated_at));
        matches.truncate(50);
        Ok(matches)
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;

    const AT: &str = "2026-11-08T00:00:00Z";

    fn doc() -> SessionDoc {
        SessionDoc {
            schema_version: 1,
            id: "0b7f2c38-6f1d-4c55-9a27-35f6c9d2a1e4".into(),
            title: "Title".into(),
            created_at: AT.into(),
            updated_at: AT.into(),
            revision: 0,
            archived: false,
            messages: vec![message("user", "hi")],
        }
    }
    fn message(role: &str, content: &str) -> StoredMessage {
        StoredMessage {
            role: role.into(),
            content: content.into(),
            at: AT.into(),
            attachments: None,
        }
    }
    fn attachment() -> Attachment {
        Attachment {
            kind: Some("file".into()),
            label: Some("label".into()),
            path: Some("src/main.rs".into()),
            hash: "abc".into(),
            size: 1,
            truncated: false,
            included: true,
            range: None,
        }
    }
    fn valid(change: impl FnOnce(&mut SessionDoc)) -> bool {
        let mut value = doc();
        change(&mut value);
        value.validate().is_ok()
    }
    fn with_attachment(change: impl FnOnce(&mut Attachment)) -> bool {
        valid(|doc| {
            let mut value = attachment();
            change(&mut value);
            doc.messages[0].attachments = Some(vec![value]);
        })
    }

    #[test]
    fn lengths_are_counted_in_utf16_units() {
        assert_eq!(chars(""), 0);
        assert_eq!(chars("ab"), 2);
        assert_eq!(chars("😀"), 2);
    }

    #[test]
    fn session_documents_are_bounded_field_by_field() {
        assert!(valid(|_| {}));
        assert!(!valid(|d| d.schema_version = 2));
        assert!(!valid(|d| d.id = "not-a-uuid".into()));
        assert!(!valid(|d| d.id = d.id.replace('-', "")));
        assert!(!valid(|d| d.id = d.id.to_uppercase()));
        assert!(valid(|d| d.title = "t".repeat(200)));
        assert!(!valid(|d| d.title = "t".repeat(201)));
        for bad in ["", &"9".repeat(41)] {
            assert!(!valid(|d| d.created_at = bad.into()), "{bad:?}");
            assert!(!valid(|d| d.updated_at = bad.into()), "{bad:?}");
            assert!(!valid(|d| d.messages[0].at = bad.into()), "{bad:?}");
        }
        assert!(valid(|d| d.created_at = "9".repeat(40)));
        assert!(valid(|d| d.revision = 9007199254740991));
        assert!(!valid(|d| d.revision = 9007199254740992));
        assert!(valid(|d| d.messages = vec![message("user", "x"); 2000]));
        assert!(!valid(|d| d.messages = vec![message("user", "x"); 2001]));
        for role in ["user", "assistant", "system"] {
            assert!(valid(|d| d.messages[0].role = role.into()), "{role}");
        }
        assert!(!valid(|d| d.messages[0].role = "tool".into()));
        assert!(valid(|d| d.messages[0].content = "c".repeat(32768)));
        assert!(!valid(|d| d.messages[0].content = "c".repeat(32769)));
    }

    #[test]
    fn attachments_are_bounded_field_by_field() {
        assert!(with_attachment(|_| {}));
        for kind in ["file", "terminal", "diagnostics", "git"] {
            assert!(with_attachment(|a| a.kind = Some(kind.into())), "{kind}");
        }
        assert!(with_attachment(|a| a.kind = None));
        assert!(!with_attachment(|a| a.kind = Some("url".into())));
        assert!(!with_attachment(|a| a.hash = String::new()));
        assert!(with_attachment(|a| a.hash = "h".repeat(128)));
        assert!(!with_attachment(|a| a.hash = "h".repeat(129)));
        assert!(with_attachment(|a| a.size = 9007199254740991));
        assert!(!with_attachment(|a| a.size = 9007199254740992));
        assert!(with_attachment(|a| a.label = Some("l".repeat(200))));
        assert!(!with_attachment(|a| a.label = Some("l".repeat(201))));
        assert!(!with_attachment(|a| a.label = Some(String::new())));
        assert!(with_attachment(|a| a.path = Some("p".repeat(1024))));
        assert!(!with_attachment(|a| a.path = Some("p".repeat(1025))));
        assert!(!with_attachment(|a| a.path = Some(String::new())));
        let range = |start, end| crate::workspace::LineRange {
            start_line: start,
            end_line: end,
        };
        assert!(with_attachment(|a| a.range = Some(range(1, 1))));
        assert!(!with_attachment(|a| a.range = Some(range(0, 1))));
        assert!(!with_attachment(|a| a.range = Some(range(1, 0))));
        assert!(valid(
            |d| d.messages[0].attachments = Some(vec![attachment(); 20])
        ));
        assert!(!valid(
            |d| d.messages[0].attachments = Some(vec![attachment(); 21])
        ));
    }

    #[test]
    fn gui_text_drops_every_escape_form_and_keeps_the_text_after() {
        // Two-character escapes end at their final byte.
        assert_eq!(gui_text("a\u{1b}Mb"), "ab");
        assert_eq!(gui_text("a\u{1b}\\b"), "ab");
        // An escape followed by an ordinary character keeps that character.
        assert_eq!(gui_text("a\u{1b}1b"), "a1b");
        // C1 CSI, OSC and string introducers.
        assert_eq!(gui_text("a\u{9b}31mb"), "ab");
        assert_eq!(gui_text("a\u{9d}title\u{7}b"), "ab");
        for introducer in ['\u{90}', '\u{98}', '\u{9e}', '\u{9f}'] {
            assert_eq!(
                gui_text(&format!("a{introducer}payload\u{9c}b")),
                "ab",
                "{introducer:?}"
            );
        }
        // BEL ends only OSC strings; other strings need ST.
        assert_eq!(gui_text("a\u{1b}Ppay\u{7}load\u{1b}\\b"), "ab");
        assert_eq!(gui_text("a\u{1b}]t\u{7}b"), "ab");
    }

    fn repository() -> (tempfile::TempDir, SessionRepository) {
        let home = tempfile::tempdir().unwrap();
        let repository = SessionRepository::new(home.path());
        (home, repository)
    }

    #[test]
    fn the_home_must_be_a_real_directory_or_not_exist_yet() {
        let parent = tempfile::tempdir().unwrap();
        let missing = SessionRepository::new(&parent.path().join("missing"));
        assert!(missing.list(false, 0, 10).unwrap().0.is_empty());
        let file = parent.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(matches!(
            SessionRepository::new(&file).list(false, 0, 10),
            Err(SessionError::Storage)
        ));
        #[cfg(unix)]
        {
            let real = parent.path().join("real");
            std::fs::create_dir(&real).unwrap();
            let link = parent.path().join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            assert!(matches!(
                SessionRepository::new(&link).list(false, 0, 10),
                Err(SessionError::Storage)
            ));
        }
        // A session directory that is a file is refused too.
        let (home, sessions) = repository();
        std::fs::write(home.path().join("gui-sessions"), "x").unwrap();
        assert!(sessions.get(&doc().id).is_err());
    }

    #[test]
    fn documents_up_to_eight_mib_round_trip_and_larger_ones_are_refused() {
        let (_home, sessions) = repository();
        let created = sessions.create("big", AT).unwrap();
        // 2000 messages of 1500 characters: about 3 MiB on disk.
        let mut doc = sessions.get(&created.id).unwrap().unwrap();
        doc.messages = vec![message("user", &"m".repeat(1500)); 2000];
        sessions.write(&doc).unwrap();
        assert_eq!(
            sessions.get(&created.id).unwrap().unwrap().messages.len(),
            2000
        );
        // 2000 messages of 4500 characters pass validation but exceed 8 MiB serialized.
        doc.messages = vec![message("user", &"m".repeat(4500)); 2000];
        assert!(doc.validate().is_ok());
        assert!(matches!(sessions.write(&doc), Err(SessionError::Invalid)));
    }

    #[test]
    fn appends_keep_the_newest_2000_messages() {
        let (_home, sessions) = repository();
        let created = sessions.create("", AT).unwrap();
        let mut doc = sessions.get(&created.id).unwrap().unwrap();
        doc.messages = (0..1999).map(|n| message("user", &n.to_string())).collect();
        sessions.write(&doc).unwrap();
        let doc = sessions
            .append(&created.id, message("assistant", "1999"), None)
            .unwrap();
        assert_eq!(doc.messages.len(), 2000);
        let doc = sessions
            .append(&created.id, message("assistant", "2000"), None)
            .unwrap();
        assert_eq!(doc.messages.len(), 2000);
        assert_eq!(doc.messages[0].content, "1");
        assert_eq!(doc.messages[1999].content, "2000");
        let doc = sessions
            .append_exchange(
                &created.id,
                message("user", "q"),
                message("assistant", "a"),
                doc.revision,
            )
            .unwrap();
        assert_eq!(doc.messages.len(), 2000);
        assert_eq!(doc.messages[0].content, "3");
        assert_eq!(doc.messages[1999].content, "a");
    }

    #[test]
    fn exchanges_are_a_user_then_an_assistant_message_and_title_a_new_chat() {
        let (_home, sessions) = repository();
        let created = sessions.create("", AT).unwrap();
        for (user, assistant) in [
            ("assistant", "assistant"),
            ("user", "user"),
            ("system", "assistant"),
        ] {
            assert!(
                sessions
                    .append_exchange(&created.id, message(user, "q"), message(assistant, "a"), 0)
                    .is_err(),
                "{user} then {assistant}"
            );
        }
        let titled = sessions
            .append_exchange(
                &created.id,
                message("user", "What is a KV cache?"),
                message("assistant", "a"),
                0,
            )
            .unwrap();
        assert_eq!(titled.title, "What is a KV cache?");
        // A titled chat keeps its title; a blank question leaves "New chat".
        let kept = sessions
            .append_exchange(
                &created.id,
                message("user", "another"),
                message("assistant", "a"),
                titled.revision,
            )
            .unwrap();
        assert_eq!(kept.title, "What is a KV cache?");
        let blank = sessions.create("", AT).unwrap();
        let untitled = sessions
            .append_exchange(
                &blank.id,
                message("user", "   "),
                message("assistant", "a"),
                0,
            )
            .unwrap();
        assert_eq!(untitled.title, "New chat");
    }

    #[test]
    fn removing_reports_storage_errors_other_than_a_missing_file() {
        let (home, sessions) = repository();
        let id = doc().id;
        sessions.remove(&id).unwrap();
        std::fs::create_dir_all(home.path().join("gui-sessions").join(format!("{id}.json")))
            .unwrap();
        assert!(matches!(sessions.remove(&id), Err(SessionError::Storage)));
    }

    #[test]
    fn session_listing_stops_past_ten_thousand_entries() {
        let (home, sessions) = repository();
        let directory = home.path().join("gui-sessions");
        std::fs::create_dir(&directory).unwrap();
        for index in 0..10_001 {
            std::fs::write(directory.join(format!("{index}.txt")), "").unwrap();
        }
        assert!(sessions.list(false, 0, 10).unwrap().0.is_empty());
        std::fs::write(directory.join("one-more.txt"), "").unwrap();
        assert!(matches!(
            sessions.list(false, 0, 10),
            Err(SessionError::Storage)
        ));
    }

    #[test]
    fn pages_end_exactly_at_the_last_item() {
        let (_home, sessions) = repository();
        let first = sessions.create("one", AT).unwrap();
        sessions.create("two", AT).unwrap();
        assert_eq!(sessions.list(false, 0, 2).unwrap().1, None);
        assert_eq!(sessions.list(false, 0, 1).unwrap().1.as_deref(), Some("1"));
        let doc = sessions
            .append(&first.id, message("user", "a"), None)
            .unwrap();
        sessions
            .append(&first.id, message("user", "b"), Some(doc.revision))
            .unwrap();
        assert_eq!(sessions.messages(&first.id, 0, 2).unwrap().1, None);
        assert_eq!(
            sessions.messages(&first.id, 0, 1).unwrap().1.as_deref(),
            Some("1")
        );
    }

    #[test]
    fn search_skips_archived_chats_unless_asked() {
        let (_home, sessions) = repository();
        let created = sessions.create("needle chat", AT).unwrap();
        assert_eq!(sessions.search("needle", false).unwrap().len(), 1);
        sessions.archive(&created.id, true, None, AT).unwrap();
        assert!(sessions.search("needle", false).unwrap().is_empty());
        assert_eq!(sessions.search("needle", true).unwrap().len(), 1);
    }
}
