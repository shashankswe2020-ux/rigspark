use crate::{
    accessible_catalog::CatalogPresentation,
    accessible_installed::{InstalledCommand, InstalledView},
    accessible_read_only::can_run_screen,
    accessible_recommend::Recommendation,
    accessible_text::{identifier, single_line},
    tui_theme::{self, Theme, Verdict},
};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Cell, Clear, HighlightSpacing, Paragraph, Row, Table, TableState},
};
use rigspark_core::reports::strip_control;
use serde::Deserialize;
use serde_json::Value;
use std::io;
use unicode_segmentation::UnicodeSegmentation;

const MAX_ROWS: usize = 1000;
const MAX_TEXT: usize = 64 * 1024;
const MAX_DOCUMENT: usize = 4 * 1024 * 1024;
const MAX_QUERY: usize = 256;
const MAX_MARKED: usize = 4;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CanRunMetadata<'evidence> {
    model_id: &'evidence str,
    runnable: &'evidence str,
    quant: Option<&'evidence str>,
    reason: Option<&'evidence str>,
    throughput_backend: &'evidence str,
    context: Option<f64>,
}

fn bound_can_run(value: &Value, depth: usize, budget: &mut usize) -> io::Result<()> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "can-run view input limit exceeded",
        )
    };
    if depth > 16 {
        return Err(invalid());
    }
    *budget = budget.checked_sub(64).ok_or_else(invalid)?;
    match value {
        Value::String(text) => {
            if text.len() > MAX_TEXT {
                return Err(invalid());
            }
            *budget = budget.checked_sub(text.len()).ok_or_else(invalid)?;
        }
        Value::Array(values) => {
            if values.len() > MAX_ROWS {
                return Err(invalid());
            }
            for value in values {
                bound_can_run(value, depth + 1, budget)?;
            }
        }
        Value::Object(values) => {
            if values.len() > MAX_ROWS {
                return Err(invalid());
            }
            for (key, value) in values {
                if key.len() > MAX_TEXT {
                    return Err(invalid());
                }
                *budget = budget.checked_sub(key.len()).ok_or_else(invalid)?;
                bound_can_run(value, depth + 1, budget)?;
            }
        }
        _ => (),
    }
    Ok(())
}

#[derive(Debug)]
pub struct ModelRow {
    label: String,
    summary: String,
    search: String,
    evidence: String,
    need_bytes: Option<f64>,
    /// Release date, else auto-admission date (YYYY-MM-DD), for the month filter.
    recency: Option<String>,
}

fn bounded_text(value: &str, limit: usize) -> io::Result<String> {
    if value.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "model view text limit exceeded",
        ));
    }
    Ok(strip_control(value)
        .chars()
        .filter(|character| !character.is_control())
        .collect())
}

impl ModelRow {
    pub fn new(label: &str, search: &str, evidence: &str) -> io::Result<Self> {
        Ok(Self {
            label: bounded_text(label, 1024)?,
            summary: String::new(),
            search: bounded_text(search, MAX_TEXT)?.to_lowercase(),
            evidence: bounded_evidence(evidence)?,
            need_bytes: None,
            recency: None,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    fn cells(&self) -> Vec<&str> {
        if self.summary.is_empty() {
            Vec::new()
        } else {
            self.summary.split(" | ").collect()
        }
    }

    fn verdict(&self) -> Option<Verdict> {
        self.cells().into_iter().find_map(tui_theme::verdict_kind)
    }
}

fn bounded_evidence(value: &str) -> io::Result<String> {
    if value.len() > MAX_TEXT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "model view text limit exceeded",
        ));
    }
    value
        .lines()
        .map(|line| bounded_text(line, MAX_TEXT))
        .collect::<io::Result<Vec<_>>>()
        .map(|lines| lines.join("\n"))
}

#[derive(Debug, PartialEq, Eq)]
pub enum ModelOutcome {
    Exit { code: u8 },
    PrintCommand { command: String },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Detail,
    Overview,
    Help,
    Compare,
}

#[derive(Default)]
struct HitAreas {
    rows: Rect,
    header: Vec<(Rect, usize)>,
    detail: Rect,
}

pub struct ModelView {
    title: String,
    overview: Vec<String>,
    rows: Vec<ModelRow>,
    visible: Vec<usize>,
    matches: Vec<Vec<usize>>,
    list: TableState,
    query: String,
    searching: bool,
    focus: Focus,
    help_return: Option<(Focus, usize, usize)>,
    marked: Vec<usize>,
    notice: Option<&'static str>,
    toast: Option<String>,
    clipboard: Option<String>,
    scroll: usize,
    scroll_max: usize,
    page: usize,
    detail_page: usize,
    color: bool,
    command: Option<String>,
    comparison: bool,
    columns: Vec<&'static str>,
    priority: Vec<usize>,
    sort: Option<(usize, bool)>,
    verdict_filter: Option<Verdict>,
    /// Earliest recency date shown for the 1, 2 and 3 month windows.
    month_cutoffs: Option<[String; 3]>,
    month_filter: Option<u8>,
    usable_bytes: Option<f64>,
    areas: HitAreas,
}

