use crate::tui_theme::{SyncWriter, Theme};
use crossterm::{
    cursor::{Hide, Show},
    event::{
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
        MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Position, Rect},
    text::{Line, Span},
    widgets::{HighlightSpacing, List, ListItem, ListState},
};
use rigspark_core::reports::strip_control;
use std::io;
use unicode_segmentation::UnicodeSegmentation;

pub struct ReportView {
    title: String,
    lines: Vec<String>,
    state: ListState,
    query: String,
    searching: bool,
    color: bool,
    picker: bool,
    horizontal: usize,
    page: usize,
    list_area: Rect,
}

impl ReportView {
    pub fn new(title: &str, text: &str, color: bool) -> io::Result<Self> {
        if text.len() > 1024 * 1024 || text.lines().count() > 20000 || title.len() > 256 {
            return Err(io::Error::other("report exceeds terminal display limits"));
        }
        let mut lines: Vec<_> = text.lines().map(strip_control).collect();
        if lines.is_empty() {
            lines.push("No results".into());
        }
        Ok(Self {
            title: strip_control(title),
            lines,
            state: ListState::default().with_selected(Some(0)),
            query: String::new(),
            searching: false,
            color,
            picker: false,
            horizontal: 0,
            page: 10,
            list_area: Rect::default(),
        })
    }
    pub fn selected(&self) -> usize {
        self.state.selected().unwrap_or(0)
    }
    fn select(&mut self, selected: usize) {
        self.state.select(Some(selected.min(self.lines.len() - 1)));
    }
    fn find(&mut self, next: bool) {
        if self.query.is_empty() {
            return;
        }
        let query = self.query.to_lowercase();
        let start = if next { self.selected() + 1 } else { 0 };
        for offset in 0..self.lines.len() {
            let index = (start + offset) % self.lines.len();
            if self.lines[index].to_lowercase().contains(&query) {
                self.select(index);
                break;
            }
        }
    }
}

/// Wheel scrolls; clicking a row selects it. Returns true when an already-selected row is clicked again.
pub fn handle_mouse(view: &mut ReportView, mouse: MouseEvent) -> bool {
    match mouse.kind {
        MouseEventKind::ScrollDown => view.select(view.selected().saturating_add(3)),
        MouseEventKind::ScrollUp => view.select(view.selected().saturating_sub(3)),
        MouseEventKind::Down(MouseButton::Left)
            if view
                .list_area
                .contains(Position::new(mouse.column, mouse.row)) =>
        {
            let row = view.state.offset() + usize::from(mouse.row - view.list_area.y);
            if row < view.lines.len() {
                let again = row == view.selected();
                view.select(row);
                return again;
            }
        }
        _ => (),
    }
    false
}

pub fn handle_key(view: &mut ReportView, key: KeyEvent) -> bool {
    if key.kind == KeyEventKind::Release {
        return false;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return true;
    }
    if view.searching {
        match key.code {
            KeyCode::Esc => {
                view.searching = false;
                view.query.clear();
            }
            KeyCode::Enter => view.searching = false,
            KeyCode::Backspace => {
                view.query.pop();
                view.find(false);
            }
            KeyCode::Char(character)
                if !character.is_control() && view.query.len() + character.len_utf8() <= 256 =>
            {
                view.query.push(character);
                view.find(false);
            }
            _ => (),
        }
        return false;
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc | KeyCode::Enter => return true,
        KeyCode::Down | KeyCode::Char('j') => view.select(view.selected().saturating_add(1)),
        KeyCode::Up | KeyCode::Char('k') => view.select(view.selected().saturating_sub(1)),
        KeyCode::PageDown => view.select(view.selected().saturating_add(view.page)),
        KeyCode::PageUp => view.select(view.selected().saturating_sub(view.page)),
        KeyCode::Home => view.select(0),
        KeyCode::End => view.select(view.lines.len() - 1),
        KeyCode::Right => view.horizontal = view.horizontal.saturating_add(8).min(32768),
        KeyCode::Left => view.horizontal = view.horizontal.saturating_sub(8),
        KeyCode::Char('/') => {
            view.searching = true;
            view.query.clear();
        }
        KeyCode::Char('n') => view.find(true),
        _ => (),
    }
    false
}

