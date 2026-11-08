use crate::terminal::{ChatReply, ChatSummary};
use crate::tui_theme::{self, Theme};
use crossterm::event::Event;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use futures_util::{Stream, StreamExt};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
use rigspark_runtime::{harness::HarnessMessage, sessions::gui_text};
use std::io;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use unicode_segmentation::UnicodeSegmentation;

const MAX_STREAM_BYTES: usize = 1024 * 1024;

fn scroll_event(view: &mut ChatView, event: &Event) {
    if let Event::Mouse(mouse) = event {
        match mouse.kind {
            MouseEventKind::ScrollUp => view.scroll = view.scroll.saturating_add(3),
            MouseEventKind::ScrollDown => view.scroll = view.scroll.saturating_sub(3),
            _ => (),
        }
    }
}

fn flush_clipboard(view: &mut ChatView) -> io::Result<()> {
    match view.clipboard.take() {
        Some(text) => tui_theme::copy_to_clipboard(&text),
        None => Ok(()),
    }
}

pub async fn drive<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    events: &mut (impl Stream<Item = io::Result<Event>> + Unpin),
    engine: &impl crate::terminal::ChatEngine,
    view: &mut ChatView,
    color: bool,
    cancel: &CancellationToken,
) -> io::Result<u8> {
    loop {
        terminal
            .draw(|frame| render(frame, view, color))
            .map_err(|error| io::Error::other(error.to_string()))?;
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(130),
            event = events.next() => event.ok_or_else(|| io::Error::other("terminal input ended"))??,
        };
        scroll_event(view, &event);
        let action = match event {
            Event::Key(key) => handle_key(view, key),
            Event::Paste(text) => {
                if let Err(error) = view.insert(&text) {
                    view.error = Some(error.to_string());
                }
                InputAction::Continue
            }
            _ => InputAction::Continue,
        };
        flush_clipboard(view)?;
        match action {
            InputAction::Exit => return Ok(u8::from(view.summary.failed_turns > 0)),
            InputAction::Interrupt => {
                cancel.cancel();
                return Ok(130);
            }
            InputAction::Continue => continue,
            InputAction::Submit => (),
        }
        let context = view.submit()?;
        let (sender, mut deltas) = tokio::sync::mpsc::unbounded_channel();
        let reply = engine.reply_streaming(&context, cancel, sender);
        tokio::pin!(reply);
        loop {
            terminal
                .draw(|frame| render(frame, view, color))
                .map_err(|error| io::Error::other(error.to_string()))?;
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Ok(130),
                reply = &mut reply => {view.finish(reply); break;},
                Some(delta) = deltas.recv() => view.stream(&delta),
                () = tokio::time::sleep(Duration::from_millis(250)) => (),
                event = events.next() => {
                    let event = event.ok_or_else(|| io::Error::other("terminal input ended"))??;
                    scroll_event(view, &event);
                    if let Event::Key(key) = event {
                        match handle_key(view,key) {
                            InputAction::Exit => {cancel.cancel(); return Ok(0);}
                            InputAction::Interrupt => {cancel.cancel(); return Ok(130);}
                            _ => (),
                        }
                    }
                    flush_clipboard(view)?;
                },
            }
        }
    }
}

pub async fn run_chat(
    title: &str,
    engine: &impl crate::terminal::ChatEngine,
    color: bool,
    cancel: &CancellationToken,
) -> io::Result<(ChatSummary, u8)> {
    let mut view = ChatView::new(title);
    let mut signals = crate::cancellation::TerminalSignals::new()?;
    let (mut terminal, _restore) = crate::tui_view::enter_terminal()?;
    let mut events = crate::terminal_events::terminal_events();
    let result = tokio::select! {
        result = drive(&mut terminal, &mut events, engine, &mut view, color, cancel) => result,
        result = signals.recv() => result,
    };
    cancel.cancel();
    let code = result?;
    view.summary.cancelled = matches!(code, 129 | 130 | 143);
    Ok((view.summary, code))
}