impl ModelView {
    pub fn new(
        title: &str,
        overview: Vec<String>,
        rows: Vec<ModelRow>,
        color: bool,
    ) -> io::Result<Self> {
        if rows.len() > MAX_ROWS || overview.len() > MAX_ROWS + 8 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "model view row limit exceeded",
            ));
        }
        let bytes = rows
            .iter()
            .map(|row| row.label.len() + row.summary.len() + row.search.len() + row.evidence.len())
            .sum::<usize>()
            + overview.iter().map(String::len).sum::<usize>();
        if bytes > MAX_DOCUMENT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "model view document limit exceeded",
            ));
        }
        let overview = overview
            .iter()
            .map(|line| bounded_text(line, MAX_TEXT))
            .collect::<io::Result<_>>()?;
        let selection = if rows.is_empty() { None } else { Some(0) };
        Ok(Self {
            title: bounded_text(title, 256)?,
            overview,
            visible: (0..rows.len()).collect(),
            matches: vec![Vec::new(); rows.len()],
            rows,
            list: TableState::default().with_selected(selection),
            query: String::new(),
            searching: false,
            focus: Focus::List,
            help_return: None,
            marked: Vec::new(),
            notice: None,
            toast: None,
            clipboard: None,
            scroll: 0,
            scroll_max: 0,
            page: 1,
            detail_page: 1,
            color,
            command: None,
            comparison: true,
            columns: Vec::new(),
            priority: Vec::new(),
            sort: None,
            verdict_filter: None,
            month_cutoffs: None,
            month_filter: None,
            usable_bytes: None,
            areas: HitAreas::default(),
        })
    }

    fn with_columns(mut self, columns: &[&'static str], priority: &[usize]) -> Self {
        self.columns = columns.to_vec();
        self.priority = priority.to_vec();
        self
    }

    fn with_memory(mut self, usable: f64, needs: impl Iterator<Item = f64>) -> Self {
        self.usable_bytes = (usable.is_finite() && usable > 0.0).then_some(usable);
        for (row, need) in self.rows.iter_mut().zip(needs) {
            row.need_bytes = (need.is_finite() && need >= 0.0).then_some(need);
        }
        self
    }

    /// Enables the `m` month filter using each model's release or auto-admission date.
    pub fn with_recency(
        mut self,
        dates: &std::collections::BTreeMap<String, String>,
        today: &str,
    ) -> io::Result<Self> {
        let cutoff = |months| {
            rigspark_core::catalog::months_before(today, months)
                .map(|day| day.to_string())
                .map_err(io::Error::other)
        };
        self.month_cutoffs = Some([cutoff(1)?, cutoff(2)?, cutoff(3)?]);
        for row in &mut self.rows {
            row.recency = dates.get(&row.label).cloned();
        }
        Ok(self)
    }

    pub fn from_catalog(presentation: &CatalogPresentation, color: bool) -> io::Result<Self> {
        Ok(Self::new(
            "Catalog",
            presentation.visual_overview(),
            adapt_rows(presentation.visual_rows())?,
            color,
        )?
        .with_columns(&["Quant", "Need", "Fit", "Release"], &[2, 1, 0, 3])
        .with_memory(presentation.usable_bytes(), presentation.need_bytes()))
    }

    pub fn from_recommendation(presentation: &Recommendation, color: bool) -> io::Result<Self> {
        let mut view = Self::new(
            "Recommend",
            presentation.visual_overview(),
            adapt_rows(presentation.visual_rows())?,
            color,
        )?
        .with_columns(
            &["Rank", "Quant", "Need", "Verdict", "Tok/s", "Score"],
            &[3, 4, 2, 0, 5, 1],
        )
        .with_memory(presentation.usable_bytes(), presentation.need_bytes());
        view.command = presentation.print_command().map(str::to_owned);
        Ok(view)
    }

    pub fn from_installed(presentation: &InstalledView, color: bool) -> io::Result<Self> {
        let title = match presentation.command() {
            InstalledCommand::Recommend => "Recommend / Installed",
            InstalledCommand::CanRun => "Can Run / Installed",
        };
        let rows = presentation
            .visual_rows()
            .map(|(label, fit, search, evidence)| {
                let mut row = ModelRow::new(label, search, evidence)?;
                row.summary = bounded_text(fit, 64)?;
                Ok(row)
            })
            .collect::<io::Result<Vec<_>>>()?;
        let mut view = Self::new(title, presentation.visual_overview(), rows, color)?
            .with_columns(&["Fit"], &[0]);
        view.comparison =
            presentation.command() == InstalledCommand::Recommend && view.rows.len() >= 2;
        if view.rows.is_empty() {
            view.focus(Focus::Overview);
        } else if presentation.command() == InstalledCommand::CanRun {
            view.focus(Focus::Detail);
        }
        Ok(view)
    }

    pub fn from_can_run(evidence: &Value, color: bool) -> io::Result<Self> {
        let mut budget = MAX_DOCUMENT;
        bound_can_run(evidence, 0, &mut budget)?;
        let screen = can_run_screen(evidence)?;
        let metadata = CanRunMetadata::deserialize(evidence).map_err(io::Error::other)?;
        if metadata.model_id.len() > 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "model view text limit exceeded",
            ));
        }
        let label = identifier(metadata.model_id)?;
        let summary = single_line(&format!(
            "Verdict: {}; quant {}; reason {}",
            metadata.runnable,
            metadata.quant.unwrap_or("unknown"),
            metadata.reason.unwrap_or("none"),
        ))?;
        let search = single_line(&format!(
            "{} {} {}",
            metadata.model_id, summary, metadata.throughput_backend,
        ))?;
        let mut row = ModelRow::new(&label, &search, &screen)?;
        row.summary = bounded_text(&summary, 2048)?;
        let context = metadata.context.map_or_else(
            || "Requested context: not specified".into(),
            |context| format!("Requested context: {context} tokens"),
        );
        let mut view = Self::new(
            "Can Run",
            vec![
                format!("Verdict: {} / offline evidence", metadata.runnable),
                context,
            ],
            vec![row],
            color,
        )?;
        view.comparison = false;
        view.focus(Focus::Detail);
        Ok(view)
    }

    pub fn selected(&self) -> Option<&ModelRow> {
        self.list
            .selected()
            .and_then(|index| self.visible.get(index))
            .map(|index| &self.rows[*index])
    }

    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }

    pub fn detail_focused(&self) -> bool {
        self.focus == Focus::Detail
    }

    fn filter(&mut self) {
        let selected = self
            .list
            .selected()
            .and_then(|index| self.visible.get(index))
            .copied();
        let needle = self.query.to_lowercase();
        let mut found: Vec<(usize, usize, Vec<usize>)> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                self.verdict_filter
                    .is_none_or(|wanted| row.verdict() == Some(wanted))
            })
            .filter(|(_, row)| {
                let cutoff = self
                    .month_filter
                    .zip(self.month_cutoffs.as_ref())
                    .map(|(months, cutoffs)| cutoffs[usize::from(months) - 1].as_str());
                cutoff.is_none_or(|cutoff| row.recency.as_deref().is_some_and(|day| day >= cutoff))
            })
            .filter_map(|(index, row)| {
                if let Some(positions) = tui_theme::fuzzy(&row.label, &needle) {
                    Some((index, tui_theme::fuzzy_rank(&positions), positions))
                } else {
                    row.search
                        .contains(&needle)
                        .then_some((index, usize::MAX, Vec::new()))
                }
            })
            .collect();
        if let Some((column, descending)) = self.sort {
            found.sort_by(|left, right| {
                let order = sort_key(&self.rows[left.0], column)
                    .partial_cmp(&sort_key(&self.rows[right.0], column))
                    .unwrap_or(std::cmp::Ordering::Equal);
                let unknown = |row: usize| sort_key(&self.rows[row], column).0;
                match (unknown(left.0), unknown(right.0)) {
                    (0, 0) if descending => order.reverse(),
                    _ => order,
                }
                .then(left.0.cmp(&right.0))
            });
        } else if !needle.is_empty() {
            found.sort_by_key(|(index, rank, _)| (*rank, *index));
        }
        self.visible = found.iter().map(|(index, _, _)| *index).collect();
        self.matches = found
            .into_iter()
            .map(|(_, _, positions)| positions)
            .collect();
        let position = selected
            .and_then(|selected| self.visible.iter().position(|index| *index == selected))
            .or_else(|| (!self.visible.is_empty()).then_some(0));
        self.list.select(position);
        *self.list.offset_mut() = 0;
        self.scroll = 0;
    }

    fn cycle_sort(&mut self, column: Option<usize>) {
        let columns = self.columns.len() + 1;
        self.sort = match (column, self.sort) {
            (Some(column), Some((current, descending))) if column == current => {
                Some((column, !descending))
            }
            (Some(column), _) => Some((column, false)),
            (None, None) => Some((0, false)),
            (None, Some((current, _))) if current + 1 < columns => Some((current + 1, false)),
            (None, Some(_)) => None,
        };
        self.filter();
    }

    fn selected_index(&self) -> Option<usize> {
        self.list
            .selected()
            .and_then(|index| self.visible.get(index))
            .copied()
    }

    /// Text queued for the terminal clipboard by `y`, `Y`, `e`, or chat copy.
    pub fn take_clipboard(&mut self) -> Option<String> {
        self.clipboard.take()
    }

    pub fn toast(&self) -> Option<&str> {
        self.toast.as_deref()
    }

    fn copy(&mut self, text: String, what: &str) {
        self.toast = Some(format!("Copied {what} to clipboard"));
        self.clipboard = Some(text);
    }

    /// Paste-ready 72-column ASCII summary of the selected model.
    fn card(&self, row: &ModelRow) -> String {
        const WIDTH: usize = 72;
        let mut card = format!(
            "rigspark / {} / {}\n{}\n",
            self.title,
            row.label,
            "=".repeat(WIDTH)
        );
        let mut field = |key: &str, value: &str| {
            for (index, line) in tui_theme::wrap(value, WIDTH - 10).into_iter().enumerate() {
                let key = if index == 0 { key } else { "" };
                card.push_str(&format!("{key:<9} {line}\n"));
            }
        };
        for (header, cell) in self.columns.iter().zip(row.cells()) {
            field(&header.to_uppercase(), cell);
        }
        if let Some(usable) = self.usable_bytes {
            field(
                "MEMORY",
                &row.need_bytes.map_or_else(
                    || "need unknown".into(),
                    |need| {
                        format!(
                            "{:.1} / {:.1} GiB ({:.0}%)",
                            need / 1_073_741_824.0,
                            usable / 1_073_741_824.0,
                            need / usable * 100.0
                        )
                    },
                ),
            );
        }
        for line in self.overview.iter().take(2) {
            field("MACHINE", line);
        }
        let mut entries = Vec::new();
        evidence_entries(row, &mut entries);
        for entry in entries.iter().take(24) {
            if let Entry::Field { key, value } = entry {
                field(
                    &key.to_uppercase().chars().take(9).collect::<String>(),
                    value,
                );
            }
        }
        card.push_str(&format!(
            "{}\nrigspark: offline, cited estimate\n",
            "-".repeat(WIDTH)
        ));
        card
    }

    fn move_to(&mut self, position: usize) {
        if self.focus != Focus::List {
            self.scroll = position.min(self.scroll_max);
        } else if !self.visible.is_empty() {
            self.list.select(Some(position.min(self.visible.len() - 1)));
            self.scroll = 0;
        }
    }

    fn focus(&mut self, focus: Focus) {
        self.focus = focus;
        self.scroll = 0;
        self.scroll_max = 0;
    }

    fn append_query(&mut self, text: &str) {
        let available = MAX_QUERY.saturating_sub(self.query.len());
        let mut bytes = 0;
        let prefix: String = text
            .chars()
            .take_while(|character| {
                bytes += character.len_utf8();
                bytes <= available
            })
            .collect();
        let clean = strip_control(&prefix);
        self.query
            .extend(clean.chars().filter(|character| !character.is_control()));
        self.filter();
    }

    fn toggle_help(&mut self) {
        if let Some((focus, scroll, scroll_max)) = self.help_return.take() {
            self.focus = focus;
            self.scroll = scroll;
            self.scroll_max = scroll_max;
        } else {
            self.help_return = Some((self.focus, self.scroll, self.scroll_max));
            self.focus(Focus::Help);
        }
    }

    fn toggle_mark(&mut self) {
        let Some(index) = self
            .list
            .selected()
            .and_then(|index| self.visible.get(index))
            .copied()
        else {
            self.notice = Some("No model selected to mark");
            return;
        };
        if let Some(position) = self.marked.iter().position(|marked| *marked == index) {
            self.marked.remove(position);
            self.notice = None;
        } else if self.marked.len() == MAX_MARKED {
            self.notice = Some("Mark limit 4: unmark a model with Space");
        } else {
            self.marked.push(index);
            self.notice = None;
        }
    }
}