pub fn render(frame: &mut Frame<'_>, view: &mut ReportView) {
    let area = frame.area();
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(if area.height >= 4 { 1 } else { 0 }),
    ])
    .areas(area);
    let theme = Theme::new(view.color);
    theme.header(
        frame,
        header,
        &format!("rigspark / {}", view.title),
        if view.picker { "Select" } else { "Report" },
    );
    let inner = theme.panel(frame, body, if view.picker { "Choices" } else { "Output" });
    view.page = usize::from(inner.height.max(1));
    view.list_area = inner;
    let query = view.query.to_lowercase();
    let items: Vec<_> = view
        .lines
        .iter()
        .map(|line| {
            let visible: String = line.graphemes(true).skip(view.horizontal).collect();
            ListItem::new(Line::from(matched(&visible, &query, theme)))
        })
        .collect();
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("▸ ")
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_style(theme.selection()),
        inner,
        &mut view.state,
    );
    crate::tui_theme::scrollbar(
        frame,
        body,
        view.lines.len(),
        view.state.offset(),
        usize::from(inner.height),
        theme,
    );
    let position = Line::styled(
        format!("{}/{} ", view.selected() + 1, view.lines.len()),
        theme.muted(),
    );
    let left = if view.searching {
        Line::from(vec![
            Span::styled(" /", theme.title()),
            Span::raw(view.query.clone()),
            Span::styled("▌", theme.accent()),
        ])
    } else if view.picker {
        theme.hints(
            &[
                ("Enter", "select"),
                ("↑↓", "move"),
                ("/", "find"),
                ("n", "next"),
                ("q", "cancel"),
            ],
            footer.width.saturating_sub(10),
        )
    } else {
        theme.hints(
            &[
                ("q", "close"),
                ("↑↓", "scroll"),
                ("/", "find"),
                ("n", "next"),
                ("←→", "pan"),
            ],
            footer.width.saturating_sub(10),
        )
    };
    theme.bar(frame, footer, left, position);
}

/// Highlights case-insensitive occurrences of `query`; non-ASCII case folds that change length are left plain.
fn matched(line: &str, query: &str, theme: Theme) -> Vec<Span<'static>> {
    let lower = line.to_lowercase();
    if query.is_empty() || lower.len() != line.len() {
        return vec![Span::raw(line.to_owned())];
    }
    let mut spans = Vec::new();
    let mut cursor = 0;
    for (start, _) in lower.match_indices(query) {
        if start < cursor || !line.is_char_boundary(start) {
            continue;
        }
        spans.push(Span::raw(line[cursor..start].to_owned()));
        spans.push(Span::styled(
            line[start..start + query.len()].to_owned(),
            theme.highlight(),
        ));
        cursor = start + query.len();
    }
    spans.push(Span::raw(line[cursor..].to_owned()));
    spans
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalResource {
    Raw,
    Alternate,
    Cursor,
    Paste,
    Mouse,
}

const TERMINAL_RESOURCES: [TerminalResource; 5] = [
    TerminalResource::Raw,
    TerminalResource::Alternate,
    TerminalResource::Cursor,
    TerminalResource::Paste,
    TerminalResource::Mouse,
];

/// Mouse capture blocks native text selection, so users can opt out.
pub(crate) fn mouse_enabled() -> bool {
    std::env::var_os("RIGSPARK_NO_MOUSE").is_none_or(|value| value.is_empty())
}

fn set_mouse(enabled: bool) -> io::Result<()> {
    #[cfg(windows)]
    {
        if enabled {
            execute!(io::stderr(), crossterm::event::EnableMouseCapture)
        } else {
            execute!(io::stderr(), crossterm::event::DisableMouseCapture)
        }
    }
    #[cfg(not(windows))]
    {
        use std::io::Write;
        // Press and wheel reports only (SGR encoding); motion tracking would flood redraws.
        let sequence: &[u8] = if enabled {
            b"\x1b[?1000h\x1b[?1006h"
        } else {
            b"\x1b[?1006l\x1b[?1000l"
        };
        let mut stderr = io::stderr();
        stderr.write_all(sequence)?;
        stderr.flush()
    }
}

pub(crate) trait TerminalControl {
    fn raw(&self) -> io::Result<bool>;
    fn set(&mut self, resource: TerminalResource, enabled: bool) -> io::Result<()>;
}

pub(crate) struct SystemTerminal;

