use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
    },
};
use unicode_segmentation::UnicodeSegmentation;

/// Shared terminal styling; monochrome mode uses modifiers only, never colors.
#[derive(Clone, Copy)]
pub(crate) struct Theme {
    color: bool,
    palette: Palette,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Palette {
    Default,
    Light,
    HighContrast,
}

impl Palette {
    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "" | "default" | "dark" => Some(Self::Default),
            "light" => Some(Self::Light),
            "high-contrast" | "contrast" => Some(Self::HighContrast),
            _ => None,
        }
    }

    fn current() -> Self {
        static PALETTE: std::sync::OnceLock<Palette> = std::sync::OnceLock::new();
        *PALETTE.get_or_init(|| {
            std::env::var("RIGSPARK_THEME")
                .ok()
                .filter(|name| name.len() <= 32)
                .and_then(|name| Self::parse(&name))
                .unwrap_or(Self::Default)
        })
    }
}

impl Theme {
    pub(crate) fn new(color: bool) -> Self {
        Self::with_palette(color, Palette::current())
    }

    pub(crate) fn with_palette(color: bool, palette: Palette) -> Self {
        Self { color, palette }
    }

    fn fg(self, color: Color) -> Style {
        if self.color {
            Style::default().fg(color)
        } else {
            Style::default()
        }
    }

    pub(crate) fn accent(self) -> Style {
        self.fg(match self.palette {
            Palette::Default => Color::Cyan,
            Palette::Light => Color::Blue,
            Palette::HighContrast => Color::Yellow,
        })
    }

    pub(crate) fn title(self) -> Style {
        self.accent().add_modifier(Modifier::BOLD)
    }