fn adapt_rows<'row>(
    rows: impl Iterator<Item = (&'row str, &'row str, &'row str, &'row str)>,
) -> io::Result<Vec<ModelRow>> {
    rows.map(|(label, summary, search, evidence)| {
        let mut row = ModelRow::new(label, search, evidence)?;
        row.summary = bounded_text(summary, 2048)?;
        Ok(row)
    })
    .collect()
}

pub fn handle_key(view: &mut ModelView, key: KeyEvent) -> Option<ModelOutcome> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    view.toast = None;
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => return Some(ModelOutcome::Exit { code: 130 }),
            KeyCode::Char('u') if view.focus != Focus::Help => {
                view.query.clear();
                view.verdict_filter = None;
                view.month_filter = None;
                view.filter();
            }
            _ => (),
        }
        return None;
    }
    if key
        .modifiers
        .intersects(KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        return None;
    }
    if view.searching {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => view.searching = false,
            KeyCode::Backspace | KeyCode::Delete => {
                if let Some((index, _)) = view.query.grapheme_indices(true).next_back() {
                    view.query.truncate(index);
                }
                view.filter();
            }
            KeyCode::Char(character) if !character.is_control() => {
                view.append_query(&character.to_string())
            }
            _ => (),
        }
        return None;
    }
    if view.focus == Focus::Help {
        match key.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => {
                view.toggle_help();
                return None;
            }
            KeyCode::Char('?' | 'q' | 'j' | 'k' | 'p')
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::PageUp
            | KeyCode::PageDown
            | KeyCode::Home
            | KeyCode::End => (),
            _ => return None,
        }
    }
    let position = if view.focus == Focus::List {
        view.list.selected().unwrap_or(0)
    } else {
        view.scroll
    };
    let page = if view.focus == Focus::List {
        view.page
    } else {
        view.detail_page
    };
    match key.code {
        KeyCode::Char('q') => return Some(ModelOutcome::Exit { code: 0 }),
        KeyCode::Char('?') if key.kind == KeyEventKind::Press => view.toggle_help(),
        KeyCode::Char(' ')
            if key.kind == KeyEventKind::Press
                && view.comparison
                && matches!(view.focus, Focus::List | Focus::Detail) =>
        {
            view.toggle_mark()
        }
        KeyCode::Char('c') if key.kind == KeyEventKind::Press && view.comparison => {
            if view.focus == Focus::Compare {
                view.focus(Focus::List);
            } else if view.marked.len() < 2 {
                view.notice = Some("Mark at least 2 models with Space to compare (max 4)");
            } else {
                view.notice = None;
                view.focus(Focus::Compare);
            }
        }
        KeyCode::Esc if view.focus == Focus::List => return Some(ModelOutcome::Exit { code: 0 }),
        KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => view.focus(Focus::List),
        KeyCode::Enter | KeyCode::Tab | KeyCode::BackTab => {
            if view.focus == Focus::List && view.selected().is_some() {
                view.focus(Focus::Detail);
            } else {
                view.focus(Focus::List);
            }
        }
        KeyCode::Right if view.selected().is_some() => view.focus(Focus::Detail),
        KeyCode::Char('i') => view.focus(if view.focus == Focus::Overview {
            Focus::List
        } else {
            Focus::Overview
        }),
        KeyCode::Char('/') => {
            view.searching = true;
            view.focus(Focus::List);
        }
        KeyCode::Down | KeyCode::Char('j') => view.move_to(position.saturating_add(1)),
        KeyCode::Up | KeyCode::Char('k') => view.move_to(position.saturating_sub(1)),
        KeyCode::PageDown => view.move_to(position.saturating_add(page)),
        KeyCode::PageUp => view.move_to(position.saturating_sub(page)),
        KeyCode::Home => view.move_to(0),
        KeyCode::End => view.move_to(usize::MAX),
        KeyCode::Char('p') if key.kind == KeyEventKind::Press => {
            return view
                .command
                .as_ref()
                .map(|command| ModelOutcome::PrintCommand {
                    command: command.clone(),
                });
        }
        KeyCode::Char('s') if view.focus == Focus::List => view.cycle_sort(None),
        KeyCode::Char('S') if view.focus == Focus::List => {
            if let Some((column, descending)) = view.sort {
                view.sort = Some((column, !descending));
                view.filter();
            }
        }
        KeyCode::Char('m') if view.focus == Focus::List && view.month_cutoffs.is_some() => {
            view.month_filter = match view.month_filter {
                None => Some(1),
                Some(months) if months < 3 => Some(months + 1),
                Some(_) => None,
            };
            view.filter();
        }
        KeyCode::Char('v') if view.focus == Focus::List && !view.columns.is_empty() => {
            view.verdict_filter = match view.verdict_filter {
                None => Some(Verdict::Yes),
                Some(Verdict::Yes) => Some(Verdict::Slow),
                Some(Verdict::Slow) => Some(Verdict::No),
                Some(Verdict::No) => Some(Verdict::Unknown),
                Some(Verdict::Unknown) => None,
            };
            view.filter();
        }
        KeyCode::Char('y') => {
            if let Some(index) = view.selected_index() {
                let label = view.rows[index].label.clone();
                view.copy(label, "model id");
            }
        }
        KeyCode::Char('Y') => {
            if let Some(index) = view.selected_index() {
                let label = view.rows[index].label.clone();
                if command_safe(&label) {
                    view.copy(format!("rigspark up {label}"), "up command");
                } else {
                    view.notice = Some("Model id is not safe to copy as a command");
                }
            }
        }
        KeyCode::Char('e') => {
            if let Some(index) = view.selected_index() {
                let card = view.card(&view.rows[index]);
                view.copy(card, "shareable card");
            }
        }
        _ => (),
    }
    None
}