#[derive(Debug, PartialEq, Eq)]
pub enum InputAction {
    Continue,
    Submit,
    Exit,
    Interrupt,
}

pub struct ChatView {
    title: String,
    draft: String,
    history: Vec<HarnessMessage>,
    pending: Option<String>,
    streaming: String,
    error: Option<String>,
    summary: ChatSummary,
    scroll: usize,
    started: Option<Instant>,
    first_output: Option<Duration>,
    timing: Option<(Option<Duration>, Duration)>,
    clipboard: Option<String>,
    notice: Option<&'static str>,
}

impl ChatView {
    pub fn new(title: &str) -> Self {
        Self {
            title: gui_text(title).chars().take(256).collect(),
            draft: String::new(),
            history: Vec::new(),
            pending: None,
            streaming: String::new(),
            error: None,
            summary: ChatSummary::default(),
            scroll: 0,
            started: None,
            first_output: None,
            timing: None,
            clipboard: None,
            notice: None,
        }
    }
    /// Appends partial assistant output; anything past the 1 MiB reply limit is dropped.
    pub fn stream(&mut self, delta: &str) {
        if self.pending.is_none() {
            return;
        }
        if self.first_output.is_none() {
            self.first_output = self.started.map(|started| started.elapsed());
        }
        let room = MAX_STREAM_BYTES.saturating_sub(self.streaming.len());
        let mut end = delta.len().min(room);
        while !delta.is_char_boundary(end) {
            end -= 1;
        }
        self.streaming.push_str(&delta[..end]);
    }
    pub fn streaming(&self) -> &str {
        &self.streaming
    }
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }
    pub fn draft(&self) -> &str {
        &self.draft
    }
    pub fn history(&self) -> &[HarnessMessage] {
        &self.history
    }
    pub fn summary(&self) -> &ChatSummary {
        &self.summary
    }
    pub fn insert(&mut self, text: &str) -> io::Result<()> {
        let bytes = self.draft.len().saturating_add(text.len());
        if bytes > 32768 {
            return Err(io::Error::other(format!(
                "Draft exceeds 32768 byte limit ({bytes} bytes)"
            )));
        }
        let text = gui_text(text);
        let next = format!("{}{text}", self.draft);
        crate::terminal::validate_draft(&next)?;
        self.draft = next;
        self.error = None;
        Ok(())
    }
    pub fn submit(&mut self) -> io::Result<Vec<HarnessMessage>> {
        if self.pending.is_some() || self.draft.trim().is_empty() {
            return Err(io::Error::other("no submittable draft"));
        }
        let turn = std::mem::take(&mut self.draft);
        self.pending = Some(turn.clone());
        self.error = None;
        self.streaming.clear();
        self.scroll = 0;
        self.started = Some(Instant::now());
        self.first_output = None;
        self.timing = None;
        let mut context = self.history.clone();
        context.push(HarnessMessage {
            role: "user".into(),
            content: turn,
        });
        if context.len() > 20 {
            context.drain(..context.len() - 20);
        }
        Ok(context)
    }
    pub fn finish(&mut self, reply: Result<ChatReply, String>) {
        let Some(turn) = self.pending.take() else {
            return;
        };
        self.streaming.clear();
        if let Some(started) = self.started.take() {
            self.timing = Some((self.first_output.take(), started.elapsed()));
        }
        let reply = reply.and_then(|reply| {
            if reply.content.len() > 1024 * 1024 {
                Err("response exceeds 1 MiB".into())
            } else {
                Ok(reply)
            }
        });
        match reply {
            Ok(reply) => {
                self.history.push(HarnessMessage {
                    role: "user".into(),
                    content: turn,
                });
                self.history.push(HarnessMessage {
                    role: "assistant".into(),
                    content: reply.content,
                });
                if self.history.len() > 20 {
                    self.history.drain(..self.history.len() - 20);
                }
                self.summary.turns += 1;
                if reply.memory_warning {
                    self.summary.memory_warnings += 1;
                    self.error = Some("Failed to record memory".into());
                }
            }
            Err(error) => {
                self.summary.failed_turns += 1;
                self.error = Some(gui_text(&error).chars().take(1024).collect());
            }
        }
    }
}