impl TerminalControl for SystemTerminal {
    fn raw(&self) -> io::Result<bool> {
        crossterm::terminal::is_raw_mode_enabled()
    }
    fn set(&mut self, resource: TerminalResource, enabled: bool) -> io::Result<()> {
        match (resource, enabled) {
            (TerminalResource::Raw, true) => enable_raw_mode(),
            (TerminalResource::Raw, false) => disable_raw_mode(),
            (TerminalResource::Alternate, true) => execute!(io::stderr(), EnterAlternateScreen),
            (TerminalResource::Alternate, false) => execute!(io::stderr(), LeaveAlternateScreen),
            (TerminalResource::Cursor, true) => execute!(io::stderr(), Hide),
            (TerminalResource::Cursor, false) => execute!(io::stderr(), Show),
            (TerminalResource::Paste, true) => {
                execute!(io::stderr(), crossterm::event::EnableBracketedPaste)
            }
            (TerminalResource::Paste, false) => {
                execute!(io::stderr(), crossterm::event::DisableBracketedPaste)
            }
            (TerminalResource::Mouse, true) if mouse_enabled() => set_mouse(true),
            (TerminalResource::Mouse, true) => Ok(()),
            (TerminalResource::Mouse, false) => set_mouse(false),
        }
    }
}

pub(crate) struct RestoreTerminal<Control: TerminalControl = SystemTerminal> {
    control: Control,
    initially_raw: bool,
    acquired: usize,
}

impl<Control: TerminalControl> RestoreTerminal<Control> {
    fn acquire(control: Control) -> io::Result<Self> {
        let initially_raw = control.raw()?;
        let mut restore = Self {
            control,
            initially_raw,
            acquired: 0,
        };
        for resource in TERMINAL_RESOURCES {
            restore.acquired += 1;
            restore.control.set(resource, true)?;
        }
        Ok(restore)
    }
}

impl<Control: TerminalControl> Drop for RestoreTerminal<Control> {
    fn drop(&mut self) {
        for resource in TERMINAL_RESOURCES[..self.acquired].iter().rev() {
            let enabled = *resource == TerminalResource::Raw && self.initially_raw;
            let _ = self.control.set(*resource, enabled);
        }
    }
}

pub(crate) type TerminalBackend = CrosstermBackend<SyncWriter<io::Stderr>>;

pub(crate) fn enter_terminal() -> io::Result<(Terminal<TerminalBackend>, RestoreTerminal)> {
    let restore = RestoreTerminal::acquire(SystemTerminal)?;
    let terminal = Terminal::new(CrosstermBackend::new(SyncWriter::new(io::stderr())))?;
    Ok((terminal, restore))
}

pub async fn show_report(title: &str, text: &str, color: bool) -> io::Result<u8> {
    let mut view = ReportView::new(title, text, color)?;
    let mut signals = crate::cancellation::TerminalSignals::new()?;
    let (mut terminal, _restore) = enter_terminal()?;
    let mut events = crate::terminal_events::terminal_events();
    loop {
        terminal.draw(|frame| render(frame, &mut view))?;
        let event = tokio::select! {
            code = signals.recv() => return code,
            event = events.next() => event.ok_or_else(|| io::Error::other("terminal input ended"))??,
        };
        if let Event::Mouse(mouse) = event {
            handle_mouse(&mut view, mouse);
        }
        if let Event::Key(key) = event {
            let interrupted =
                key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
            if handle_key(&mut view, key) {
                return Ok(if interrupted { 130 } else { 0 });
            }
        }
    }
}
pub(crate) async fn next_navigation_event(
    events: &mut (impl futures_util::Stream<Item = io::Result<Event>> + Unpin),
) -> io::Result<Event> {
    let event = events
        .next()
        .await
        .ok_or_else(|| io::Error::other("terminal input ended"))??;
    let Event::Key(escape) = event else {
        return Ok(event);
    };
    if escape.code != KeyCode::Esc || escape.kind == KeyEventKind::Release {
        return Ok(event);
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(50);
    let mut bracket = false;
    loop {
        let Ok(next) = tokio::time::timeout_at(deadline, events.next()).await else {
            return Ok(event);
        };
        let next = next.ok_or_else(|| io::Error::other("terminal input ended"))??;
        let Event::Key(mut key) = next else {
            continue;
        };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(next);
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return Ok(event);
        }
        match (bracket, key.code) {
            (false, KeyCode::Char('[')) => bracket = true,
            (true, KeyCode::Char('H' | 'F')) => {
                key.code = if key.code == KeyCode::Char('H') {
                    KeyCode::Home
                } else {
                    KeyCode::End
                };
                return Ok(Event::Key(key));
            }
            _ => return Ok(event),
        }
    }
}