    pub(crate) fn strong(self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    pub(crate) fn muted(self) -> Style {
        match (self.color, self.palette) {
            (_, Palette::HighContrast) => Style::default(),
            (true, _) => Style::default().fg(Color::DarkGray),
            (false, _) => Style::default().add_modifier(Modifier::DIM),
        }
    }

    pub(crate) fn border(self) -> Style {
        match self.palette {
            Palette::HighContrast => Style::default(),
            _ => self.muted(),
        }
    }

    pub(crate) fn warning(self) -> Style {
        self.fg(Color::Yellow).add_modifier(Modifier::BOLD)
    }

    pub(crate) fn error(self) -> Style {
        self.fg(Color::Red).add_modifier(Modifier::BOLD)
    }

    pub(crate) fn success(self) -> Style {
        self.fg(Color::Green).add_modifier(Modifier::BOLD)
    }

    pub(crate) fn selection(self) -> Style {
        match (self.color, self.palette) {
            (true, Palette::Default) => Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            (true, Palette::Light) => Style::default()
                .fg(Color::White)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
            _ => Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
        }
    }

    pub(crate) fn highlight(self) -> Style {
        self.accent()
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    }

    /// Styles a fit verdict word; anything unrecognised keeps the default style.
    pub(crate) fn verdict(self, word: &str) -> Style {
        match verdict_kind(word) {
            Some(Verdict::Yes) => self.success(),
            Some(Verdict::Slow) => self.warning(),
            Some(Verdict::No) => self.error(),
            Some(Verdict::Unknown) => self.muted(),
            None => Style::default(),
        }
    }

    /// Draws a bordered panel and returns its padded interior; tiny areas stay borderless.
    pub(crate) fn panel(self, frame: &mut Frame<'_>, area: Rect, title: &str) -> Rect {
        if area.width < 8 || area.height < 3 {
            return area;
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(self.border());
        let block = if title.is_empty() {
            block
        } else {
            block.title(Span::styled(format!(" {title} "), self.title()))
        };
        let inner = block.inner(area);
        frame.render_widget(block, area);
        Rect {
            x: inner.x.saturating_add(1).min(inner.right()),
            width: inner.width.saturating_sub(2),
            ..inner
        }
    }

    /// One-line bar with a left title and, when it fits, right-aligned muted metadata.
    pub(crate) fn header(self, frame: &mut Frame<'_>, area: Rect, title: &str, right: &str) {
        self.bar(
            frame,
            area,
            Line::styled(format!(" {title}"), self.title()),
            Line::styled(format!("{right} "), self.muted()),
        );
    }

    /// Renders `left`, plus right-aligned `right` only when both fit without overlap.
    pub(crate) fn bar(self, frame: &mut Frame<'_>, area: Rect, left: Line<'_>, right: Line<'_>) {
        if area.is_empty() {
            return;
        }
        let left_width = left.width();
        if right.width() > 0 && left_width + right.width() + 2 <= usize::from(area.width) {
            frame.render_widget(Paragraph::new(right).alignment(Alignment::Right), area);
        }
        frame.render_widget(
            Paragraph::new(left),
            Rect {
                width: area
                    .width
                    .min(u16::try_from(left_width).unwrap_or(u16::MAX)),
                ..area
            },
        );
    }

    /// Key hints that fit `width`; earlier hints take priority.
    pub(crate) fn hints(self, hints: &[(&str, &str)], width: u16) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        let mut used = 1;
        for (key, label) in hints {
            let cost = Span::raw(*key).width() + Span::raw(*label).width() + 3;
            if used + cost > usize::from(width) {
                break;
            }
            spans.push(Span::styled((*key).to_owned(), self.title()));
            spans.push(Span::styled(format!(" {label}  "), self.muted()));
            used += cost;
        }
        Line::from(spans)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Verdict {
    Yes,
    Slow,
    No,
    Unknown,
}

pub(crate) fn verdict_kind(word: &str) -> Option<Verdict> {
    let word = word.trim().to_ascii_lowercase();
    match word.strip_prefix("fit: ").unwrap_or(&word) {
        "yes" | "fit" | "fits" => Some(Verdict::Yes),
        "slow" | "tight" | "partial" => Some(Verdict::Slow),
        "no" | "won't fit" | "wont-fit" | "does not fit" => Some(Verdict::No),
        bound if bound.ends_with("-bound") && !bound.contains(' ') => Some(Verdict::No),
        "unknown" => Some(Verdict::Unknown),
        _ => None,
    }
}

pub(crate) fn width(text: &str) -> usize {
    Span::raw(text).width()
}

/// Word-aware wrap; words wider than a line are split at grapheme boundaries.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for (index, word) in text.split(' ').enumerate() {
        let cells = self::width(word);
        let gap = usize::from(index > 0);
        if used + gap + cells <= width {
            if gap == 1 {
                line.push(' ');
            }
            line.push_str(word);
            used += gap + cells;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        for grapheme in word.graphemes(true) {
            let cells = self::width(grapheme);
            if used + cells > width && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            if cells > width {
                line.push('?');
                used += 1;
            } else {
                line.push_str(grapheme);
                used += cells;
            }
        }
    }
    lines.push(line);
    lines
}

/// Truncates styled spans to `width` cells, marking the cut with an ellipsis.
pub(crate) fn truncate(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(Span::width).sum();
    if total <= width {
        return spans;
    }
    let budget = width.saturating_sub(1);
    let mut used = 0;
    let mut result = Vec::new();
    let mut last_style = Style::default();
    for span in spans {
        last_style = span.style;
        let mut kept = String::new();
        for grapheme in span.content.graphemes(true) {
            let cells = self::width(grapheme);
            if used + cells > budget {
                break;
            }
            kept.push_str(grapheme);
            used += cells;
        }
        let complete = kept.len() == span.content.len();
        result.push(Span::styled(kept, span.style));
        if !complete {
            break;
        }
    }
    if width > 0 {
        result.push(Span::styled("…", last_style));
    }
    result
}

/// Memory gauge `used / total`; colour reflects headroom so overcommit is obvious at a glance.
pub(crate) fn gauge(theme: Theme, used: f64, total: f64, width: usize) -> Line<'static> {
    if !(used.is_finite() && total.is_finite() && used >= 0.0 && total > 0.0) {
        return Line::styled("unknown", theme.muted());
    }
    let ratio = used / total;
    let label = format!(
        " {:.1} / {:.1} GiB · {:.0}%",
        used / 1_073_741_824.0,
        total / 1_073_741_824.0,
        ratio * 100.0
    );
    let bar = width.saturating_sub(self::width(&label)).clamp(4, 40);
    let filled = ((ratio.min(1.0) * bar as f64).round() as usize).min(bar);
    let style = if ratio > 1.0 {
        theme.error()
    } else if ratio > 0.8 {
        theme.warning()
    } else {
        theme.success()
    };
    Line::from(vec![
        Span::styled("█".repeat(filled), style),
        Span::styled("░".repeat(bar - filled), theme.muted()),
        Span::raw(label),
    ])
}

/// Case-insensitive grapheme subsequence match; returns matched grapheme indices.
pub(crate) fn fuzzy(haystack: &str, needle: &str) -> Option<Vec<usize>> {
    let needle: Vec<String> = needle.graphemes(true).map(str::to_lowercase).collect();
    if needle.is_empty() {
        return Some(Vec::new());
    }
    let mut positions = Vec::with_capacity(needle.len());
    let mut wanted = needle.iter();
    let mut current = wanted.next();
    for (index, grapheme) in haystack.graphemes(true).enumerate() {
        let Some(target) = current else { break };
        if grapheme.to_lowercase() == *target {
            positions.push(index);
            current = wanted.next();
        }
    }
    current.is_none().then_some(positions)
}

/// Lower is better: contiguous, early matches rank first.
pub(crate) fn fuzzy_rank(positions: &[usize]) -> usize {
    let gaps: usize = positions.windows(2).map(|pair| pair[1] - pair[0] - 1).sum();
    gaps * 4 + positions.first().copied().unwrap_or(0)
}

/// Label spans with fuzzy-matched graphemes highlighted.
pub(crate) fn highlighted(
    text: &str,
    positions: &[usize],
    base: Style,
    theme: Theme,
) -> Vec<Span<'static>> {
    if positions.is_empty() {
        return vec![Span::styled(text.to_owned(), base)];
    }
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (index, grapheme) in text.graphemes(true).enumerate() {
        let style = if positions.binary_search(&index).is_ok() {
            base.patch(theme.highlight())
        } else {
            base
        };
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(grapheme),
            _ => spans.push(Span::styled(grapheme.to_owned(), style)),
        }
    }
    spans
}

