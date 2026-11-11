//! Interactive cleanup checklist.

use std::fmt::Write as _;
use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::queue;
use crossterm::terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate};

use crate::cleanup::{self, Candidate, Category, CleanupReport};
use crate::format::{
    Palette, SizeUnit, display_width, sanitize_text, truncate_left, truncate_right,
};
use crate::term::{TerminalGuard, size};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CleanupOptions {
    pub color: bool,
    pub unit: SizeUnit,
    pub profile: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Filter {
    All,
    Build,
    Cache,
    Dependency,
}

impl Filter {
    fn next(self) -> Self {
        match self {
            Self::All => Self::Build,
            Self::Build => Self::Cache,
            Self::Cache => Self::Dependency,
            Self::Dependency => Self::All,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::All => Self::Dependency,
            Self::Build => Self::All,
            Self::Cache => Self::Build,
            Self::Dependency => Self::Cache,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Build => "build",
            Self::Cache => "cache",
            Self::Dependency => "deps",
        }
    }

    fn accepts(self, category: Category) -> bool {
        match self {
            Self::All => true,
            Self::Build => category == Category::Build,
            Self::Cache => category == Category::Cache,
            Self::Dependency => category == Category::Dependency,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SortMode {
    Size,
    Name,
}

#[derive(Debug)]
struct State {
    candidates: Vec<Candidate>,
    filter: Filter,
    sort: SortMode,
    cursor: usize,
    offset: usize,
    page_rows: usize,
    confirm: bool,
    deleting: bool,
    report: Option<CleanupReport>,
    status: Option<String>,
}

impl State {
    fn new(candidates: Vec<Candidate>) -> Self {
        let mut state = Self {
            candidates,
            filter: Filter::All,
            sort: SortMode::Size,
            cursor: 0,
            offset: 0,
            page_rows: 10,
            confirm: false,
            deleting: false,
            report: None,
            status: None,
        };
        state.clamp_cursor();
        state
    }

    fn visible(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = self
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| self.filter.accepts(candidate.category))
            .map(|(index, _)| index)
            .collect();
        indices.sort_by(|left, right| {
            let a = &self.candidates[*left];
            let b = &self.candidates[*right];
            match self.sort {
                SortMode::Size => b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)),
                SortMode::Name => a.path.cmp(&b.path).then_with(|| b.size.cmp(&a.size)),
            }
        });
        indices
    }

    fn selected_count(&self) -> usize {
        self.candidates
            .iter()
            .filter(|candidate| candidate.selected)
            .count()
    }

    fn selected_size(&self) -> u64 {
        self.candidates
            .iter()
            .filter(|candidate| candidate.selected)
            .fold(0u64, |sum, candidate| sum.saturating_add(candidate.size))
    }

    fn total_size(&self) -> u64 {
        self.candidates
            .iter()
            .fold(0u64, |sum, candidate| sum.saturating_add(candidate.size))
    }

    fn selected(&self) -> Option<usize> {
        self.visible().get(self.cursor).copied()
    }

    fn clamp_cursor(&mut self) {
        let visible = self.visible();
        if visible.is_empty() {
            self.cursor = 0;
            self.offset = 0;
        } else if self.cursor >= visible.len() {
            self.cursor = visible.len() - 1;
        }
    }

    fn move_by(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            self.cursor = 0;
            return;
        }
        if delta < 0 {
            self.cursor = self.cursor.saturating_sub(delta.unsigned_abs());
        } else {
            self.cursor = self.cursor.saturating_add(delta as usize).min(len - 1);
        }
    }

    fn page_by(&mut self, rows: usize, down: bool) {
        let len = self.visible().len();
        if len == 0 {
            self.cursor = 0;
            self.offset = 0;
            return;
        }
        if down {
            self.cursor = self.cursor.saturating_add(rows).min(len - 1);
        } else {
            self.cursor = self.cursor.saturating_sub(rows);
        }
    }

    fn ensure_visible(&mut self, rows: usize) {
        if rows == 0 {
            self.offset = 0;
            return;
        }
        let len = self.visible().len();
        if len == 0 {
            self.cursor = 0;
            self.offset = 0;
            return;
        }
        if self.cursor < self.offset {
            self.offset = self.cursor;
        }
        if self.cursor >= self.offset.saturating_add(rows) {
            self.offset = self.cursor.saturating_add(1).saturating_sub(rows);
        }
        let max_offset = len.saturating_sub(rows);
        self.offset = self.offset.min(max_offset);
    }

    fn toggle_selected(&mut self) {
        if let Some(index) = self.selected() {
            self.candidates[index].selected = !self.candidates[index].selected;
            self.status = None;
        }
    }

    fn select_visible(&mut self, selected: bool) {
        let visible = self.visible();
        for index in visible {
            self.candidates[index].selected = selected;
        }
        self.status = None;
    }

    fn cycle_filter(&mut self) {
        self.filter = self.filter.next();
        self.cursor = 0;
        self.offset = 0;
        self.clamp_cursor();
    }

    fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            SortMode::Size => SortMode::Name,
            SortMode::Name => SortMode::Size,
        };
        self.cursor = 0;
        self.offset = 0;
    }
}