fn command_safe(id: &str) -> bool {
    !id.is_empty()
        && !id.starts_with('-')
        && !id.split('/').any(|part| part == "..")
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._:/-".contains(&byte)
        })
}

/// Unknowns sort last in both directions; numbers compare numerically, then text.
fn sort_key(row: &ModelRow, column: usize) -> (u8, f64, String) {
    let cells = row.cells();
    let text = if column == 0 {
        row.label.as_str()
    } else {
        cells.get(column - 1).copied().unwrap_or("")
    };
    if column > 0 {
        match tui_theme::verdict_kind(text) {
            Some(Verdict::Unknown) => return (1, 0.0, String::new()),
            Some(verdict) => return (0, f64::from(verdict as u8), String::new()),
            None => (),
        }
    }
    let lower = text.to_lowercase();
    if text.is_empty() || lower.starts_with("unknown") {
        return (1, 0.0, lower);
    }
    let number = if column == 0 {
        0.0
    } else {
        let digits: String = text
            .chars()
            .skip_while(|character| !character.is_ascii_digit())
            .take_while(|character| character.is_ascii_digit() || *character == '.')
            .collect();
        digits.parse().unwrap_or(0.0)
    };
    (0, number, lower)
}

/// Wheel scrolls the pane under the pointer; clicks select rows, open the selected row, or sort by a header.
pub fn handle_mouse(view: &mut ModelView, mouse: MouseEvent) {
    let point = Position::new(mouse.column, mouse.row);
    view.toast = None;
    match mouse.kind {
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            let down = mouse.kind == MouseEventKind::ScrollDown;
            if view.focus == Focus::List && !view.areas.detail.contains(point) {
                let position = view.list.selected().unwrap_or(0);
                view.move_to(if down {
                    position.saturating_add(1)
                } else {
                    position.saturating_sub(1)
                });
            } else if view.focus != Focus::List {
                view.scroll = if down {
                    view.scroll.saturating_add(3).min(view.scroll_max)
                } else {
                    view.scroll.saturating_sub(3)
                };
            }
        }
        MouseEventKind::Down(MouseButton::Left) if view.focus == Focus::List => {
            if let Some((_, column)) = view
                .areas
                .header
                .iter()
                .find(|(area, _)| area.contains(point))
            {
                let column = *column;
                view.cycle_sort(Some(column));
            } else if view.areas.rows.contains(point) {
                let row = view.list.offset() + usize::from(point.y - view.areas.rows.y);
                if row < view.visible.len() {
                    if view.list.selected() == Some(row) {
                        view.focus(Focus::Detail);
                    } else {
                        view.list.select(Some(row));
                    }
                }
            }
        }
        _ => (),
    }
}