pub async fn pick(title: &str, choices: &[String], color: bool) -> io::Result<(Option<usize>, u8)> {
    crate::accessible::validate_picker_choices(choices, 8192)?;
    let mut view = ReportView::new(title, &choices.join("\n"), color)?;
    view.picker = true;
    let mut signals = crate::cancellation::TerminalSignals::new()?;
    let (mut terminal, _restore) = enter_terminal()?;
    let mut events = crate::terminal_events::terminal_events();
    loop {
        terminal.draw(|frame| render(frame, &mut view))?;
        let event = tokio::select! {
            code = signals.recv() => return Ok((None, code?)),
            event = next_navigation_event(&mut events) => event?,
        };
        if let Event::Mouse(mouse) = event
            && handle_mouse(&mut view, mouse)
            && !view.searching
        {
            return Ok((Some(view.selected()), 0));
        }
        if let Event::Key(key) = event {
            if key.kind == KeyEventKind::Release {
                continue;
            }
            if key.code == KeyCode::Enter && !view.searching {
                return Ok((Some(view.selected()), 0));
            }
            let interrupted =
                key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
            if handle_key(&mut view, key) {
                return Ok((None, if interrupted { 130 } else { 0 }));
            }
        }
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Default)]
    struct State {
        raw: bool,
        calls: Vec<(TerminalResource, bool)>,
    }

    struct Control {
        state: Rc<RefCell<State>>,
        fail: Option<(TerminalResource, bool)>,
    }

    impl TerminalControl for Control {
        fn raw(&self) -> io::Result<bool> {
            Ok(self.state.borrow().raw)
        }
        fn set(&mut self, resource: TerminalResource, enabled: bool) -> io::Result<()> {
            let mut state = self.state.borrow_mut();
            state.calls.push((resource, enabled));
            if resource == TerminalResource::Raw {
                state.raw = enabled;
            }
            if self.fail == Some((resource, enabled)) {
                return Err(io::Error::other("injected failure after mutation"));
            }
            Ok(())
        }
    }

    #[test]
    fn partial_acquisition_restores_each_owned_resource_and_original_raw_mode() {
        use TerminalResource::*;
        for fail in [Raw, Alternate, Cursor, Paste, Mouse] {
            let state = Rc::new(RefCell::new(State::default()));
            assert!(
                RestoreTerminal::acquire(Control {
                    state: state.clone(),
                    fail: Some((fail, true))
                })
                .is_err()
            );
            let state = state.borrow();
            assert!(!state.raw);
            for resource in [Raw, Alternate, Cursor, Paste, Mouse] {
                let acquired = state.calls.contains(&(resource, true));
                assert_eq!(
                    state
                        .calls
                        .iter()
                        .filter(|call| **call == (resource, false))
                        .count(),
                    usize::from(acquired)
                );
            }
        }
        let state = Rc::new(RefCell::new(State {
            raw: true,
            ..Default::default()
        }));
        drop(
            RestoreTerminal::acquire(Control {
                state: state.clone(),
                fail: None,
            })
            .unwrap(),
        );
        assert!(state.borrow().raw);
        assert!(!state.borrow().calls.contains(&(Raw, false)));
    }

    #[test]
    fn restoration_continues_after_independent_output_failure_and_repeated_sessions() {
        use TerminalResource::*;
        for failed in [Mouse, Paste, Cursor, Alternate, Raw] {
            let state = Rc::new(RefCell::new(State::default()));
            for _ in 0..20 {
                let restore = RestoreTerminal::acquire(Control {
                    state: state.clone(),
                    fail: Some((failed, false)),
                })
                .unwrap();
                assert!(state.borrow().raw);
                drop(restore);
                assert!(!state.borrow().raw);
            }
            for resource in [Raw, Alternate, Cursor, Paste, Mouse] {
                assert_eq!(
                    state
                        .borrow()
                        .calls
                        .iter()
                        .filter(|call| **call == (resource, false))
                        .count(),
                    20
                );
            }
        }
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn view(lines: usize) -> ReportView {
        let text: Vec<_> = (0..lines).map(|n| format!("line{n}")).collect();
        ReportView::new("title", &text.join("\n"), false).unwrap()
    }
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    fn with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }
    fn press(view: &mut ReportView, codes: &[KeyCode]) {
        for code in codes {
            assert!(!handle_key(view, key(*code)), "{code:?}");
        }
    }
    fn typed(view: &mut ReportView, text: &str) {
        for character in text.chars() {
            handle_key(view, key(KeyCode::Char(character)));
        }
    }

    #[test]
    fn keys_move_by_line_page_and_end_and_stop_at_the_last_line() {
        let mut view = view(30);
        press(&mut view, &[KeyCode::Down, KeyCode::Char('j')]);
        assert_eq!(view.selected(), 2);
        press(&mut view, &[KeyCode::Up]);
        assert_eq!(view.selected(), 1);
        press(&mut view, &[KeyCode::Char('k'), KeyCode::Up]);
        assert_eq!(view.selected(), 0);
        press(&mut view, &[KeyCode::PageDown]);
        assert_eq!(view.selected(), 10);
        press(&mut view, &[KeyCode::PageUp]);
        assert_eq!(view.selected(), 0);
        press(&mut view, &[KeyCode::End]);
        assert_eq!(view.selected(), 29);
        press(&mut view, &[KeyCode::Home]);
        assert_eq!(view.selected(), 0);
        // Moving past the end stays on the last line.
        press(&mut view, &[KeyCode::PageDown; 4]);
        assert_eq!(view.selected(), 29);
        press(&mut view, &[KeyCode::Down]);
        assert_eq!(view.selected(), 29);
    }

    #[test]
    fn panning_moves_eight_columns_up_to_32768() {
        let mut view = view(1);
        press(&mut view, &[KeyCode::Right, KeyCode::Right]);
        assert_eq!(view.horizontal, 16);
        press(&mut view, &[KeyCode::Left]);
        assert_eq!(view.horizontal, 8);
        press(&mut view, &[KeyCode::Left, KeyCode::Left]);
        assert_eq!(view.horizontal, 0);
        for _ in 0..5000 {
            handle_key(&mut view, key(KeyCode::Right));
        }
        assert_eq!(view.horizontal, 32768);
    }

    #[test]
    fn only_quit_keys_and_control_c_close_the_view() {
        let mut view = view(3);
        for code in [KeyCode::Char('q'), KeyCode::Esc, KeyCode::Enter] {
            assert!(handle_key(&mut view, key(code)), "{code:?}");
        }
        assert!(handle_key(
            &mut view,
            with(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
        assert!(!handle_key(&mut view, key(KeyCode::Char('c'))));
        assert!(!handle_key(
            &mut view,
            with(KeyCode::Char('x'), KeyModifiers::CONTROL)
        ));
        let mut release = key(KeyCode::Char('q'));
        release.kind = KeyEventKind::Release;
        release.state = KeyEventState::NONE;
        assert!(!handle_key(&mut view, release));
    }

    #[test]
    fn search_finds_edits_and_repeats_matches() {
        let lines = ["alpha", "beta", "Match one", "gamma", "match two", "delta"].join("\n");
        let mut view = ReportView::new("t", &lines, false).unwrap();
        press(&mut view, &[KeyCode::Char('/')]);
        assert!(view.searching);
        typed(&mut view, "matcx");
        assert_eq!(view.query, "matcx");
        // Backspace edits the query and searches again from the top.
        press(&mut view, &[KeyCode::Backspace]);
        assert_eq!(view.query, "matc");
        assert_eq!(view.selected(), 2);
        // Enter ends the search but keeps the query for `n`.
        press(&mut view, &[KeyCode::Enter]);
        assert!(!view.searching);
        assert_eq!(view.query, "matc");
        press(&mut view, &[KeyCode::Char('n')]);
        assert_eq!(view.selected(), 4);
        press(&mut view, &[KeyCode::Char('n')]);
        assert_eq!(view.selected(), 2);
        // Esc cancels a search and clears it.
        press(&mut view, &[KeyCode::Char('/')]);
        typed(&mut view, "x");
        press(&mut view, &[KeyCode::Esc]);
        assert!(!view.searching);
        assert!(view.query.is_empty());
    }

    #[test]
    fn queries_take_printable_characters_up_to_256_bytes() {
        let mut view = view(3);
        press(&mut view, &[KeyCode::Char('/')]);
        typed(&mut view, "a\u{7}b");
        assert_eq!(view.query, "ab");
        typed(&mut view, &"a".repeat(198));
        // A two-byte character fits at 200 bytes.
        typed(&mut view, "é");
        assert_eq!(view.query.len(), 202);
        typed(&mut view, &"a".repeat(53));
        assert_eq!(view.query.len(), 255);
        typed(&mut view, "é");
        assert_eq!(view.query.len(), 255);
        typed(&mut view, "aa");
        assert_eq!(view.query.len(), 256);
    }

    fn click(view: &mut ReportView, kind: MouseEventKind, row: u16) -> bool {
        handle_mouse(
            view,
            MouseEvent {
                kind,
                column: 5,
                row,
                modifiers: KeyModifiers::NONE,
            },
        )
    }

    #[test]
    fn the_wheel_scrolls_three_lines_and_clicks_select_rows() {
        let mut view = view(20);
        view.list_area = Rect::new(0, 2, 40, 10);
        assert!(!click(&mut view, MouseEventKind::ScrollDown, 0));
        assert_eq!(view.selected(), 3);
        click(&mut view, MouseEventKind::ScrollDown, 0);
        click(&mut view, MouseEventKind::ScrollUp, 0);
        assert_eq!(view.selected(), 3);
        let left = MouseEventKind::Down(MouseButton::Left);
        // Rows are counted from the top of the list, plus the scroll offset.
        *view.state.offset_mut() = 5;
        assert!(!click(&mut view, left, 4));
        assert_eq!(view.selected(), 7);
        assert!(click(&mut view, left, 4));
        // Outside the list nothing changes.
        assert!(!click(&mut view, left, 1));
        assert_eq!(view.selected(), 7);
        // The last line is selectable; the row after it is not.
        *view.state.offset_mut() = 10;
        assert!(!click(&mut view, left, 11));
        assert_eq!(view.selected(), 19);
        let mut short = self::view(3);
        short.list_area = Rect::new(0, 2, 40, 10);
        assert!(!click(&mut short, left, 5));
        assert_eq!(short.selected(), 0);
        assert!(!click(&mut short, MouseEventKind::Moved, 3));
    }

    fn texts(spans: &[Span<'_>]) -> Vec<String> {
        spans.iter().map(|span| span.content.to_string()).collect()
    }

    #[test]
    fn matches_are_highlighted_case_insensitively_without_overlap() {
        let theme = Theme::new(false);
        assert_eq!(texts(&matched("Hello", "", theme)), ["Hello"]);
        let spans = matched("say Hello, hello", "hello", theme);
        assert_eq!(texts(&spans), ["say ", "Hello", ", ", "hello", ""]);
        assert_eq!(spans[1].style, theme.highlight());
        assert_eq!(spans[0].style, Span::raw("").style);
        // Overlapping occurrences highlight only the first.
        assert_eq!(texts(&matched("aaa", "aa", theme)), ["", "aa", "a"]);
        assert_eq!(
            texts(&matched("xaaaay", "aa", theme)),
            ["x", "aa", "", "aa", "y"]
        );
        // A case fold that changes length leaves the line plain.
        assert_eq!(texts(&matched("İa", "a", theme)), ["İa"]);
    }

    #[test]
    fn the_footer_appears_from_four_rows_and_the_page_follows_the_panel() {
        let draw = |height: u16| {
            let mut view = view(30);
            let mut terminal =
                Terminal::new(ratatui::backend::TestBackend::new(40, height)).unwrap();
            terminal.draw(|frame| render(frame, &mut view)).unwrap();
            let buffer = terminal.backend().buffer().clone();
            let last: String = (0..40)
                .map(|x| buffer[(x, height - 1)].symbol().to_owned())
                .collect();
            (last, view.page, view.list_area)
        };
        let (last, _, _) = draw(4);
        assert!(last.contains("1/30"), "{last:?}");
        let (last, _, _) = draw(3);
        assert!(!last.contains("1/30"), "{last:?}");
        let (_, page, area) = draw(12);
        assert_eq!(page, usize::from(area.height));
        assert!(page > 1);
    }

    /// Run in a child process by the test below, with the variable already set.
    #[test]
    fn mouse_probe() {
        if let Some(expected) = std::env::var_os("RIGSPARK_TEST_MOUSE_EXPECT") {
            assert_eq!(mouse_enabled().to_string(), expected.to_string_lossy());
        }
    }

    #[test]
    fn mouse_capture_can_be_turned_off_with_a_non_empty_variable() {
        for (value, expected) in [(None, "true"), (Some(""), "true"), (Some("1"), "false")] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "tui_view::view_tests::mouse_probe",
                    "--test-threads=1",
                ])
                .env("RIGSPARK_TEST_MOUSE_EXPECT", expected)
                .env_remove("RIGSPARK_NO_MOUSE");
            if let Some(value) = value {
                child.env("RIGSPARK_NO_MOUSE", value);
            }
            let output = child.output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{value:?}: {stdout}");
            assert!(stdout.contains("1 passed"), "{value:?}: {stdout}");
        }
    }

    fn keys(events: Vec<KeyEvent>) -> Vec<io::Result<Event>> {
        events.into_iter().map(|key| Ok(Event::Key(key))).collect()
    }
    /// Events that arrive 10 ms apart, then a stream that stays open.
    async fn navigate(events: Vec<io::Result<Event>>) -> Event {
        let stream = futures_util::stream::iter(events)
            .then(|event| async {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                event
            })
            .chain(futures_util::stream::pending());
        let mut stream = Box::pin(stream);
        next_navigation_event(&mut stream).await.unwrap()
    }
    fn code(event: Event) -> KeyCode {
        match event {
            Event::Key(key) => key.code,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn escape_bracket_h_and_f_become_home_and_end() {
        let esc = key(KeyCode::Esc);
        let bracket = key(KeyCode::Char('['));
        assert_eq!(
            code(navigate(keys(vec![esc, bracket, key(KeyCode::Char('H'))])).await),
            KeyCode::Home
        );
        assert_eq!(
            code(navigate(keys(vec![esc, bracket, key(KeyCode::Char('F'))])).await),
            KeyCode::End
        );
        // Releases in between are skipped.
        let mut release = key(KeyCode::Char('x'));
        release.kind = KeyEventKind::Release;
        assert_eq!(
            code(navigate(keys(vec![esc, release, bracket, key(KeyCode::Char('H'))])).await),
            KeyCode::Home
        );
        // Anything else leaves the escape as it was.
        for tail in [
            vec![key(KeyCode::Char('x'))],
            vec![bracket, key(KeyCode::Char('x'))],
            vec![key(KeyCode::Char('H'))],
            vec![bracket, bracket],
            vec![
                with(KeyCode::Char('['), KeyModifiers::ALT),
                key(KeyCode::Char('H')),
            ],
            vec![
                with(KeyCode::Char('['), KeyModifiers::SUPER),
                key(KeyCode::Char('H')),
            ],
            vec![bracket, with(KeyCode::Char('H'), KeyModifiers::CONTROL)],
            vec![],
        ] {
            let mut events = vec![esc];
            events.extend(tail.iter().copied());
            assert_eq!(code(navigate(keys(events)).await), KeyCode::Esc, "{tail:?}");
        }
        // Control-C after an escape is delivered.
        let interrupt = with(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(
            navigate(keys(vec![esc, interrupt])).await,
            Event::Key(interrupt)
        );
    }

    #[tokio::test]
    async fn only_a_pressed_escape_waits_for_a_sequence() {
        // Other keys and events come straight back.
        assert_eq!(
            code(navigate(keys(vec![key(KeyCode::Char('['))])).await),
            KeyCode::Char('[')
        );
        assert_eq!(
            navigate(vec![Ok(Event::FocusGained)]).await,
            Event::FocusGained
        );
        let mut released = key(KeyCode::Esc);
        released.kind = KeyEventKind::Release;
        assert_eq!(
            navigate(keys(vec![
                released,
                key(KeyCode::Char('[')),
                key(KeyCode::Char('H'))
            ]))
            .await,
            Event::Key(released)
        );
        // Non-key events inside the window are skipped.
        assert_eq!(
            code(
                navigate(vec![
                    Ok(Event::Key(key(KeyCode::Esc))),
                    Ok(Event::FocusLost),
                    Ok(Event::Key(key(KeyCode::Char('[')))),
                    Ok(Event::Key(key(KeyCode::Char('H')))),
                ])
                .await
            ),
            KeyCode::Home
        );
        // An input stream that ends is an error.
        let mut ended = futures_util::stream::iter(Vec::<io::Result<Event>>::new());
        assert!(next_navigation_event(&mut ended).await.is_err());
    }
}