pub fn handle_key(view: &mut ChatView, key: KeyEvent) -> InputAction {
    if key.kind == KeyEventKind::Release {
        return InputAction::Continue;
    }
    if key.code == KeyCode::Esc {
        return InputAction::Exit;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        if view.draft.is_empty() || view.pending.is_some() {
            return InputAction::Interrupt;
        }
        view.draft.clear();
        return InputAction::Continue;
    }
    view.notice = None;
    match key.code {
        KeyCode::PageUp => {
            view.scroll = view.scroll.saturating_add(10).min(1 << 20);
            return InputAction::Continue;
        }
        KeyCode::PageDown => {
            view.scroll = view.scroll.saturating_sub(10);
            return InputAction::Continue;
        }
        KeyCode::End => {
            view.scroll = 0;
            return InputAction::Continue;
        }
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let last = view
                .history
                .iter()
                .rev()
                .find(|message| message.role == "assistant")
                .map(|message| message.content.clone());
            match last {
                Some(text) => {
                    view.clipboard = Some(text);
                    view.notice = Some("Copied last reply to clipboard");
                }
                None => view.notice = Some("No reply to copy yet"),
            }
            return InputAction::Continue;
        }
        _ => (),
    }
    if view.pending.is_some() {
        return InputAction::Continue;
    }
    let inserted = match key.code {
        KeyCode::Enter if !view.draft.trim().is_empty() => return InputAction::Submit,
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => Some("\n".into()),
        KeyCode::Char(character)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                && !character.is_control() =>
        {
            Some(character.to_string())
        }
        KeyCode::Backspace => {
            if let Some((position, _)) = view.draft.grapheme_indices(true).next_back() {
                view.draft.truncate(position);
            }
            None
        }
        _ => None,
    };
    if let Some(text) = inserted
        && let Err(error) = view.insert(&text)
    {
        view.error = Some(error.to_string());
    }
    InputAction::Continue
}

fn transcript_lines(view: &ChatView) -> Vec<String> {
    let streaming = (view.pending.is_some() && !view.streaming.is_empty()).then(|| {
        (
            " ",
            crate::accessible_text::chat_message(&view.streaming)
                .unwrap_or_else(|_| "[message exceeds display limits]".into()),
        )
    });
    let pending = view.pending.as_ref().map(|turn| {
        (
            ">",
            crate::accessible_text::multiline(turn)
                .unwrap_or_else(|_| "[draft exceeds display limits]".into()),
        )
    });
    let history = view.history.iter().rev().take(10).map(|message| {
        (
            if message.role == "user" { ">" } else { " " },
            crate::accessible_text::chat_message(&message.content)
                .unwrap_or_else(|_| "[message exceeds display limits]".into()),
        )
    });
    let mut retained_bytes = 0usize;
    let mut lines = Vec::new();
    'messages: for (prefix, visible) in streaming.into_iter().chain(pending).chain(history) {
        for line in visible.lines().rev() {
            let bytes = prefix.len() + 1 + line.len() + 1;
            if retained_bytes + bytes > 192 * 1024 {
                break 'messages;
            }
            retained_bytes += bytes;
            lines.push(format!("{prefix} {line}"));
        }
    }
    lines.reverse();
    lines
}