pub fn handle_event(view: &mut ModelView, event: Event) -> Option<ModelOutcome> {
    match event {
        Event::Key(key) => handle_key(view, key),
        Event::Mouse(mouse) => {
            handle_mouse(view, mouse);
            None
        }
        Event::Paste(text) if view.searching => {
            view.append_query(&text);
            None
        }
        _ => None,
    }
}

/// Evidence labels emitted by the presentation builders, longest prefixes first.
const EVIDENCE_KEYS: [&str; 24] = [
    "throughput backend",
    "unknown reason",
    "KV bytes/token",
    "selected quant",
    "quantizations",
    "capabilities",
    "architecture",
    "open weight",
    "throughput",
    "benchmark",
    "backends",
    "release",
    "license",
    "context",
    "verdict",
    "sources",
    "family",
    "params",
    "scores",
    "source",
    "score",
    "rank",
    "need",
    "fit",
];

enum Entry {
    Heading(String),
    Note(String),
    Plain(String),
    Field { key: String, value: String },
    Blank,
}

fn split_key(segment: &str) -> Entry {
    EVIDENCE_KEYS
        .iter()
        .find_map(|key| {
            segment
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix(' '))
                .map(|value| Entry::Field {
                    key: (*key).to_owned(),
                    value: value.to_owned(),
                })
        })
        .unwrap_or_else(|| Entry::Field {
            key: String::new(),
            value: segment.to_owned(),
        })
}

fn evidence_entries(row: &ModelRow, entries: &mut Vec<Entry>) {
    for (index, segment) in row
        .evidence
        .lines()
        .flat_map(|line| line.split("; "))
        .enumerate()
    {
        entries.push(if index == 0 && segment == row.label {
            Entry::Heading(segment.to_owned())
        } else {
            split_key(segment)
        });
    }
}

/// Lays out entries with right-aligned keys so every `key value` stays contiguous.
fn layout(entries: &[Entry], width: u16, theme: Theme, key_style: Style) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    let key_width = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Field { key, .. } => Some(tui_theme::width(key)),
            _ => None,
        })
        .max()
        .unwrap_or(0)
        .min(width / 3);
    let aligned = width >= 30;
    let mut lines = Vec::new();
    for entry in entries {
        match entry {
            Entry::Heading(text) => lines.extend(
                tui_theme::wrap(text, width)
                    .into_iter()
                    .map(|line| Line::styled(line, theme.title())),
            ),
            Entry::Note(text) => lines.extend(
                tui_theme::wrap(text, width)
                    .into_iter()
                    .map(|line| Line::styled(line, theme.muted())),
            ),
            Entry::Plain(text) => {
                lines.extend(tui_theme::wrap(text, width).into_iter().map(Line::from))
            }
            Entry::Blank => lines.push(Line::default()),
            Entry::Field { key, value } if !aligned => {
                let text = if key.is_empty() {
                    value.clone()
                } else {
                    format!("{key} {value}")
                };
                lines.extend(tui_theme::wrap(&text, width).into_iter().map(Line::from));
            }
            Entry::Field { key, value } => {
                let key_cells = tui_theme::width(key);
                let indent = key_width.max(key_cells) + 1;
                let value_style = match key.as_str() {
                    "verdict" | "fit" => theme.verdict(value),
                    _ => Style::default(),
                };
                let items: Vec<&str> = if matches!(key.as_str(), "sources" | "quantizations") {
                    value.split(", ").collect()
                } else {
                    vec![value.as_str()]
                };
                let mut first = true;
                for item in items {
                    for piece in tui_theme::wrap(item, width.saturating_sub(indent)) {
                        let mut spans = Vec::with_capacity(4);
                        if first && !key.is_empty() {
                            spans.push(Span::raw(" ".repeat(key_width.saturating_sub(key_cells))));
                            spans.push(Span::styled(key.clone(), key_style));
                            spans.push(Span::raw(" "));
                        } else {
                            spans.push(Span::raw(" ".repeat(indent)));
                        }
                        spans.push(Span::styled(piece, value_style));
                        lines.push(Line::from(spans));
                        first = false;
                    }
                }
            }
        }
    }
    lines
}