/// OSC 52 clipboard write; the payload is base64 so it cannot inject terminal controls.
pub(crate) fn osc52(text: &str) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let triple = chunk.iter().enumerate().fold(0u32, |acc, (index, byte)| {
            acc | u32::from(*byte) << (16 - 8 * index)
        });
        for index in 0..4 {
            if index <= chunk.len() {
                encoded.push(char::from(
                    TABLE[(triple >> (18 - 6 * index)) as usize & 63],
                ));
            } else {
                encoded.push('=');
            }
        }
    }
    format!("\x1b]52;c;{encoded}\x07")
}

pub(crate) fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut stderr = std::io::stderr();
    stderr.write_all(osc52(text).as_bytes())?;
    stderr.flush()
}

pub(crate) fn scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    content: usize,
    position: usize,
    viewport: usize,
    theme: Theme,
) {
    if content <= viewport || area.height < 3 {
        return;
    }
    let area = Rect {
        y: area.y + 1,
        height: area.height - 2,
        ..area
    };
    let mut state = ScrollbarState::new(content.saturating_sub(viewport))
        .position(position)
        .viewport_content_length(viewport);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .track_style(theme.border())
            .thumb_style(theme.accent()),
        area,
        &mut state,
    );
}

/// Brackets each frame in DEC 2026 synchronized-update markers to prevent tearing.
pub(crate) struct SyncWriter<W: std::io::Write> {
    inner: W,
    open: bool,
}

impl<W: std::io::Write> SyncWriter<W> {
    pub(crate) fn new(inner: W) -> Self {
        Self { inner, open: false }
    }
}

