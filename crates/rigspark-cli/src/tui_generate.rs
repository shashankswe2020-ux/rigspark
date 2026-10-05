//! Interactive terminal UI for local image/video generation through ComfyUI.
use crate::tui_theme::Theme;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use futures_util::{Stream, StreamExt};
use ratatui::{
    Frame, Terminal,
    backend::Backend,
    layout::{Constraint, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use rigspark_core::{
    generation::{GenerationCatalog, fit},
    reports::strip_control,
    sizing::Hardware,
};
use rigspark_runtime::generation::{
    Events, Generator, MAX_PROMPT_BYTES, MAX_SEED, NativeOptions, NativeOutcome,
};
use std::{
    collections::VecDeque,
    io,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const LOG_LINES: usize = 200;

#[derive(Debug, Clone)]
pub struct ModelRow {
    pub id: String,
    pub kind: &'static str,
    pub default: bool,
    pub params: String,
    pub weights_gib: f64,
    pub verdict: &'static str,
    pub reason: &'static str,
}

/// Rows for every bundled generation model with its memory-only fit on `hardware`.
pub fn model_rows(catalog: &GenerationCatalog, hardware: &Hardware) -> Vec<ModelRow> {
    let mut rows: Vec<_> = catalog
        .models
        .iter()
        .map(|model| {
            let memory = fit(model, hardware);
            ModelRow {
                id: model.id.clone(),
                kind: model.kind.name(),
                default: model.default,
                params: model.params.clone(),
                weights_gib: model.total_bytes() as f64 / 1073741824.0,
                verdict: memory.verdict.name(),
                reason: memory.reason,
            }
        })
        .collect();
    rows.sort_by(|left, right| {
        left.kind
            .cmp(right.kind)
            .then(right.default.cmp(&left.default))
            .then(left.id.cmp(&right.id))
    });
    rows
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Model,
    Prompt,
    Directory,
    Seed,
}
const FIELDS: [Field; 4] = [Field::Model, Field::Prompt, Field::Directory, Field::Seed];

#[derive(Debug)]
enum Status {
    Idle,
    Running(Instant),
    Done(Box<NativeOutcome>),
    Failed(String),
    Cancelled,
}

pub struct GenerateView {
    pub rows: Vec<ModelRow>,
    pub selected: usize,
    pub prompt: String,
    pub directory: String,
    pub seed: String,
    pub focus: Field,
    pub port: u16,
    pub bypass: bool,
    pub output_dir: PathBuf,
    log: VecDeque<String>,
    status: Status,
    error: Option<String>,
    pub saved: Vec<PathBuf>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Continue,
    Submit,
    Cancel,
    Exit(u8),
}

impl GenerateView {
    pub fn new(rows: Vec<ModelRow>, output_dir: PathBuf) -> Self {
        Self {
            rows,
            selected: 0,
            prompt: String::new(),
            directory: String::new(),
            seed: String::new(),
            focus: Field::Prompt,
            port: rigspark_runtime::generation::DEFAULT_PORT,
            bypass: false,
            output_dir,
            log: VecDeque::new(),
            status: Status::Idle,
            error: None,
            saved: Vec::new(),
        }
    }
    /// Preselects `image`, `video`, or an exact model id.
    pub fn select(&mut self, query: &str) {
        if let Some(index) = self
            .rows
            .iter()
            .position(|row| row.id == query)
            .or_else(|| {
                self.rows
                    .iter()
                    .position(|row| row.kind == query && row.default)
            })
        {
            self.selected = index;
        }
    }
    pub fn running(&self) -> bool {
        matches!(self.status, Status::Running(_))
    }
    pub fn status_text(&self) -> String {
        match &self.status {
            Status::Idle => "Ready".into(),
            Status::Running(started) => {
                format!("Generating… {:.0}s", started.elapsed().as_secs_f64())
            }
            Status::Done(outcome) => format!(
                "Saved {}: {} (seed {})",
                outcome.result.kind.name(),
                outcome.result.path.display(),
                outcome.result.seed
            ),
            Status::Failed(error) => format!("Failed: {error}"),
            Status::Cancelled => "Cancelled".into(),
        }
    }
    pub fn log_lines(&self) -> impl Iterator<Item = &String> {
        self.log.iter()
    }
    fn push_log(&mut self, line: String) {
        let line: String = strip_control(&line).chars().take(512).collect();
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }
    fn insert(&mut self, text: &str) {
        let (field, limit) = match self.focus {
            Field::Model => return,
            Field::Prompt => (&mut self.prompt, MAX_PROMPT_BYTES),
            Field::Directory => (&mut self.directory, 4096),
            Field::Seed => (&mut self.seed, 16),
        };
        for ch in text.chars() {
            let allowed = match self.focus {
                Field::Seed => ch.is_ascii_digit(),
                Field::Prompt => !ch.is_control() || ch == '\n',
                _ => !ch.is_control(),
            };
            if allowed && field.len() + ch.len_utf8() <= limit {
                field.push(ch);
            }
        }
    }
    /// Builds validated options or explains what is missing.
    pub fn options(&self) -> Result<NativeOptions, String> {
        let row = self
            .rows
            .get(self.selected)
            .ok_or("no generation model selected")?;
        if self.prompt.trim().is_empty() {
            return Err("enter a prompt".into());
        }
        if self.directory.trim().is_empty() {
            return Err("enter your ComfyUI directory".into());
        }
        let seed = if self.seed.is_empty() {
            None
        } else {
            Some(
                self.seed
                    .parse::<u64>()
                    .ok()
                    .filter(|seed| *seed <= MAX_SEED)
                    .ok_or("seed must be in 0..=9007199254740991")?,
            )
        };
        if row.verdict == "no" && !self.bypass {
            return Err(format!(
                "{} does not fit this machine ({}); restart with --bypass to try anyway",
                row.id, row.reason
            ));
        }
        Ok(NativeOptions {
            model: row.id.clone(),
            prompt: self.prompt.clone(),
            seed,
            comfyui_dir: PathBuf::from(self.directory.trim()),
            port: self.port,
            bypass: self.bypass,
            output: None,
            output_dir: self.output_dir.clone(),
        })
    }
}

pub fn handle_key(view: &mut GenerateView, key: KeyEvent) -> Action {
    if key.kind == KeyEventKind::Release {
        return Action::Continue;
    }
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    if control && key.code == KeyCode::Char('c') {
        return if view.running() {
            Action::Cancel
        } else {
            Action::Exit(130)
        };
    }
    if key.code == KeyCode::Esc {
        return if view.running() {
            Action::Cancel
        } else {
            Action::Exit(0)
        };
    }
    if view.running() {
        return Action::Continue;
    }
    let position = FIELDS
        .iter()
        .position(|field| *field == view.focus)
        .unwrap_or(0);
    match key.code {
        KeyCode::Tab => view.focus = FIELDS[(position + 1) % FIELDS.len()],
        KeyCode::BackTab => view.focus = FIELDS[(position + FIELDS.len() - 1) % FIELDS.len()],
        KeyCode::Up if view.focus == Field::Model => {
            view.selected = view.selected.saturating_sub(1);
        }
        KeyCode::Down if view.focus == Field::Model => {
            view.selected = (view.selected + 1).min(view.rows.len().saturating_sub(1));
        }
        KeyCode::Enter if control || key.modifiers.contains(KeyModifiers::ALT) => {
            view.insert("\n");
        }
        KeyCode::Char('j') if control => view.insert("\n"),
        KeyCode::Enter => return Action::Submit,
        KeyCode::Backspace => {
            let field = match view.focus {
                Field::Model => return Action::Continue,
                Field::Prompt => &mut view.prompt,
                Field::Directory => &mut view.directory,
                Field::Seed => &mut view.seed,
            };
            field.pop();
        }
        KeyCode::Char(ch) if !control => view.insert(&ch.to_string()),
        _ => (),
    }
    view.error = None;
    Action::Continue
}

fn verdict_badge(theme: Theme, verdict: &str) -> Span<'static> {
    Span::styled(format!(" {verdict:>4} "), theme.verdict(verdict))
}

pub fn render(frame: &mut Frame<'_>, view: &GenerateView, color: bool) {
    let theme = Theme::new(color);
    let area = frame.area();
    let model_height = (view.rows.len() as u16 + 2).min(8);
    let [header, models, prompt, settings, status, log, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(model_height),
        Constraint::Length(5),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(area);
    theme.header(
        frame,
        header,
        "rigspark / generate",
        "ComfyUI · local only · speed unknown",
    );
    let focused = |field: Field| {
        if view.focus == field && !view.running() {
            "▸ "
        } else {
            ""
        }
    };
    let inner = theme.panel(frame, models, &format!("{}Model", focused(Field::Model)));
    let rows: Vec<Line> = view
        .rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let marker = if index == view.selected {
                "● "
            } else {
                "○ "
            };
            let style = if index == view.selected {
                theme.selection()
            } else {
                Style::default()
            };
            Line::from(vec![
                Span::styled(marker, theme.accent()),
                Span::styled(format!("{:<20}", row.id), style),
                Span::styled(format!(" {:<6}", row.kind), theme.muted()),
                Span::styled(format!(" {:>5}", row.params), theme.muted()),
                Span::styled(format!(" {:>5.1} GiB ", row.weights_gib), theme.muted()),
                verdict_badge(theme, row.verdict),
                Span::styled(format!(" {}", row.reason), theme.muted()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), inner);
    let inner = theme.panel(frame, prompt, &format!("{}Prompt", focused(Field::Prompt)));
    let mut text = strip_control(&view.prompt.replace('\n', " ⏎ "));
    if view.focus == Field::Prompt && !view.running() {
        text.push('▌');
    }
    let prompt_line = if view.prompt.is_empty() && view.focus != Field::Prompt {
        Line::styled("Describe the image or video", theme.muted())
    } else {
        Line::raw(text)
    };
    frame.render_widget(
        Paragraph::new(prompt_line).wrap(Wrap { trim: false }),
        inner,
    );
    let [directory, seed] =
        Layout::horizontal([Constraint::Min(20), Constraint::Length(26)]).areas(settings);
    for (area, field, title, value, placeholder) in [
        (
            directory,
            Field::Directory,
            "ComfyUI directory",
            &view.directory,
            "path to your ComfyUI install",
        ),
        (seed, Field::Seed, "Seed", &view.seed, "random"),
    ] {
        let inner = theme.panel(frame, area, &format!("{}{title}", focused(field)));
        let mut value = strip_control(value);
        let line = if value.is_empty() && view.focus != field {
            Line::styled(placeholder, theme.muted())
        } else {
            if view.focus == field && !view.running() {
                value.push('▌');
            }
            let width = usize::from(inner.width.max(1));
            let skip = value.chars().count().saturating_sub(width);
            Line::raw(value.chars().skip(skip).collect::<String>())
        };
        frame.render_widget(Paragraph::new(line), inner);
    }
    let status_line = if let Some(error) = &view.error {
        Line::styled(format!(" {error}"), theme.error())
    } else {
        let style = match view.status {
            Status::Running(_) => theme.warning(),
            Status::Done(_) => theme.success(),
            Status::Failed(_) => theme.error(),
            _ => theme.muted(),
        };
        Line::styled(format!(" {}", strip_control(&view.status_text())), style)
    };
    theme.bar(
        frame,
        status,
        status_line,
        Line::styled(
            format!(
                "port {}{} ",
                view.port,
                if view.bypass { " · bypass" } else { "" }
            ),
            theme.muted(),
        ),
    );
    let inner = theme.panel(frame, log, "Progress");
    let visible = usize::from(inner.height);
    let lines: Vec<Line> = view
        .log
        .iter()
        .skip(view.log.len().saturating_sub(visible))
        .map(|line| Line::raw(line.clone()))
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
    let hints: &[(&str, &str)] = if view.running() {
        &[("Esc", "cancel"), ("Ctrl+C", "cancel")]
    } else {
        &[
            ("Enter", "generate"),
            ("Tab", "next field"),
            ("↑/↓", "model"),
            ("Ctrl+J", "newline"),
            ("Esc", "exit"),
        ]
    };
    frame.render_widget(Paragraph::new(theme.hints(hints, footer.width)), footer);
}

fn draw<B: Backend>(
    terminal: &mut Terminal<B>,
    view: &GenerateView,
    color: bool,
) -> io::Result<()> {
    terminal
        .draw(|frame| render(frame, view, color))
        .map(|_| ())
        .map_err(|error| io::Error::other(error.to_string()))
}

/// Runs the form loop; each submission drives `generator` while streaming progress.
pub async fn drive<B: Backend>(
    terminal: &mut Terminal<B>,
    events: &mut (impl Stream<Item = io::Result<Event>> + Unpin),
    generator: &dyn Generator,
    view: &mut GenerateView,
    color: bool,
) -> io::Result<u8> {
    loop {
        draw(terminal, view, color)?;
        let event = events
            .next()
            .await
            .ok_or_else(|| io::Error::other("terminal input ended"))??;
        let action = match event {
            Event::Key(key) => handle_key(view, key),
            Event::Paste(text) => {
                view.insert(&text);
                Action::Continue
            }
            _ => Action::Continue,
        };
        match action {
            Action::Exit(code) => return Ok(code),
            Action::Continue | Action::Cancel => continue,
            Action::Submit => (),
        }
        let options = match view.options() {
            Ok(options) => options,
            Err(error) => {
                view.error = Some(error);
                continue;
            }
        };
        view.log.clear();
        view.push_log(format!(
            "Starting {} with seed {}",
            options.model,
            options
                .seed
                .map_or_else(|| "random".to_owned(), |seed| seed.to_string())
        ));
        view.status = Status::Running(Instant::now());
        let (sender, mut progress) = tokio::sync::mpsc::unbounded_channel::<String>();
        let sink: Events = Arc::new(move |message| {
            let _ = sender.send(message);
        });
        let cancel = CancellationToken::new();
        let operation = generator.generate(options, sink, cancel.clone());
        tokio::pin!(operation);
        let mut exit_after = None;
        let result = loop {
            draw(terminal, view, color)?;
            tokio::select! {
                biased;
                result = &mut operation => break result,
                Some(message) = progress.recv() => view.push_log(message),
                () = tokio::time::sleep(Duration::from_millis(250)) => (),
                event = events.next() => {
                    let event = event.ok_or_else(|| io::Error::other("terminal input ended"))??;
                    if let Event::Key(key) = event {
                        match handle_key(view, key) {
                            Action::Cancel if !cancel.is_cancelled() => {
                                if key.modifiers.contains(KeyModifiers::CONTROL) {
                                    exit_after = Some(130);
                                }
                                view.push_log("Cancelling: removing the queued ComfyUI prompt…".into());
                                cancel.cancel();
                            }
                            Action::Exit(code) => exit_after = Some(code),
                            _ => (),
                        }
                    }
                },
            }
        };
        while let Ok(message) = progress.try_recv() {
            view.push_log(message);
        }
        view.status = match result {
            Ok(outcome) => {
                view.push_log(format!("Saved {}", outcome.result.path.display()));
                view.saved.push(outcome.result.path.clone());
                Status::Done(Box::new(outcome))
            }
            Err(_) if cancel.is_cancelled() => Status::Cancelled,
            Err(error) => Status::Failed(error.to_string()),
        };
        if let Some(code) = exit_after {
            return Ok(code);
        }
    }
}

pub async fn run(
    view: &mut GenerateView,
    generator: &dyn Generator,
    color: bool,
) -> io::Result<u8> {
    let mut signals = crate::cancellation::TerminalSignals::new()?;
    let (mut terminal, _restore) = crate::tui_view::enter_terminal()?;
    let mut events = crate::terminal_events::terminal_events();
    tokio::select! {
        result = drive(&mut terminal, &mut events, generator, view, color) => result,
        result = signals.recv() => result,
    }
}