fn render_detail(frame: &mut Frame<'_>, view: &mut ModelView, area: Rect, theme: Theme) {
    let title = match view.focus {
        Focus::Overview => "Machine / scope".to_owned(),
        Focus::Help => "Keyboard help".to_owned(),
        Focus::Compare => format!("Compare {} models", view.marked.len()),
        _ => "Evidence".to_owned(),
    };
    let inner = theme.panel(frame, area, &title);
    let lines = if view.focus == Focus::Help {
        let mut help = vec![
            "Up/Down or j/k: navigate list or scroll screen",
            "PageUp/PageDown: page; Home/End: first/last",
            "/: search; Enter/Esc: close search; Ctrl+U: reset filter",
            "Enter/Right/Tab: details; Left/Backspace/Esc: back",
            "i: machine/scope; ?: toggle help",
            "s: sort by next column (Model, then each column, then original); S: reverse",
            "y: copy model id; Y: copy `rigspark up` command; e: copy shareable card",
            "Mouse: wheel scrolls; click selects, click again opens; click a header sorts",
            "Esc: back from screen; Esc on list or q: quit",
            "Ctrl+C: interrupt (exit 130)",
            "Read-only: stored evidence only; no actions execute",
        ];
        if view.comparison {
            help.extend([
                "Space: mark/unmark selected model (max 4)",
                "c: compare 2-4 marked models / return to list",
                "Marks survive filtering; * identifies a marked model",
            ]);
        }
        if !view.columns.is_empty() {
            help.push("v: cycle verdict filter (yes, slow, no, unknown, all)");
        }
        if view.month_cutoffs.is_some() {
            help.push("m: cycle recency filter (released or added in 1, 2, 3 months, all)");
        }
        if view.command.is_some() {
            help.push("p: finish and print existing top-pick command; never execute");
        }
        let entries: Vec<_> = help
            .into_iter()
            .map(|line| match line.split_once(": ") {
                Some((key, value)) => Entry::Field {
                    key: format!("{key}:"),
                    value: value.to_owned(),
                },
                None => Entry::Plain(line.to_owned()),
            })
            .collect();
        layout(&entries, inner.width, theme, theme.title())
    } else if view.focus == Focus::Compare {
        compare_lines(view, inner.width, theme).unwrap_or_else(|| {
            let mut entries = Vec::new();
            for index in &view.marked {
                let row = &view.rows[*index];
                if !entries.is_empty() {
                    entries.push(Entry::Blank);
                }
                entries.push(Entry::Heading(row.label.clone()));
                if !row.summary.is_empty() {
                    entries.push(Entry::Note(row.summary.clone()));
                }
                let mut evidence = Vec::new();
                evidence_entries(row, &mut evidence);
                if matches!(evidence.first(), Some(Entry::Heading(_))) {
                    evidence.remove(0);
                }
                entries.extend(evidence);
            }
            layout(&entries, inner.width, theme, theme.muted())
        })
    } else if view.focus == Focus::Overview {
        let entries: Vec<_> = view
            .overview
            .iter()
            .map(|line| Entry::Plain(line.clone()))
            .collect();
        layout(&entries, inner.width, theme, theme.muted())
    } else if let Some(row) = view.selected() {
        let mut entries = Vec::new();
        evidence_entries(row, &mut entries);
        let heading = matches!(entries.first(), Some(Entry::Heading(_)));
        if heading {
            entries.insert(1, Entry::Blank);
        }
        let mut lines = layout(&entries, inner.width, theme, theme.muted());
        if let Some(usable) = view.usable_bytes {
            let gauge = match row.need_bytes {
                Some(need) => tui_theme::gauge(
                    theme,
                    need,
                    usable,
                    usize::from(inner.width).saturating_sub(8),
                ),
                None => Line::styled("need unknown", theme.muted()),
            };
            let mut spans = vec![Span::styled("Memory  ", theme.muted())];
            spans.extend(gauge.spans);
            lines.insert(usize::from(heading), Line::from(spans));
        }
        lines
    } else {
        vec![Line::styled("No model selected", theme.muted())]
    };
    view.detail_page = usize::from(inner.height.max(1));
    view.scroll_max = lines.len().saturating_sub(view.detail_page);
    view.scroll = view.scroll.min(view.scroll_max);
    let total = lines.len();
    let visible: Vec<_> = lines
        .into_iter()
        .skip(view.scroll)
        .take(view.detail_page)
        .collect();
    frame.render_widget(Paragraph::new(visible), inner);
    tui_theme::scrollbar(frame, area, total, view.scroll, view.detail_page, theme);
}

/// Side-by-side comparison aligned by evidence key; `None` when columns would be too narrow.
fn compare_lines(view: &ModelView, width: u16, theme: Theme) -> Option<Vec<Line<'static>>> {
    let width = usize::from(width);
    let count = view.marked.len();
    if count == 0 {
        return None;
    }
    let mut order: Vec<String> = Vec::new();
    let mut tables: Vec<Vec<(String, String)>> = Vec::new();
    for index in &view.marked {
        let row = &view.rows[*index];
        let mut entries = Vec::new();
        evidence_entries(row, &mut entries);
        let mut unkeyed = 0;
        let mut fields = Vec::new();
        for entry in entries {
            if let Entry::Field { key, value } = entry {
                let id = if key.is_empty() {
                    unkeyed += 1;
                    format!("\u{0}{unkeyed}")
                } else {
                    key
                };
                if !order.contains(&id) {
                    order.push(id.clone());
                }
                fields.push((id, value));
            }
        }
        tables.push(fields);
    }
    let display = |id: &str| {
        if id.starts_with('\u{0}') {
            String::new()
        } else {
            id.to_owned()
        }
    };
    let key_width = order
        .iter()
        .map(|id| tui_theme::width(&display(id)))
        .max()
        .unwrap_or(0)
        .min(18);
    let cell = width.checked_sub(key_width + 3 * count)? / count;
    if cell < 14 {
        return None;
    }
    let pad = |text: &str, width: usize| {
        format!(
            "{text}{}",
            " ".repeat(width.saturating_sub(tui_theme::width(text)))
        )
    };
    let mut lines = Vec::new();
    let mut header = vec![Span::raw(" ".repeat(key_width))];
    for index in &view.marked {
        header.push(Span::styled(" │ ", theme.border()));
        let label = tui_theme::truncate(vec![Span::raw(view.rows[*index].label.clone())], cell);
        let text: String = label.iter().map(|span| span.content.as_ref()).collect();
        header.push(Span::styled(pad(&text, cell), theme.title()));
    }
    lines.push(Line::from(header));
    lines.push(Line::styled("─".repeat(width), theme.border()));
    let mut rows: Vec<(String, Vec<String>)> = vec![(
        "summary".into(),
        view.marked
            .iter()
            .map(|index| view.rows[*index].cells().join(" · "))
            .collect(),
    )];
    for id in &order {
        rows.push((
            display(id),
            tables
                .iter()
                .map(|fields| {
                    fields
                        .iter()
                        .find(|(key, _)| key == id)
                        .map_or_else(|| "—".into(), |(_, value)| value.clone())
                })
                .collect(),
        ));
    }
    for (key, values) in rows {
        let wrapped: Vec<Vec<String>> = values
            .iter()
            .map(|value| tui_theme::wrap(value, cell))
            .collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
        for line in 0..height {
            let key_text = if line == 0 { key.as_str() } else { "" };
            let mut spans = vec![Span::styled(
                pad(
                    &key_text.chars().take(key_width).collect::<String>(),
                    key_width,
                ),
                theme.muted(),
            )];
            for (column, cells) in wrapped.iter().enumerate() {
                spans.push(Span::styled(" │ ", theme.border()));
                let text = cells.get(line).cloned().unwrap_or_default();
                let style = match key.as_str() {
                    "verdict" | "fit" => theme.verdict(&values[column]),
                    "summary" => theme.muted(),
                    _ => Style::default(),
                };
                spans.push(Span::styled(pad(&text, cell), style));
            }
            lines.push(Line::from(spans));
        }
    }
    Some(lines)
}