pub fn run(candidates: Vec<Candidate>, options: CleanupOptions) -> io::Result<CleanupReport> {
    let _guard = TerminalGuard::enter()?;
    let mut state = State::new(candidates);

    loop {
        let (width, height) = size()?;
        let lines = build_frame(&mut state, &options, width, height);
        draw(&lines)?;

        if let Some(report) = &state.report {
            let event = event::read()?;
            if let Event::Key(key) = event
                && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
            {
                return Ok(report.clone());
            }
            continue;
        }

        let event = event::read()?;
        if let Event::Key(key) = event
            && matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
            && handle_key(&mut state, key)
        {
            return Ok(state.report.unwrap_or_default());
        }
        if state.deleting {
            let (width, height) = size()?;
            let lines = deleting_frame(&state, &options, width, height);
            draw(&lines)?;
            let report = cleanup::delete_selected(&state.candidates);
            state.report = Some(report);
            state.deleting = false;
            state.confirm = false;
        }
    }
}

fn handle_key(state: &mut State, key: KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
    {
        return true;
    }
    if state.confirm {
        match key.code {
            KeyCode::Char('y' | 'Y') => {
                if state.selected_count() == 0 {
                    state.confirm = false;
                    state.status = Some("nothing selected".to_owned());
                } else {
                    state.deleting = true;
                }
            }
            KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Char('q') => {
                state.confirm = false;
            }
            _ => {}
        }
        return false;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return true,
        KeyCode::Up | KeyCode::Char('k') => state.move_by(-1),
        KeyCode::Down | KeyCode::Char('j') => state.move_by(1),
        KeyCode::PageUp => state.page_by(state.page_rows, false),
        KeyCode::PageDown => state.page_by(state.page_rows, true),
        KeyCode::Home | KeyCode::Char('g') => state.cursor = 0,
        KeyCode::End | KeyCode::Char('G') => {
            state.cursor = state.visible().len().saturating_sub(1);
        }
        KeyCode::Char(' ') => state.toggle_selected(),
        KeyCode::Char('a') => state.select_visible(true),
        KeyCode::Char('n') | KeyCode::Char('u') => state.select_visible(false),
        KeyCode::Char('f') | KeyCode::Tab | KeyCode::Right => state.cycle_filter(),
        KeyCode::Left | KeyCode::BackTab => {
            state.filter = state.filter.previous();
            state.cursor = 0;
            state.offset = 0;
            state.clamp_cursor();
        }
        KeyCode::Char('s') => state.cycle_sort(),
        KeyCode::Enter => {
            if state.selected_count() == 0 {
                state.status = Some("select at least one item".to_owned());
            } else {
                state.confirm = true;
                state.status = None;
            }
        }
        _ => {}
    }
    false
}