pub fn render(frame: &mut Frame<'_>, view: &ChatView, color: bool) {
    let area = frame.area();
    let theme = Theme::new(color);
    let roomy = area.height > 12;
    let [header, transcript, status, input, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(if roomy {
            5
        } else if area.height > 8 {
            3
        } else {
            1
        }),
        Constraint::Length(if area.height > 8 { 1 } else { 0 }),
    ])
    .areas(area);
    theme.header(
        frame,
        header,
        &format!("rigspark / chat / {}", view.title),
        &format!(
            "{} turn{}",
            view.summary.turns,
            if view.summary.turns == 1 { "" } else { "s" }
        ),
    );
    let (rows, scrolled) = transcript_rows(view, transcript, theme);
    frame.render_widget(Paragraph::new(rows), transcript);
    let seconds = |duration: Duration| format!("{:.1}s", duration.as_secs_f64());
    let status_line = if view.pending.is_some() {
        let mut text = " Waiting for response...".to_owned();
        if let Some(started) = view.started {
            text.push_str(&format!(" {}", seconds(started.elapsed())));
        }
        if let Some(first) = view.first_output {
            text.push_str(&format!(" · first output {}", seconds(first)));
        }
        Line::styled(text, theme.warning())
    } else if let Some(error) = &view.error {
        Line::styled(format!(" {error}"), theme.error())
    } else if let Some(notice) = view.notice {
        Line::styled(format!(" {notice}"), theme.success())
    } else {
        let mut text = " Ready".to_owned();
        if let Some((first, total)) = view.timing {
            if let Some(first) = first {
                text.push_str(&format!(" · first output {}", seconds(first)));
            }
            text.push_str(&format!(" · {} total", seconds(total)));
        }
        Line::styled(text, theme.muted())
    };
    let right = if scrolled > 0 {
        Line::styled(format!("↑ {scrolled} lines · End follows "), theme.accent())
    } else {
        Line::default()
    };
    theme.bar(frame, status, status_line, right);
    let inner = if roomy {
        theme.panel(frame, input, "Message")
    } else {
        input
    };
    let mut draft: Vec<&str> = view.draft.split('\n').collect();
    let keep = usize::from(inner.height.max(1));
    draft.drain(..draft.len().saturating_sub(keep));
    let last = draft.len() - 1;
    let lines: Vec<_> = draft
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            let mut spans = vec![if index == 0 {
                Span::styled("> ", theme.title())
            } else {
                Span::raw("  ")
            }];
            spans.push(Span::raw(tail(
                text,
                usize::from(inner.width).saturating_sub(3),
            )));
            if index == last && view.pending.is_none() {
                spans.push(Span::styled("▌", theme.accent()));
                if view.draft.is_empty() {
                    spans.push(Span::styled(" Type a message", theme.muted()));
                }
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    frame.render_widget(
        Paragraph::new(theme.hints(
            &[
                ("Enter", "send"),
                ("Ctrl+J", "newline"),
                ("PgUp/PgDn", "scroll"),
                ("Ctrl+Y", "copy reply"),
                ("Ctrl+C", "clear/stop"),
                ("Esc", "exit"),
            ],
            footer.width,
        )),
        footer,
    );
}

/// The trailing graphemes of `text` that fit in `width` cells, so the cursor stays visible.
fn tail(text: &str, width: usize) -> String {
    let mut used = 0;
    let mut kept: Vec<&str> = text
        .graphemes(true)
        .rev()
        .take_while(|grapheme| {
            used += crate::tui_theme::width(grapheme);
            used <= width
        })
        .collect();
    kept.reverse();
    kept.concat()
}