impl<W: std::io::Write> std::io::Write for SyncWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !self.open && !buf.is_empty() {
            self.inner.write_all(b"\x1b[?2026h")?;
            self.open = true;
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if self.open {
            self.inner.write_all(b"\x1b[?2026l")?;
            self.open = false;
        }
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_breaks_on_words_and_splits_only_oversized_words() {
        assert_eq!(wrap("alpha beta gamma", 11), ["alpha beta", "gamma"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("ab abcdefgh", 4), ["ab", "abcd", "efgh"]);
        assert_eq!(wrap("", 4), [""]);
        assert_eq!(wrap("\u{754c}", 1), ["?"]);
    }

    #[test]
    fn truncate_keeps_short_spans_and_marks_cuts() {
        let spans = vec![Span::raw("abc"), Span::raw("def")];
        assert_eq!(truncate(spans.clone(), 6), spans);
        let cut: String = truncate(spans, 5)
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(cut, "abcd…");
    }

    #[test]
    fn osc52_is_base64_framed_and_fuzzy_matches_graphemes_in_order() {
        assert_eq!(osc52("hello"), "\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(osc52("ab"), "\x1b]52;c;YWI=\x07");
        assert_eq!(osc52("\x1b[2J"), "\x1b]52;c;G1sySg==\x07");
        assert_eq!(fuzzy("Llama3.1:8b", "l38"), Some(vec![0, 5, 9]));
        assert_eq!(fuzzy("alpha code", "chat"), None);
        assert_eq!(fuzzy("alpha", "e\u{301}"), None);
        assert!(fuzzy_rank(&[0, 1, 2]) < fuzzy_rank(&[0, 4, 9]));
    }

    #[test]
    fn sync_writer_brackets_only_frames_that_wrote() {
        use std::io::Write;
        let mut writer = SyncWriter::new(Vec::new());
        writer.flush().unwrap();
        writer.write_all(b"x").unwrap();
        writer.flush().unwrap();
        assert_eq!(writer.inner, b"\x1b[?2026hx\x1b[?2026l");
    }

    #[test]
    fn gauge_never_fabricates_and_flags_overcommit() {
        let theme = Theme::with_palette(false, Palette::Default);
        let text = |line: Line<'_>| {
            line.spans
                .iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        };
        assert_eq!(text(gauge(theme, f64::NAN, 10.0, 40)), "unknown");
        assert_eq!(text(gauge(theme, 1.0, 0.0, 40)), "unknown");
        let over = gauge(Theme::with_palette(true, Palette::Default), 2.0, 1.0, 40);
        assert_eq!(
            over.spans[0].style,
            Theme::with_palette(true, Palette::Default).error()
        );
        assert!(text(over).ends_with("· 200%"));
    }

    #[test]
    fn monochrome_theme_never_emits_colors() {
        for palette in [Palette::Default, Palette::Light, Palette::HighContrast] {
            let theme = Theme::with_palette(false, palette);
            for style in [
                theme.accent(),
                theme.title(),
                theme.muted(),
                theme.border(),
                theme.highlight(),
                theme.warning(),
                theme.error(),
                theme.success(),
                theme.selection(),
                theme.verdict("yes"),
                theme.verdict("no"),
            ] {
                assert_eq!(style.fg, None);
                assert_eq!(style.bg, None);
            }
        }
        assert_eq!(Palette::parse("LIGHT"), Some(Palette::Light));
        assert_eq!(Palette::parse("neon"), None);
        assert_eq!(verdict_kind("ram-bound"), Some(Verdict::No));
    }

    fn styled(fg: Option<Color>, bg: Option<Color>, modifier: Modifier) -> Style {
        let mut style = Style::default().add_modifier(modifier);
        style.fg = fg;
        style.bg = bg;
        style
    }