fn draw(lines: &[String]) -> io::Result<()> {
    let (_, height) = size()?;
    let mut out = io::stdout().lock();
    queue!(out, BeginSynchronizedUpdate)?;
    let result = (|| -> io::Result<()> {
        queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
        for (index, line) in lines.iter().take(height).enumerate() {
            queue!(out, MoveTo(0, index as u16))?;
            out.write_all(line.as_bytes())?;
        }
        Ok(())
    })();
    let end = queue!(out, EndSynchronizedUpdate);
    let flush = out.flush();
    result?;
    end?;
    flush
}

fn build_frame(
    state: &mut State,
    options: &CleanupOptions,
    width: usize,
    height: usize,
) -> Vec<String> {
    if width < 40 || height < 8 {
        return vec![center(
            &format!("terminal too small ({width}x{height}); need 40x8"),
            width,
        )];
    }
    let palette = Palette::new(options.color);
    if let Some(report) = &state.report {
        return report_frame(report, &palette, options, width, height);
    }
    if state.confirm {
        return confirm_frame(state, &palette, options, width, height);
    }

    let visible = state.visible();
    let list_rows = height.saturating_sub(5).max(1);
    state.page_rows = list_rows;
    state.ensure_visible(list_rows);

    let mut lines = Vec::with_capacity(height);
    lines.push(header_line(state, &palette, options, width));
    lines.push(filter_line(state, &palette, width));
    lines.push(palette.dim(&"─".repeat(width)));

    if visible.is_empty() {
        lines.push(palette.dim("  no candidates in this filter"));
    } else {
        let category_width = 5;
        let language_width = 9;
        let marker_width = 2;
        let checkbox_width = 4;
        let used = marker_width + checkbox_width + category_width + 1 + language_width + 1;
        let max_size_width = width.saturating_sub(used + 2 + 1).max(1);
        let size_width = visible
            .iter()
            .map(|index| display_width(&options.unit.format(state.candidates[*index].size)))
            .max()
            .unwrap_or(8)
            .max(7)
            .min(max_size_width);
        let name_width = width.saturating_sub(used + size_width + 2);
        let end = state.offset.saturating_add(list_rows).min(visible.len());
        for (position, index) in visible.iter().enumerate().take(end).skip(state.offset) {
            let candidate = &state.candidates[*index];
            let selected = candidate.selected;
            let cursor = position == state.cursor;
            let marker = if cursor { "❯ " } else { "  " };
            let check = if selected { "[x] " } else { "[ ] " };
            let style_check = if selected {
                palette.accent(check)
            } else {
                palette.dim(check)
            };
            let category = match candidate.category {
                Category::Build => palette.accent(&format!(
                    "{:<width$}",
                    candidate.category.label(),
                    width = category_width
                )),
                Category::Cache => palette.directory(&format!(
                    "{:<width$}",
                    candidate.category.label(),
                    width = category_width
                )),
                Category::Dependency => palette.warning(&format!(
                    "{:<width$}",
                    candidate.category.label(),
                    width = category_width
                )),
            };
            let language = format!(
                "{:<width$}",
                sanitize_text(&candidate.language),
                width = language_width
            );
            let path = compact_candidate_path(candidate);
            let path = truncate_left(&path, name_width);
            let path_style = if cursor {
                palette.bold(&path)
            } else if selected {
                palette.accent(&path)
            } else {
                palette.dim(&path)
            };
            let size_full = options.unit.format(candidate.size);
            let size = truncate_right(&size_full, size_width);
            let mut line = String::new();
            let _ = write!(
                line,
                "{}{}{} {} {:>size_width$}  {path_style}",
                if cursor {
                    palette.accent(marker)
                } else {
                    marker.to_owned()
                },
                style_check,
                category,
                language,
                size,
                size_width = size_width,
            );
            lines.push(line);
        }
    }

    let selected = state.selected().map(|index| &state.candidates[index]);
    lines.push(detail_line(selected, &palette, options, width));
    lines.push(hint_line(state, &palette, width));
    lines.truncate(height);
    lines
}