/// Transcript rows with role labels and lightweight markdown, windowed by the scroll offset.
/// Returns the visible rows and how many rows sit below the window.
fn transcript_rows(view: &ChatView, area: Rect, theme: Theme) -> (Vec<Line<'static>>, usize) {
    let height = usize::from(area.height);
    let width = usize::from(area.width.saturating_sub(3)).max(1);
    let code = theme.accent();
    let mut rows = Vec::new();
    let mut previous = None;
    let mut fenced = false;
    for line in &transcript_lines(view) {
        let user = line.starts_with('>');
        if previous != Some(user) {
            if previous.is_some() {
                rows.push(Line::default());
            }
            rows.push(if user {
                Line::styled(" you", theme.title())
            } else {
                Line::styled(" assistant", theme.success())
            });
            fenced = false;
        }
        previous = Some(user);
        let text = line.get(2..).unwrap_or("");
        let trimmed = text.trim_start();
        if !user && trimmed.starts_with("```") {
            fenced = !fenced;
            rows.push(Line::styled(format!("   {}", trimmed), theme.muted()));
            continue;
        }
        let (lead, body, style, markup) = if user {
            ("", text, theme.strong(), false)
        } else if fenced {
            ("", text, code, false)
        } else if let Some(heading) = trimmed
            .strip_prefix("### ")
            .or_else(|| trimmed.strip_prefix("## "))
            .or_else(|| trimmed.strip_prefix("# "))
        {
            ("", heading, theme.title(), true)
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            ("• ", item, Style::default(), true)
        } else {
            ("", text, Style::default(), true)
        };
        let indent = tui_theme::width(lead);
        for (index, piece) in tui_theme::wrap(body, width.saturating_sub(indent).max(1))
            .into_iter()
            .enumerate()
        {
            let mut spans = vec![
                Span::raw("   "),
                Span::styled(
                    if index == 0 {
                        lead.to_owned()
                    } else {
                        " ".repeat(indent)
                    },
                    theme.muted(),
                ),
            ];
            if markup {
                spans.extend(inline_markdown(&piece, style, code));
            } else {
                spans.push(Span::styled(piece, style));
            }
            rows.push(Line::from(spans));
        }
    }
    let below = view.scroll.min(rows.len().saturating_sub(height));
    let end = rows.len() - below;
    rows.truncate(end);
    let start = end.saturating_sub(height);
    (rows.split_off(start), below)
}