fn render_table(frame: &mut Frame<'_>, view: &mut ModelView, area: Rect, theme: Theme) {
    let inner = theme.panel(
        frame,
        area,
        &format!("Models {}/{}", view.visible_count(), view.rows.len()),
    );
    view.areas.header.clear();
    view.areas.rows = Rect::default();
    if view.visible.is_empty() {
        let mut lines = vec![Line::styled("No results", theme.muted())];
        if !view.query.is_empty() || view.verdict_filter.is_some() || view.month_filter.is_some() {
            lines.push(Line::styled("Ctrl+U resets the filter", theme.muted()));
        }
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }
    let header_row = inner.height >= 3;
    let available = usize::from(inner.width).saturating_sub(2);
    let widths: Vec<usize> = (0..view.columns.len())
        .map(|column| {
            view.visible
                .iter()
                .map(|index| {
                    view.rows[*index]
                        .cells()
                        .get(column)
                        .map_or(0, |text| tui_theme::width(text))
                })
                .max()
                .unwrap_or(0)
                .max(tui_theme::width(view.columns[column]) + 2)
                .min(18)
        })
        .collect();
    let model_width = view
        .visible
        .iter()
        .map(|index| tui_theme::width(&view.rows[*index].label) + 2)
        .max()
        .unwrap_or(12)
        .clamp(12, 28);
    let mut shown: Vec<usize> = (0..view.columns.len()).collect();
    let mut droppable = view.priority.clone();
    while shown
        .iter()
        .map(|column| widths[*column] + 1)
        .sum::<usize>()
        + model_width
        > available
    {
        let Some(victim) = droppable.pop() else {
            shown.clear();
            break;
        };
        shown.retain(|column| *column != victim);
    }
    let arrow = |column: usize| match view.sort {
        Some((sorted, descending)) if sorted == column => {
            if descending {
                " ▼"
            } else {
                " ▲"
            }
        }
        _ => "",
    };
    let mut constraints = vec![Constraint::Min(u16::try_from(model_width).unwrap_or(12))];
    constraints.extend(
        shown
            .iter()
            .map(|column| Constraint::Length(u16::try_from(widths[*column]).unwrap_or(18))),
    );
    let rows: Vec<Row> = view
        .visible
        .iter()
        .enumerate()
        .map(|(position, index)| {
            let row = &view.rows[*index];
            let mut label = vec![if view.marked.contains(index) {
                Span::styled("* ", theme.title())
            } else {
                Span::raw("  ")
            }];
            label.extend(tui_theme::highlighted(
                &row.label,
                view.matches.get(position).map_or(&[], Vec::as_slice),
                theme.strong(),
                theme,
            ));
            let cells = row.cells();
            let mut columns = vec![Cell::from(Line::from(label))];
            for column in &shown {
                let text = cells.get(*column).copied().unwrap_or("");
                let style = if tui_theme::verdict_kind(text).is_some() {
                    theme.verdict(text)
                } else {
                    Style::default()
                };
                columns.push(Cell::from(Span::styled(text.to_owned(), style)));
            }
            Row::new(columns)
        })
        .collect();
    let mut table = Table::new(rows, constraints.clone())
        .column_spacing(1)
        .highlight_symbol("▸ ")
        .highlight_spacing(HighlightSpacing::Always)
        .row_highlight_style(theme.selection());
    if header_row {
        let mut header = vec![Cell::from(format!("Model{}", arrow(0)))];
        header.extend(
            shown.iter().map(|column| {
                Cell::from(format!("{}{}", view.columns[*column], arrow(column + 1)))
            }),
        );
        table = table.header(Row::new(header).style(theme.title()));
    }
    frame.render_stateful_widget(table, inner, &mut view.list);
    let body_offset = u16::from(header_row);
    view.page = usize::from(inner.height.saturating_sub(body_offset)).max(1);
    view.areas.rows = Rect {
        y: inner.y + body_offset,
        height: inner.height.saturating_sub(body_offset),
        ..inner
    };
    if header_row {
        let header_area = Rect {
            x: inner.x + 2,
            width: inner.width.saturating_sub(2),
            height: 1,
            ..inner
        };
        let cells = Layout::horizontal(constraints)
            .spacing(1)
            .split(header_area);
        view.areas.header = cells
            .iter()
            .enumerate()
            .map(|(position, area)| {
                (
                    *area,
                    if position == 0 {
                        0
                    } else {
                        shown[position - 1] + 1
                    },
                )
            })
            .collect();
    }
    tui_theme::scrollbar(
        frame,
        area,
        view.visible.len(),
        view.list.offset(),
        view.page,
        theme,
    );
}