fn header_line(state: &State, palette: &Palette, options: &CleanupOptions, width: usize) -> String {
    let count = state.selected_count();
    let selected_size = options.unit.format(state.selected_size());
    let total_size = options.unit.format(state.total_size());
    let right = truncate_right(
        &format!("{count} selected  ·  {selected_size} / {total_size}"),
        width,
    )
    .into_owned();
    let right_width = display_width(&right);
    let left_max = width.saturating_sub(right_width + 1);
    let profile = sanitize_text(&options.profile);
    let left = format!("dulens clean  profile {profile}");
    let left = truncate_right(&left, left_max);
    let left_width = display_width(&left);
    let gap = width.saturating_sub(left_width + right_width);
    format!(
        "{}{}{}",
        palette.title(&left),
        " ".repeat(gap),
        palette.accent(&right)
    )
}

fn filter_line(state: &State, palette: &Palette, width: usize) -> String {
    let counts = format!(
        "{} candidates  ·  {} shown",
        state.candidates.len(),
        state.visible().len()
    );
    if width < 60 {
        let text = format!("filter: {}  ·  {}", state.filter.label(), counts);
        return palette.dim(&truncate_right(&text, width));
    }

    let mut raw_width = display_width("filter ");
    let mut line = String::from("filter ");
    for filter in [
        Filter::All,
        Filter::Build,
        Filter::Cache,
        Filter::Dependency,
    ] {
        let token = format!(" {} ", filter.label());
        let token_width = display_width(&token);
        if raw_width + token_width + 2 > width {
            break;
        }
        if filter == state.filter {
            line.push_str(&palette.accent(&token));
        } else {
            line.push_str(&palette.dim(&token));
        }
        line.push(' ');
        raw_width += token_width + 1;
    }
    let counts_text = truncate_right(&counts, width.saturating_sub(raw_width + 1));
    let counts_width = display_width(&counts_text);
    let gap = width.saturating_sub(raw_width + counts_width);
    format!("{line}{}{}", " ".repeat(gap), palette.dim(&counts_text))
}

fn detail_line(
    candidate: Option<&Candidate>,
    palette: &Palette,
    options: &CleanupOptions,
    width: usize,
) -> String {
    let Some(candidate) = candidate else {
        return palette.dim(&truncate_right("  no candidate selected", width));
    };
    let path = compact_candidate_path(candidate);
    let text = format!(
        "{}  ·  {}  ·  {}  ·  {}",
        path,
        candidate.category.description(),
        candidate.reason,
        options.unit.format(candidate.size)
    );
    palette.dim(&truncate_left(&text, width))
}

fn hint_line(state: &State, palette: &Palette, width: usize) -> String {
    let text = if let Some(status) = &state.status {
        format!("  {status}  ·  space toggle  enter review  q quit")
    } else if width >= 92 {
        "↑↓ move  space toggle  a all  n none  s sort  f filter  enter review  q quit".to_owned()
    } else if width >= 62 {
        "↑↓ move  space toggle  a all  n none  f filter  enter review  q quit".to_owned()
    } else {
        "↑↓ move  space toggle  enter review  q quit".to_owned()
    };
    palette.dim(&truncate_right(&text, width))
}

fn deleting_frame(
    state: &State,
    options: &CleanupOptions,
    width: usize,
    height: usize,
) -> Vec<String> {
    let palette = Palette::new(options.color);
    let mut lines = vec![
        palette.title(&truncate_right("Cleaning up", width)),
        String::new(),
        truncate_right(
            &format!(
                "  Removing {} selected folder{}  ·  {}",
                state.selected_count(),
                if state.selected_count() == 1 { "" } else { "s" },
                options.unit.format(state.selected_size())
            ),
            width,
        )
        .into_owned(),
        palette.dim(&truncate_right(
            "  Large caches may take a moment; do not close the terminal.",
            width,
        )),
    ];
    while lines.len() + 1 < height {
        lines.push(String::new());
    }
    lines
}