    #[test]
    fn each_palette_has_its_exact_colors_and_modifiers() {
        let dark = Theme::with_palette(true, Palette::Default);
        let light = Theme::with_palette(true, Palette::Light);
        let contrast = Theme::with_palette(true, Palette::HighContrast);
        let mono = Theme::with_palette(false, Palette::Default);
        let none = Modifier::empty();
        let bold = Modifier::BOLD;
        assert_eq!(dark.accent(), styled(Some(Color::Cyan), None, none));
        assert_eq!(light.accent(), styled(Some(Color::Blue), None, none));
        assert_eq!(contrast.accent(), styled(Some(Color::Yellow), None, none));
        assert_eq!(dark.strong(), styled(None, None, bold));
        assert_eq!(dark.warning(), styled(Some(Color::Yellow), None, bold));
        assert_eq!(dark.error(), styled(Some(Color::Red), None, bold));
        assert_eq!(dark.success(), styled(Some(Color::Green), None, bold));
        assert_eq!(mono.error(), styled(None, None, bold));
        assert_eq!(dark.border(), styled(Some(Color::DarkGray), None, none));
        assert_eq!(mono.border(), styled(None, None, Modifier::DIM));
        assert_eq!(contrast.border(), Style::default());
        assert_eq!(
            dark.selection(),
            styled(Some(Color::Black), Some(Color::Cyan), bold)
        );
        assert_eq!(
            light.selection(),
            styled(Some(Color::White), Some(Color::Blue), bold)
        );
        for theme in [contrast, mono] {
            assert_eq!(
                theme.selection(),
                styled(None, None, Modifier::REVERSED | Modifier::BOLD)
            );
        }
        assert_eq!(
            dark.highlight(),
            styled(
                Some(Color::Cyan),
                None,
                Modifier::BOLD | Modifier::UNDERLINED
            )
        );
        assert_eq!(dark.verdict("Fit: yes"), dark.success());
        assert_eq!(dark.verdict("tight"), dark.warning());
        assert_eq!(dark.verdict("no"), dark.error());
        assert_eq!(dark.verdict("unknown"), dark.muted());
        assert_eq!(dark.verdict("maybe"), Style::default());
    }

    #[test]
    fn palette_names_are_trimmed_and_case_insensitive() {
        for (name, palette) in [
            ("", Palette::Default),
            (" default ", Palette::Default),
            ("Dark", Palette::Default),
            ("light", Palette::Light),
            ("high-contrast", Palette::HighContrast),
            ("CONTRAST", Palette::HighContrast),
        ] {
            assert_eq!(Palette::parse(name), Some(palette), "{name:?}");
        }
    }

    /// Run in a child process by the test below, with `RIGSPARK_THEME` already set.
    #[test]
    fn theme_probe() {
        if let Some(expected) = std::env::var_os("RIGSPARK_TEST_THEME_EXPECT") {
            assert_eq!(
                format!("{:?}", Palette::current()),
                expected.to_string_lossy()
            );
        }
    }