pub fn render(frame: &mut Frame<'_>, view: &mut ModelView) {
    let area = frame.area();
    if area.is_empty() {
        return;
    }
    let theme = Theme::new(view.color);
    let compact_overview = area.height >= 12 && !view.overview.is_empty();
    let overview: Vec<String> = view
        .overview
        .iter()
        .take(2)
        .flat_map(|line| tui_theme::wrap(line, usize::from(area.width.saturating_sub(4))))
        .take(3)
        .collect();
    let context_height = if area.height >= 28 && !overview.is_empty() {
        overview.len() + 2
    } else if compact_overview {
        view.overview.len().min(2)
    } else {
        0
    };
    let [header, context, body, status, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(u16::try_from(context_height).unwrap_or(0)),
        Constraint::Min(0),
        Constraint::Length(if area.height >= 3 { 1 } else { 0 }),
        Constraint::Length(if area.height >= 2 { 1 } else { 0 }),
    ])
    .areas(area);
    theme.header(
        frame,
        header,
        &format!("rigspark / {}", view.title),
        "Read-only",
    );
    if context.height > 2 {
        let inner = theme.panel(frame, context, "Machine / scope");
        let lines: Vec<_> = overview
            .into_iter()
            .enumerate()
            .map(|(index, line)| {
                if index == 0 {
                    Line::from(line)
                } else {
                    Line::styled(line, theme.muted())
                }
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), inner);
    } else {
        let lines: Vec<_> = view
            .overview
            .iter()
            .take(2)
            .map(|line| Line::styled(format!(" {line}"), theme.muted()))
            .collect();
        frame.render_widget(Paragraph::new(lines), context);
    }
    if view.focus != Focus::List {
        view.areas = HitAreas::default();
        render_detail(frame, view, body, theme);
    } else {
        let (list_area, detail_area) = if body.width >= 110 {
            let [list, detail] =
                Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                    .areas(body);
            (list, Some(detail))
        } else if body.height >= 14 {
            let [list, detail] =
                Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(body);
            (list, Some(detail))
        } else {
            (body, None)
        };
        render_table(frame, view, list_area, theme);
        if let Some(detail) = detail_area {
            render_detail(frame, view, detail, theme);
            view.areas.detail = detail;
        } else {
            view.areas.detail = Rect::default();
        }
    }
    if let Some(toast) = &view.toast {
        let width = u16::try_from(tui_theme::width(toast) + 4)
            .unwrap_or(u16::MAX)
            .min(body.width);
        if body.height >= 3 && width >= 8 {
            let area = Rect {
                x: body.right() - width,
                y: body.bottom() - 3,
                width,
                height: 3,
            };
            frame.render_widget(Clear, area);
            let inner = theme.panel(frame, area, "");
            frame.render_widget(
                Paragraph::new(Line::styled(toast.clone(), theme.success())),
                inner,
            );
        }
    }
    let left = if view.searching {
        Line::from(vec![
            Span::styled(" /", theme.title()),
            Span::raw(view.query.clone()),
            Span::styled("▌", theme.accent()),
        ])
    } else if let Some(notice) = view.notice {
        Line::styled(format!(" {notice}"), theme.warning())
    } else if !view.query.is_empty() {
        Line::from(vec![
            Span::styled(" Filter ", theme.muted()),
            Span::styled(view.query.clone(), theme.accent()),
        ])
    } else {
        Line::default()
    };
    let mut position = if view.focus == Focus::List {
        format!(
            "{}/{}",
            view.list.selected().map_or(0, |index| index + 1),
            view.visible_count()
        )
    } else {
        format!("Line {}/{}", view.scroll + 1, view.scroll_max + 1)
    };
    if view.comparison {
        position.push_str(&format!(" · Marked {}/{MAX_MARKED}", view.marked.len()));
    }
    if let Some(months) = view.month_filter {
        position.insert_str(0, &format!("last {months} mo · "));
    }
    if let Some(verdict) = view.verdict_filter {
        position.insert_str(
            0,
            &format!("verdict {} · ", format!("{verdict:?}").to_lowercase()),
        );
    }
    if let Some((column, descending)) = view.sort {
        let name = if column == 0 {
            "model"
        } else {
            view.columns.get(column - 1).copied().unwrap_or("?")
        };
        position.insert_str(
            0,
            &format!(
                "sort {} {} · ",
                name.to_lowercase(),
                if descending { "▼" } else { "▲" }
            ),
        );
    }
    theme.bar(
        frame,
        status,
        left,
        Line::styled(format!("{position} "), theme.muted()),
    );
    let mut hints: Vec<(&str, &str)> = if view.searching {
        vec![("Enter", "apply"), ("Esc", "close"), ("Ctrl+U", "reset")]
    } else if view.focus == Focus::Help {
        vec![("Esc", "back"), ("↑↓", "scroll"), ("q", "quit")]
    } else if view.focus != Focus::List {
        vec![
            ("Esc", "back"),
            ("↑↓", "scroll"),
            ("Home/End", "jump"),
            ("?", "help"),
            ("q", "quit"),
        ]
    } else {
        vec![
            ("q", "quit"),
            ("?", "help"),
            ("/", "search"),
            ("Enter", "detail"),
            ("s", "sort"),
        ]
    };
    if !view.searching && view.focus == Focus::List {
        if !view.columns.is_empty() {
            hints.push(("v", "verdict"));
        }
        if view.month_cutoffs.is_some() {
            hints.push(("m", "recent"));
        }
        hints.push(("y", "copy"));
        if view.comparison {
            hints.extend([("Space", "mark"), ("c", "compare")]);
        } else {
            hints.push(("i", "overview"));
        }
        if view.command.is_some() {
            hints.push(("p", "finish/print"));
        }
    }
    frame.render_widget(Paragraph::new(theme.hints(&hints, footer.width)), footer);
}

pub async fn show_models(mut view: ModelView) -> io::Result<ModelOutcome> {
    let mut signals = crate::cancellation::TerminalSignals::new()?;
    let (mut terminal, _restore) = crate::tui_view::enter_terminal()?;
    let mut events = crate::terminal_events::terminal_events();
    loop {
        terminal.draw(|frame| render(frame, &mut view))?;
        let expiry = tokio::time::sleep(std::time::Duration::from_millis(2500));
        let event = tokio::select! {
            code = signals.recv() => return Ok(ModelOutcome::Exit { code: code? }),
            event = crate::tui_view::next_navigation_event(&mut events) => event?,
            () = expiry, if view.toast.is_some() => {
                view.toast = None;
                continue;
            }
        };
        if let Some(outcome) = handle_event(&mut view, event) {
            return Ok(outcome);
        }
        if let Some(text) = view.take_clipboard() {
            tui_theme::copy_to_clipboard(&text)?;
        }
    }
}

#[cfg(test)]
mod recency_tests {
    use super::*;

    #[test]
    fn m_cycles_month_windows_over_release_or_added_dates() {
        let rows = ["new:8b", "older:8b", "undated:8b"]
            .iter()
            .map(|id| ModelRow::new(id, id, "evidence"))
            .collect::<io::Result<Vec<_>>>()
            .unwrap();
        let dates = [("new:8b", "2026-09-30"), ("older:8b", "2026-08-01")]
            .into_iter()
            .map(|(id, day)| (id.to_string(), day.to_string()))
            .collect();
        let mut view = ModelView::new("Catalog", Vec::new(), rows, false)
            .unwrap()
            .with_recency(&dates, "2026-10-07")
            .unwrap();
        let key = KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE);
        let shown = |view: &ModelView| {
            view.visible
                .iter()
                .map(|index| view.rows[*index].label.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(&view).len(), 3);
        handle_key(&mut view, key);
        assert_eq!(shown(&view), ["new:8b"], "1 month");
        handle_key(&mut view, key);
        handle_key(&mut view, key);
        assert_eq!(
            shown(&view),
            ["new:8b", "older:8b"],
            "3 months; undated is never assumed recent"
        );
        handle_key(&mut view, key);
        assert_eq!(shown(&view).len(), 3, "back to all");
    }
}