fn confirm_frame(
    state: &State,
    palette: &Palette,
    options: &CleanupOptions,
    width: usize,
    height: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    let title = format!(
        "Confirm cleanup  ·  {} selected  ·  {}",
        state.selected_count(),
        options.unit.format(state.selected_size())
    );
    lines.push(palette.title(&truncate_right(&title, width)));
    let detail = format!(
        "This permanently removes {} from disk. Build caches will be recreated when needed.",
        if state.selected_count() == 1 {
            "1 folder"
        } else {
            "these folders"
        }
    );
    lines.push(palette.dim(&truncate_right(&detail, width)));
    lines.push(String::new());
    let selected: Vec<_> = state
        .candidates
        .iter()
        .filter(|candidate| candidate.selected)
        .collect();
    let room = height.saturating_sub(7).max(1);
    for candidate in selected.iter().take(room) {
        let line = format!(
            "  {}  {}",
            compact_candidate_path(candidate),
            options.unit.format(candidate.size)
        );
        lines.push(palette.dim(&truncate_left(&line, width)));
    }
    if selected.len() > room {
        lines.push(palette.dim(&format!("  … and {} more", selected.len() - room)));
    }
    while lines.len() + 2 < height {
        lines.push(String::new());
    }
    lines.push(palette.warning(&truncate_right(
        "  [y] delete permanently      [n / esc] cancel",
        width,
    )));
    lines
}

fn report_frame(
    report: &CleanupReport,
    palette: &Palette,
    options: &CleanupOptions,
    width: usize,
    height: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(palette.title(&truncate_right("Cleanup complete", width)));
    lines.push(String::new());
    lines.push(
        truncate_right(
            &format!(
                "  removed {} folder{}  ·  freed {}",
                report.deleted,
                if report.deleted == 1 { "" } else { "s" },
                options.unit.format(report.freed)
            ),
            width,
        )
        .into_owned(),
    );
    if report.errors.is_empty() {
        lines.push(palette.accent("  all selected folders were removed"));
    } else {
        lines.push(palette.warning(&format!(
            "  {} error{}",
            report.errors.len(),
            if report.errors.len() == 1 { "" } else { "s" }
        )));
        for error in report.errors.iter().take(height.saturating_sub(7)) {
            lines.push(palette.dim(&truncate_right(&format!("  {error}"), width)));
        }
    }
    while lines.len() + 2 < height {
        lines.push(String::new());
    }
    lines.push(palette.dim(&truncate_right("  press any key to exit", width)));
    lines
}

fn compact_candidate_path(candidate: &Candidate) -> String {
    sanitize_text(&candidate.display_path())
}