    #[test]
    fn the_theme_variable_picks_a_palette_up_to_32_bytes() {
        for (value, expected) in [
            ("light", "Light"),
            (&format!("{:<32}", "light")[..], "Light"),
            (&format!("{:<33}", "light")[..], "Default"),
            ("neon", "Default"),
        ] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tui_theme::tests::theme_probe",
                    "--test-threads=1",
                ])
                .env("RIGSPARK_THEME", value)
                .env("RIGSPARK_TEST_THEME_EXPECT", expected)
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success(), "{value:?}: {stdout}");
            assert!(stdout.contains("1 passed"), "{value:?}: {stdout}");
        }
    }

    fn draw(width: u16, height: u16, paint: impl FnOnce(&mut Frame<'_>, Rect)) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                paint(frame, area);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn panels_need_eight_columns_and_three_rows_and_pad_their_interior() {
        let theme = Theme::with_palette(false, Palette::Default);
        let inner = |width: u16, height: u16| {
            let mut result = Rect::default();
            draw(width, height, |frame, area| {
                result = theme.panel(frame, area, "t");
            });
            result
        };
        assert_eq!(inner(7, 10), Rect::new(0, 0, 7, 10));
        assert_eq!(inner(20, 2), Rect::new(0, 0, 20, 2));
        assert_eq!(inner(8, 3), Rect::new(2, 1, 4, 1));
        assert_eq!(inner(20, 5), Rect::new(2, 1, 16, 3));
    }

    #[test]
    fn bars_show_the_right_side_only_with_two_spare_columns() {
        let theme = Theme::with_palette(false, Palette::Default);
        let bar = |width: u16| {
            draw(width, 1, |frame, area| {
                theme.bar(frame, area, Line::raw("abc"), Line::raw("xyz"));
            })[0]
                .clone()
        };
        assert_eq!(bar(8), "abc  xyz");
        assert_eq!(bar(7), "abc    ");
        assert_eq!(bar(2), "ab");
    }

    #[test]
    fn hints_are_kept_while_they_fit() {
        let theme = Theme::with_palette(false, Palette::Default);
        let shown = |width: u16| {
            theme
                .hints(&[("q", "close"), ("/", "find")], width)
                .spans
                .len()
        };
        // " " plus "q close  " (9 cells), then "/ find  " (8 cells).
        assert_eq!(shown(9), 1);
        assert_eq!(shown(10), 3);
        assert_eq!(shown(17), 3);
        assert_eq!(shown(18), 5);
    }

    #[test]
    fn wrapping_fills_single_cell_lines_and_counts_placeholders() {
        assert_eq!(wrap("abc", 1), ["a", "b", "c"]);
        assert_eq!(wrap("\u{754c}a", 1), ["?", "a"]);
    }

    #[test]
    fn a_zero_width_truncation_has_no_ellipsis() {
        let cut = truncate(vec![Span::raw("abc")], 0);
        assert_eq!(cut, vec![Span::raw("")]);
        let cut = truncate(vec![Span::raw("abc")], 1);
        assert_eq!(cut, vec![Span::raw(""), Span::raw("…")]);
    }

    #[test]
    fn gauges_fill_by_ratio_and_color_by_headroom() {
        let theme = Theme::with_palette(true, Palette::Default);
        let gib = 1_073_741_824.0;
        let parts = |used: f64| {
            let line = gauge(theme, used * gib, 2.0 * gib, 60);
            (
                line.spans[0].content.chars().count(),
                line.spans[1].content.chars().count(),
                line.spans[0].style,
                line.spans[2].content.to_string(),
            )
        };
        let (filled, empty, style, label) = parts(1.0);
        assert_eq!(label, " 1.0 / 2.0 GiB · 50%");
        assert_eq!((filled, empty), (20, 20));
        assert_eq!(style, theme.success());
        assert_eq!(parts(1.6).2, theme.success());
        assert_eq!(parts(1.8).2, theme.warning());
        assert_eq!(parts(2.0).2, theme.warning());
        assert_eq!(parts(2.0).1, 0);
        assert_eq!(parts(2.2).2, theme.error());
    }

    #[test]
    fn fuzzy_rank_is_four_per_gap_plus_the_first_position() {
        assert_eq!(fuzzy_rank(&[]), 0);
        assert_eq!(fuzzy_rank(&[2, 3]), 2);
        assert_eq!(fuzzy_rank(&[2, 3, 7]), 14);
        assert_eq!(fuzzy_rank(&[0, 5]), 16);
    }

    #[test]
    fn highlighted_runs_share_one_span() {
        let theme = Theme::with_palette(true, Palette::Default);
        let spans = highlighted("abcd", &[0, 1], Style::default(), theme);
        assert_eq!(
            spans,
            vec![
                Span::styled("ab", theme.highlight()),
                Span::styled("cd", Style::default()),
            ]
        );
        assert_eq!(
            highlighted("ab", &[], Style::default(), theme),
            vec![Span::raw("ab")]
        );
    }

    #[test]
    fn scrollbars_appear_only_for_overflowing_content_in_three_rows() {
        let theme = Theme::with_palette(false, Palette::Default);
        let column = |height: u16, content: usize, viewport: usize| -> String {
            draw(5, height, |frame, area| {
                scrollbar(frame, area, content, 0, viewport, theme);
            })
            .iter()
            .map(|row| row.chars().last().unwrap())
            .collect()
        };
        assert_eq!(column(6, 4, 4), "      ");
        assert_eq!(column(2, 20, 4), "  ");
        let drawn = column(6, 20, 4);
        assert_eq!(drawn.chars().next(), Some(' '));
        assert_eq!(drawn.chars().last(), Some(' '));
        assert!(
            drawn[1..drawn.len() - 1].chars().all(|c| c != ' '),
            "{drawn:?}"
        );
        let small = column(3, 20, 2);
        assert_eq!(small.chars().filter(|c| *c != ' ').count(), 1, "{small:?}");
        assert_eq!(small.chars().nth(1).map(|c| c != ' '), Some(true));
    }
}