/// Renders `code` spans and `**bold**` runs; unmatched markers are shown literally.
fn inline_markdown(text: &str, base: Style, code: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let segments: Vec<&str> = text.split('`').collect();
    let balanced = segments.len() % 2 == 1;
    for (index, segment) in segments.iter().enumerate() {
        if balanced && index % 2 == 1 {
            spans.push(Span::styled((*segment).to_owned(), code));
            continue;
        }
        let prefix = if !balanced && index > 0 { "`" } else { "" };
        let parts: Vec<&str> = segment.split("**").collect();
        let bold_balanced = parts.len() % 2 == 1;
        for (part_index, part) in parts.iter().enumerate() {
            let mut content = String::new();
            if part_index == 0 {
                content.push_str(prefix);
            }
            if !bold_balanced && part_index > 0 {
                content.push_str("**");
            }
            content.push_str(part);
            if content.is_empty() {
                continue;
            }
            let style = if bold_balanced && part_index % 2 == 1 {
                base.add_modifier(Modifier::BOLD)
            } else {
                base
            };
            spans.push(Span::styled(content, style));
        }
    }
    spans
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn newline_heavy_transcript_and_pending_draft_share_the_display_budget() {
        let mut view = ChatView::new("local");
        for _ in 0..5 {
            view.insert("question").unwrap();
            view.submit().unwrap();
            view.finish(Ok(crate::terminal::ChatReply {
                content: "\n".repeat(65536),
                memory_warning: false,
            }));
        }
        view.insert("pending\r\nlast line").unwrap();
        view.submit().unwrap();
        let lines = transcript_lines(&view);
        assert!(lines.iter().map(|line| line.len() + 1).sum::<usize>() <= 192 * 1024);
        assert_eq!(lines.last().unwrap(), "> last line");
        assert!(lines.iter().all(|line| !line.contains(['\r', '\n'])));
        assert_eq!(view.history.len(), 10);
    }

    #[test]
    fn transcript_budget_keeps_latest_rows_without_mutating_history() {
        let mut view = ChatView::new("local");
        for index in 0..6 {
            view.insert("question").unwrap();
            view.submit().unwrap();
            view.finish(Ok(crate::terminal::ChatReply {
                content: format!("{}\nLATEST-{index}", "x".repeat(60 * 1024)),
                memory_warning: false,
            }));
        }
        let before: Vec<_> = view
            .history
            .iter()
            .map(|message| message.content.clone())
            .collect();
        let lines = transcript_lines(&view);
        assert!(lines.iter().map(|line| line.len() + 1).sum::<usize>() <= 192 * 1024);
        assert!(lines.last().unwrap().contains("LATEST-5"));
        assert!(lines.iter().filter(|line| line.len() > 1024).count() < 6);
        assert_eq!(
            before,
            view.history
                .iter()
                .map(|message| message.content.clone())
                .collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod chat_tests {
    use super::*;
    use crate::terminal::ChatReply;
    use crossterm::event::{MouseEvent, MouseEventKind};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }
    fn reply(content: &str) -> Result<ChatReply, String> {
        Ok(ChatReply {
            content: content.into(),
            memory_warning: false,
        })
    }
    fn turn(view: &mut ChatView, question: &str, answer: &str) {
        view.insert(question).unwrap();
        view.submit().unwrap();
        view.finish(reply(answer));
    }

    #[test]
    fn streamed_output_is_kept_up_to_one_mib_on_character_boundaries() {
        let mut view = ChatView::new("t");
        view.stream("ignored");
        assert_eq!(view.streaming(), "");
        view.insert("q").unwrap();
        view.submit().unwrap();
        view.stream(&"a".repeat(4000));
        assert_eq!(view.streaming().len(), 4000);
        view.stream(&"b".repeat(MAX_STREAM_BYTES - 4001));
        assert_eq!(view.streaming().len(), MAX_STREAM_BYTES - 1);
        // A two-byte character does not fit in the last byte.
        view.stream("é");
        assert_eq!(view.streaming().len(), MAX_STREAM_BYTES - 1);
        view.stream("cd");
        assert_eq!(view.streaming().len(), MAX_STREAM_BYTES);
        assert!(view.streaming().ends_with('c'));
    }

    #[test]
    fn replies_up_to_one_mib_are_recorded() {
        let mut view = ChatView::new("t");
        view.insert("q").unwrap();
        view.submit().unwrap();
        view.finish(reply(&"r".repeat(1024 * 1024)));
        assert_eq!(view.summary().turns, 1);
        view.insert("q").unwrap();
        view.submit().unwrap();
        view.finish(reply(&"r".repeat(1024 * 1024 + 1)));
        assert_eq!(view.summary().failed_turns, 1);
    }

    #[test]
    fn the_wheel_scrolls_three_lines_and_page_keys_ten() {
        let mut view = ChatView::new("t");
        let wheel = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            })
        };
        scroll_event(&mut view, &wheel(MouseEventKind::ScrollUp));
        scroll_event(&mut view, &wheel(MouseEventKind::ScrollUp));
        assert_eq!(view.scroll, 6);
        scroll_event(&mut view, &wheel(MouseEventKind::ScrollDown));
        assert_eq!(view.scroll, 3);
        handle_key(&mut view, key(KeyCode::PageUp));
        assert_eq!(view.scroll, 13);
        handle_key(&mut view, key(KeyCode::PageDown));
        assert_eq!(view.scroll, 3);
        handle_key(&mut view, key(KeyCode::End));
        assert_eq!(view.scroll, 0);
    }

    #[test]
    fn only_control_combinations_trigger_shortcuts() {
        let mut view = ChatView::new("t");
        for code in [KeyCode::Char('y'), KeyCode::Char('j')] {
            assert_eq!(handle_key(&mut view, key(code)), InputAction::Continue);
        }
        assert_eq!(view.draft(), "yj");
        assert!(view.take_clipboard().is_none());
        handle_key(&mut view, with(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(view.draft(), "yj\n");
        for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
            handle_key(&mut view, with(KeyCode::Char('x'), modifiers));
        }
        assert_eq!(view.draft(), "yj\n");
        handle_key(&mut view, key(KeyCode::Backspace));
        assert_eq!(view.draft(), "yj");
        // Enter submits only a draft with something in it.
        let mut blank = ChatView::new("t");
        blank.insert("   ").unwrap();
        assert_eq!(
            handle_key(&mut blank, key(KeyCode::Enter)),
            InputAction::Continue
        );
        assert_eq!(
            handle_key(&mut view, key(KeyCode::Enter)),
            InputAction::Submit
        );
        // Ctrl+Y copies the last reply.
        turn(&mut view, "q", "answer");
        handle_key(&mut view, with(KeyCode::Char('y'), KeyModifiers::CONTROL));
        assert_eq!(view.take_clipboard().as_deref(), Some("answer"));
    }

    #[test]
    fn the_transcript_keeps_exactly_192_kib() {
        let mut view = ChatView::new("t");
        let line = "x".repeat(1021);
        // Four turns: "> q" lines take 4 bytes each, 1021-character lines 1024. The oldest
        // reply starts with a 1005-character line so the total is exactly 192 KiB.
        for count in [47, 48, 48, 48] {
            let mut lines = vec![line.clone(); count];
            if count == 47 {
                lines.insert(0, "y".repeat(1005));
            }
            turn(&mut view, "q", &lines.join("\n"));
        }
        let lines = transcript_lines(&view);
        assert_eq!(
            lines.iter().map(|line| line.len() + 1).sum::<usize>(),
            192 * 1024
        );
        assert_eq!(lines.len(), 4 + 192);
        assert_eq!(lines[0], "> q");
        assert!(lines[1].ends_with('y'));
        // One more byte pushes out the oldest line, the first question.
        let mut longer = ChatView::new("t");
        for count in [47, 48, 48, 48] {
            let mut lines = vec![line.clone(); count];
            if count == 47 {
                lines.insert(0, "y".repeat(1006));
            }
            turn(&mut longer, "q", &lines.join("\n"));
        }
        let lines = transcript_lines(&longer);
        assert_eq!(lines.len(), 4 + 192 - 1);
        assert!(lines[0].ends_with('y'));
    }

    fn screen(view: &ChatView, height: u16) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, height)).unwrap();
        terminal.draw(|frame| render(frame, view, false)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..60)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn the_layout_grows_input_and_footer_with_height() {
        let mut view = ChatView::new("t");
        view.insert("one").unwrap();
        for word in ["two", "three", "four", "five"] {
            handle_key(&mut view, with(KeyCode::Char('j'), KeyModifiers::CONTROL));
            view.insert(word).unwrap();
        }
        let joined = |height| screen(&view, height).join("\n");
        // Over 12 rows: a titled panel with three draft rows and a footer.
        let tall = joined(13);
        assert!(tall.contains("Message"), "{tall}");
        assert!(tall.contains("three") && !tall.contains("two"), "{tall}");
        assert!(tall.contains("Enter send"));
        // 9 to 12 rows: three plain draft rows and a footer.
        for height in [9, 12] {
            let text = joined(height);
            assert!(!text.contains("Message"), "{text}");
            assert!(text.contains("three") && !text.contains("two"), "{text}");
            assert!(text.contains("Enter send"), "{text}");
        }
        // 8 rows or fewer: one draft row and no footer.
        let short = joined(8);
        assert!(short.contains("five") && !short.contains("four"), "{short}");
        assert!(!short.contains("Enter send"), "{short}");
        // The cursor sits on the last draft line only.
        let rows = screen(&view, 13);
        assert_eq!(rows.iter().filter(|row| row.contains('▌')).count(), 1);
        assert!(rows.iter().any(|row| row.contains("five▌")));
        assert!(!tall.contains("End follows"));
    }

    #[test]
    fn no_cursor_is_drawn_while_waiting() {
        let mut view = ChatView::new("t");
        view.insert("question").unwrap();
        view.submit().unwrap();
        let text = screen(&view, 13).join("\n");
        assert!(text.contains("Waiting for response"));
        assert!(!text.contains('▌'), "{text}");
    }

    #[test]
    fn the_draft_tail_fits_the_input_width() {
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 3), "ab");
        assert_eq!(tail("a\u{754c}", 2), "\u{754c}");
        assert_eq!(tail("abc", 0), "");
    }

    fn row_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.to_string())
            .collect()
    }

    #[test]
    fn fenced_assistant_code_is_shown_literally() {
        let theme = Theme::with_palette(false, tui_theme::Palette::Default);
        let mut view = ChatView::new("t");
        turn(&mut view, "```", "```\na **b** c\n```\nd **e** f");
        let (rows, _) = transcript_rows(&view, Rect::new(0, 0, 60, 20), theme);
        let texts: Vec<_> = rows.iter().map(row_text).collect();
        // The user's backticks are plain text, not a fence.
        assert!(texts.contains(&"   ```".to_owned()), "{texts:?}");
        let fenced = rows
            .iter()
            .find(|row| row_text(row).contains("a **b** c"))
            .expect("literal code row");
        assert!(
            fenced
                .spans
                .iter()
                .all(|span| !span.style.add_modifier.contains(Modifier::BOLD))
        );
        let after = rows
            .iter()
            .find(|row| row_text(row).contains("d e f"))
            .expect("markdown after the fence");
        assert!(after.spans.iter().any(|span| span.content == "e"
            && span.style.add_modifier.contains(Modifier::BOLD)));
    }

    fn marked(text: &str) -> Vec<(String, bool, bool)> {
        let code = Style::default().add_modifier(Modifier::ITALIC);
        inline_markdown(text, Style::default(), code)
            .into_iter()
            .map(|span| {
                (
                    span.content.to_string(),
                    span.style.add_modifier.contains(Modifier::BOLD),
                    span.style.add_modifier.contains(Modifier::ITALIC),
                )
            })
            .collect()
    }
    fn plain(text: &str) -> (String, bool, bool) {
        (text.into(), false, false)
    }
    fn bold(text: &str) -> (String, bool, bool) {
        (text.into(), true, false)
    }
    fn code(text: &str) -> (String, bool, bool) {
        (text.into(), false, true)
    }

    #[test]
    fn inline_markdown_styles_balanced_markers_and_keeps_the_rest() {
        assert_eq!(marked("a `x` b"), [plain("a "), code("x"), plain(" b")]);
        assert_eq!(marked("`a` `b`"), [code("a"), plain(" "), code("b")]);
        assert_eq!(marked("a `x b"), [plain("a "), plain("`x b")]);
        assert_eq!(
            marked("a`b`c`d"),
            [plain("a"), plain("`b"), plain("`c"), plain("`d")]
        );
        assert_eq!(marked("a **b** c"), [plain("a "), bold("b"), plain(" c")]);
        assert_eq!(marked("**b**"), [bold("b")]);
        assert_eq!(
            marked("a **b** c **d** e"),
            [plain("a "), bold("b"), plain(" c "), bold("d"), plain(" e")]
        );
        assert_eq!(marked("a **b c"), [plain("a "), plain("**b c")]);
    }

    struct Silent;
    impl crate::terminal::ChatEngine for Silent {
        async fn reply(
            &self,
            _: &[HarnessMessage],
            _: &CancellationToken,
        ) -> Result<ChatReply, String> {
            Err("unused".into())
        }
    }

    async fn drive_with(view: &mut ChatView, events: Vec<Event>) -> u8 {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 20)).unwrap();
        let mut events = futures_util::stream::iter(events.into_iter().map(Ok));
        drive(
            &mut terminal,
            &mut events,
            &Silent,
            view,
            false,
            &CancellationToken::new(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn the_driver_pastes_copies_and_exits_with_the_failure_status() {
        let mut view = ChatView::new("t");
        turn(&mut view, "q", "answer");
        let code = drive_with(
            &mut view,
            vec![
                Event::Paste("pasted".into()),
                Event::Key(with(KeyCode::Char('y'), KeyModifiers::CONTROL)),
                Event::Key(key(KeyCode::Esc)),
            ],
        )
        .await;
        assert_eq!(code, 0);
        assert_eq!(view.draft(), "pasted");
        // The copy was handed to the terminal.
        assert!(view.take_clipboard().is_none());
        view.insert("x").unwrap();
        view.submit().unwrap();
        view.finish(Err("failed".into()));
        assert_eq!(
            drive_with(&mut view, vec![Event::Key(key(KeyCode::Esc))]).await,
            1
        );
    }
}