fn center(text: &str, width: usize) -> String {
    let text = truncate_right(text, width);
    let padding = width.saturating_sub(display_width(&text)) / 2;
    format!("{}{}", " ".repeat(padding), text)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn candidate(path: &str, size: u64, category: Category) -> Candidate {
        Candidate {
            path: PathBuf::from(path),
            root: PathBuf::from("/tmp"),
            size,
            category,
            language: "test".to_owned(),
            reason: "test cache".to_owned(),
            global: false,
            selected: category != Category::Dependency,
        }
    }

    fn state() -> State {
        State::new(vec![
            candidate("/tmp/target", 1000, Category::Build),
            candidate("/tmp/node_modules", 500, Category::Dependency),
            candidate("/tmp/__pycache__", 200, Category::Cache),
        ])
    }

    fn options() -> CleanupOptions {
        CleanupOptions {
            color: false,
            unit: SizeUnit::Binary,
            profile: "safe".to_owned(),
        }
    }

    #[test]
    fn toggles_and_selects_visible_rows() {
        let mut state = state();
        state.toggle_selected();
        assert!(!state.candidates[0].selected);
        state.select_visible(true);
        assert!(state.candidates.iter().all(|candidate| candidate.selected));
        state.select_visible(false);
        assert!(state.candidates.iter().all(|candidate| !candidate.selected));
    }

    #[test]
    fn filter_and_sort_keep_the_cursor_valid() {
        let mut state = state();
        state.filter = Filter::Dependency;
        state.clamp_cursor();
        assert_eq!(state.visible().len(), 1);
        state.cursor = 9;
        state.clamp_cursor();
        assert_eq!(state.cursor, 0);
        state.cycle_sort();
        assert_eq!(state.sort, SortMode::Name);
    }

    #[test]
    fn frame_fits_common_sizes() {
        let mut state = state();
        let opts = options();
        for (width, height) in [(40, 8), (60, 16), (80, 24), (120, 40)] {
            let frame = build_frame(&mut state, &opts, width, height);
            assert!(frame.len() <= height);
            for line in &frame {
                assert!(display_width(line) <= width, "{line:?} is too wide");
                assert!(!line.contains('\x1b'));
            }
        }
    }

    #[test]
    fn enter_requires_a_selection_and_confirmation_is_explicit() {
        let mut state = state();
        state.select_visible(false);
        assert!(!handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
        ));
        assert!(!state.confirm);
        state.select_visible(true);
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(state.confirm);
        handle_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE),
        );
        assert!(!state.confirm);
    }

    #[test]
    fn colored_frames_fit_without_ansi_counted_as_width() {
        fn strip_ansi(text: &str) -> String {
            let mut out = String::new();
            let mut chars = text.chars();
            while let Some(ch) = chars.next() {
                if ch == '\x1b' {
                    for next in chars.by_ref() {
                        if next == 'm' {
                            break;
                        }
                    }
                } else {
                    out.push(ch);
                }
            }
            out
        }
        let mut state = state();
        let opts = CleanupOptions {
            color: true,
            unit: SizeUnit::Binary,
            profile: "safe".to_owned(),
        };
        for (width, height) in [(40, 8), (60, 16), (92, 24), (120, 40)] {
            let frame = build_frame(&mut state, &opts, width, height);
            for line in &frame {
                let visible = strip_ansi(line);
                assert!(display_width(&visible) <= width, "{line:?} is too wide");
            }
        }
        state.confirm = true;
        let frame = build_frame(&mut state, &opts, 60, 16);
        for line in &frame {
            let visible = strip_ansi(line);
            assert!(display_width(&visible) <= 60, "{line:?} is too wide");
        }
    }

    #[test]
    fn huge_candidate_sizes_still_fit_narrow_frames() {
        let mut state = State::new(vec![candidate("/tmp/huge", u64::MAX, Category::Build)]);
        let opts = options();
        for (width, height) in [(40, 8), (50, 10), (80, 24)] {
            let frame = build_frame(&mut state, &opts, width, height);
            assert!(frame.len() <= height);
            for line in &frame {
                assert!(display_width(line) <= width, "{line:?} is too wide");
            }
        }
    }

    #[test]
    fn confirm_and_report_frames_fit_narrow_terminals() {
        let mut state = state();
        state.confirm = true;
        let opts = options();
        for (width, height) in [(40, 8), (50, 10), (80, 24)] {
            let frame = build_frame(&mut state, &opts, width, height);
            assert!(frame.len() <= height);
            for line in &frame {
                assert!(display_width(line) <= width, "{line:?} is too wide");
            }
        }
        state.confirm = false;
        state.report = Some(CleanupReport {
            deleted: 2,
            freed: 4096,
            errors: vec!["one failed path".to_owned()],
        });
        for (width, height) in [(40, 8), (80, 24)] {
            let frame = build_frame(&mut state, &opts, width, height);
            assert!(frame.len() <= height);
            for line in &frame {
                assert!(display_width(line) <= width, "{line:?} is too wide");
            }
        }
    }

    #[test]
    fn confirm_frame_does_not_exceed_height() {
        let mut state = state();
        state.confirm = true;
        let opts = options();
        let frame = build_frame(&mut state, &opts, 80, 24);
        assert!(frame.len() <= 24);
        assert!(frame.iter().any(|line| line.contains("[y]")));
    }
}
