use std::fmt::Write as _;
use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::queue;
use crossterm::terminal::{self, BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate};

use crate::SortKey;
use crate::format::{
    Palette, SizeUnit, bar_cells, bar_text, bar_units, count_noun, display_width, percent,
    sanitize_text, truncate_left, truncate_right,
};
use crate::scan::{Metric, NodeId, Tree};

#[derive(Clone, Copy, Debug)]
pub struct TuiOptions {
    pub metric: Metric,
    pub unit: SizeUnit,
    pub color: bool,
    pub plain: bool,
    pub sort: SortKey,
    pub reverse: bool,
}

use crate::term::TerminalGuard;

struct HistoryEntry {
    current: NodeId,
    selected: Option<NodeId>,
    offset: usize,
}

struct State {
    current: NodeId,
    cursor: usize,
    offset: usize,
    page_rows: usize,
    items: Vec<NodeId>,
    sort: SortKey,
    reverse: bool,
    metric: Metric,
    history: Vec<HistoryEntry>,
}

impl State {
    fn new(tree: &Tree, options: &TuiOptions) -> Self {
        let mut state = Self {
            current: tree.root,
            cursor: 0,
            offset: 0,
            page_rows: 10,
            items: Vec::new(),
            sort: options.sort,
            reverse: options.reverse,
            metric: options.metric,
            history: Vec::new(),
        };

        if tree.node(tree.root).children.len() == 1 {
            let only = tree.node(tree.root).children[0];
            if tree.node(only).is_dir {
                state.history.push(HistoryEntry {
                    current: tree.root,
                    selected: Some(only),
                    offset: 0,
                });
                state.current = only;
            }
        }

        state.refresh(tree);
        state
    }

    fn selected(&self) -> Option<NodeId> {
        self.items.get(self.cursor).copied()
    }

    fn refresh(&mut self, tree: &Tree) {
        self.items = tree.sorted_children(self.current, self.sort, self.reverse, self.metric);
        if self.items.is_empty() {
            self.cursor = 0;
            self.offset = 0;
        } else if self.cursor >= self.items.len() {
            self.cursor = self.items.len() - 1;
        }
    }

    fn refresh_keep_selection(&mut self, tree: &Tree) {
        let selected = self.selected();
        self.refresh(tree);
        if let Some(selected) = selected
            && let Some(index) = self.items.iter().position(|&id| id == selected)
        {
            self.cursor = index;
        }
    }

    fn move_by(&mut self, delta: isize) {
        if self.items.is_empty() {
            self.cursor = 0;
            return;
        }
        if delta < 0 {
            self.cursor = self.cursor.saturating_sub(delta.unsigned_abs());
        } else {
            self.cursor = self
                .cursor
                .saturating_add(delta as usize)
                .min(self.items.len() - 1);
        }
    }

    fn page_by(&mut self, rows: usize, down: bool) {
        if self.items.is_empty() {
            self.cursor = 0;
            self.offset = 0;
            return;
        }
        if down {
            self.cursor = self.cursor.saturating_add(rows).min(self.items.len() - 1);
        } else {
            self.cursor = self.cursor.saturating_sub(rows);
        }
        self.offset = self.cursor;
    }

    fn enter(&mut self, tree: &Tree) {
        let Some(selected) = self.selected() else {
            return;
        };
        if !tree.node(selected).is_dir {
            return;
        }
        self.history.push(HistoryEntry {
            current: self.current,
            selected: Some(selected),
            offset: self.offset,
        });
        self.current = selected;
        self.cursor = 0;
        self.offset = 0;
        self.refresh(tree);
    }

    fn back(&mut self, tree: &Tree) -> bool {
        let Some(entry) = self.history.pop() else {
            return false;
        };
        self.current = entry.current;
        self.refresh(tree);
        if let Some(selected) = entry.selected
            && let Some(index) = self.items.iter().position(|&id| id == selected)
        {
            self.cursor = index;
        }
        self.offset = entry.offset;
        true
    }

    fn toggle_sort(&mut self, tree: &Tree) {
        self.sort = match self.sort {
            SortKey::Size => SortKey::Name,
            SortKey::Name => SortKey::Size,
        };
        self.refresh_keep_selection(tree);
    }

    fn toggle_reverse(&mut self, tree: &Tree) {
        self.reverse = !self.reverse;
        self.refresh_keep_selection(tree);
    }

    fn toggle_metric(&mut self, tree: &Tree) {
        self.metric = match self.metric {
            Metric::Disk => Metric::Apparent,
            Metric::Apparent => Metric::Disk,
        };
        self.refresh_keep_selection(tree);
    }

    fn ensure_visible(&mut self, rows: usize) {
        if rows == 0 || self.items.is_empty() {
            self.offset = 0;
            return;
        }
        if self.cursor < self.offset {
            self.offset = self.cursor;
        }
        if self.cursor >= self.offset.saturating_add(rows) {
            self.offset = self.cursor.saturating_add(1).saturating_sub(rows);
        }
        let max_offset = self.items.len().saturating_sub(rows);
        self.offset = self.offset.min(max_offset);
    }
}

pub fn run(tree: &Tree, options: TuiOptions) -> io::Result<()> {
    let _guard = TerminalGuard::enter()?;
    let mut state = State::new(tree, &options);

    loop {
        draw(tree, &mut state, &options)?;
        let event = event::read()?;
        match event {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                if handle_key(tree, &mut state, key) {
                    break;
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }

    Ok(())
}

fn handle_key(tree: &Tree, state: &mut State, key: KeyEvent) -> bool {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
    {
        return true;
    }

    match key.code {
        KeyCode::Char('q') => return true,
        KeyCode::Esc => {
            if !state.back(tree) {
                return true;
            }
        }
        KeyCode::Up | KeyCode::Char('k') => state.move_by(-1),
        KeyCode::Down | KeyCode::Char('j') => state.move_by(1),
        KeyCode::PageUp => state.page_by(state.page_rows, false),
        KeyCode::PageDown => state.page_by(state.page_rows, true),
        KeyCode::Home | KeyCode::Char('g') => state.cursor = 0,
        KeyCode::End | KeyCode::Char('G') => {
            state.cursor = state.items.len().saturating_sub(1);
        }
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => state.enter(tree),
        KeyCode::Backspace | KeyCode::Char('h') | KeyCode::Left => {
            state.back(tree);
        }
        KeyCode::Char('s') => state.toggle_sort(tree),
        KeyCode::Char('S') => state.toggle_reverse(tree),
        KeyCode::Char('a') => state.toggle_metric(tree),
        _ => {}
    }
    false
}

fn draw(tree: &Tree, state: &mut State, options: &TuiOptions) -> io::Result<()> {
    let (width, height) = terminal::size()?;
    let width = width as usize;
    let height = height as usize;
    let lines = build_frame(tree, state, options, width, height);

    let mut out = io::stdout().lock();
    queue!(out, BeginSynchronizedUpdate)?;
    let draw_result = (|| -> io::Result<()> {
        queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
        for (index, line) in lines.iter().take(height).enumerate() {
            queue!(out, MoveTo(0, index as u16))?;
            out.write_all(line.as_bytes())?;
        }
        Ok(())
    })();
    let end_result = queue!(out, EndSynchronizedUpdate);
    let flush_result = out.flush();
    draw_result?;
    end_result?;
    flush_result
}

fn build_frame(
    tree: &Tree,
    state: &mut State,
    options: &TuiOptions,
    width: usize,
    height: usize,
) -> Vec<String> {
    if width < 40 || height < 8 {
        let mut lines = Vec::new();
        if height > 0 {
            lines.push(center(
                &format!("terminal too small ({width}x{height})"),
                width,
            ));
        }
        if height > 1 {
            lines.push(center("need at least 40x8", width));
        }
        if height > 2 {
            lines.push(center("q quit", width));
        }
        return lines;
    }

    let palette = Palette::new(options.color && !options.plain);
    let current = tree.node(state.current);
    let total = current.size(state.metric);
    let list_rows = height.saturating_sub(5);
    state.page_rows = list_rows.max(1);
    state.ensure_visible(list_rows);

    let mut lines = Vec::with_capacity(height);
    lines.push(header_path_line(tree, state, &palette, options, width));
    lines.push(header_stats_line(tree, state, &palette, width, list_rows));
    let rule = if options.plain { "-" } else { "─" };
    lines.push(palette.dim(&rule.repeat(width)));

    if state.items.is_empty() {
        lines.push(palette.dim("  empty"));
    } else {
        let size_width = display_width(&options.unit.format(total)).max(7);
        let visible_end = state
            .offset
            .saturating_add(list_rows)
            .min(state.items.len());

        if options.plain {
            let marker_width = 2;
            let name_width = width.saturating_sub(marker_width + size_width + 2);
            for (index, &item) in state
                .items
                .iter()
                .enumerate()
                .take(visible_end)
                .skip(state.offset)
            {
                let node = tree.node(item);
                let marker = if index == state.cursor { "> " } else { "  " };
                let raw_name = entry_name(node);
                let name = truncate_right(&raw_name, name_width);
                let mut line = String::new();
                let _ = write!(
                    line,
                    "{marker}{:>size_width$}  {name}",
                    options.unit.format(node.size(state.metric)),
                );
                lines.push(line);
            }
        } else {
            let marker_width = 2;
            let share_width = 4;
            let base_width = marker_width + size_width + 2 + share_width + 2;
            let desired_bar = if width >= 110 {
                20
            } else if width >= 90 {
                16
            } else if width >= 70 {
                12
            } else if width >= 50 {
                8
            } else {
                6
            };
            let bar_width = if width > base_width + 16 {
                desired_bar.min(width - base_width - 12).max(4)
            } else {
                0
            };
            let name_width =
                width.saturating_sub(base_width + if bar_width > 0 { bar_width + 2 } else { 0 });

            for (index, &item) in state
                .items
                .iter()
                .enumerate()
                .take(visible_end)
                .skip(state.offset)
            {
                let node = tree.node(item);
                let size = node.size(state.metric);
                let size_text = options.unit.format(size);
                let fraction = if total == 0 {
                    0.0
                } else {
                    (size as f64 / total as f64).clamp(0.0, 1.0)
                };
                let units = bar_units(size, total, bar_width);
                let bar = format!(
                    "{}{}",
                    palette.bar(&bar_text(units, bar_width), fraction),
                    palette.dim(&"░".repeat(bar_width - bar_cells(units)))
                );
                let selected = index == state.cursor;
                let marker = if selected {
                    palette.accent("❯ ")
                } else {
                    "  ".to_string()
                };
                let raw_name = entry_name(node);
                let name = truncate_right(&raw_name, name_width);
                let styled_name = if node.errors > 0 {
                    palette.warning(&name)
                } else if node.is_dir {
                    palette.directory(&name)
                } else if node.is_symlink {
                    palette.link(&name)
                } else {
                    palette.file(&name)
                };
                let styled_name = if selected {
                    palette.bold(&styled_name)
                } else {
                    styled_name
                };

                let mut line = String::new();
                if bar_width > 0 {
                    let _ = write!(
                        line,
                        "{marker}{:>size_width$}  {:>share_width$}  {bar}  {styled_name}",
                        size_text,
                        percent(size, total),
                    );
                } else {
                    let _ = write!(
                        line,
                        "{marker}{:>size_width$}  {:>share_width$}  {styled_name}",
                        size_text,
                        percent(size, total),
                    );
                }
                lines.push(line);
            }
        }
    }

    let selected = state.selected().map(|id| tree.node(id));
    lines.push(detail_line(selected, &palette, width));
    lines.push(palette.dim(&key_hint(width)));
    lines.truncate(height);
    lines
}

fn entry_name(node: &crate::scan::FsNode) -> String {
    let mut name = sanitize_text(&node.name);
    if node.is_dir && !name.ends_with('/') {
        name.push('/');
    }
    if node.is_symlink {
        name.push('@');
    }
    if node.deduplicated {
        name.push_str(" (hardlink)");
    }
    name
}

fn header_path_line(
    tree: &Tree,
    state: &State,
    palette: &Palette,
    options: &TuiOptions,
    width: usize,
) -> String {
    let current = tree.node(state.current);
    let path = if state.current == tree.root {
        format!("all paths ({})", tree.node(tree.root).children.len())
    } else {
        sanitize_text(&current.path.display().to_string())
    };
    let size_text = options.unit.format(current.size(state.metric));
    let right_width = display_width(&size_text);
    let left_available = width.saturating_sub(right_width + 1);
    let title_width = 6;
    let path_available = left_available.saturating_sub(title_width + 2);
    let path = truncate_left(&path, path_available);
    let path_width = display_width(&path);
    let left = format!("{}  {}", palette.title("dulens"), palette.dim(&path));
    let left_width = title_width + 2 + path_width;
    let gap = width.saturating_sub(left_width + right_width);
    format!("{left}{}{}", " ".repeat(gap), palette.accent(&size_text))
}

fn header_stats_line(
    tree: &Tree,
    state: &State,
    palette: &Palette,
    width: usize,
    list_rows: usize,
) -> String {
    let current = tree.node(state.current);
    let counts = format!(
        "{}  ·  {}",
        count_noun(current.files, "file", "files"),
        count_noun(current.dirs, "dir", "dirs")
    );
    let errors = if tree.error_count > 0 {
        format!("  ·  {}", count_noun(tree.error_count, "error", "errors"))
    } else {
        String::new()
    };
    let sort_arrow = match (state.sort, state.reverse) {
        (SortKey::Size, false) => "↓",
        (SortKey::Size, true) => "↑",
        (SortKey::Name, false) => "↑",
        (SortKey::Name, true) => "↓",
    };
    let metric = state.metric.label();
    let sort_name = match state.sort {
        SortKey::Size => "size",
        SortKey::Name => "name",
    };
    let base_right = format!(" · {sort_name} {sort_arrow}");
    let position = if state.items.len() > list_rows && !state.items.is_empty() {
        let candidate = format!(" · {}/{}", state.cursor + 1, state.items.len());
        if display_width(metric) + display_width(&base_right) + display_width(&candidate) + 1
            < width
        {
            candidate
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    let right_detail = format!("{base_right}{position}");
    let right_width = display_width(metric) + display_width(&right_detail);
    let left_available = width.saturating_sub(right_width + 1);
    let counts = truncate_right(&counts, left_available);
    let errors = truncate_right(
        &errors,
        left_available.saturating_sub(display_width(&counts)),
    );
    let left_width = display_width(&counts) + display_width(&errors);
    let left = format!("{}{}", palette.dim(&counts), palette.warning(&errors));
    let gap = width.saturating_sub(left_width + right_width);
    let right = format!("{}{}", palette.accent(metric), palette.dim(&right_detail));
    format!("{left}{}{}", " ".repeat(gap), right)
}

fn detail_line(node: Option<&crate::scan::FsNode>, palette: &Palette, width: usize) -> String {
    let Some(node) = node else {
        return palette.dim(&truncate_right("  select an entry", width));
    };
    let mut detail = sanitize_text(&node.path.display().to_string());
    if node.deduplicated {
        detail.push_str("  ·  shared hard link");
    } else if node.is_dir {
        let _ = write!(
            detail,
            "  ·  {}  ·  {}",
            count_noun(node.files, "file", "files"),
            count_noun(node.dirs, "dir", "dirs")
        );
    } else if node.is_symlink {
        detail.push_str("  ·  symbolic link");
    }
    if node.errors > 0 {
        let _ = write!(
            detail,
            "  ·  {}",
            count_noun(node.errors, "error", "errors")
        );
    }
    let detail = truncate_left(&detail, width);
    palette.dim(&detail)
}

fn key_hint(width: usize) -> String {
    let text = if width >= 100 {
        "↑↓/jk move  ↵/l open  ⌫/h up  s sort  S reverse  a metric  q quit"
    } else if width >= 70 {
        "↑↓ move  ↵ open  ⌫ up  s sort  a metric  q quit"
    } else {
        "↑↓ move  ↵ open  q quit"
    };
    truncate_right(text, width).into_owned()
}

fn center(text: &str, width: usize) -> String {
    let text = truncate_right(text, width);
    let padding = width.saturating_sub(display_width(&text)) / 2;
    format!("{}{}", " ".repeat(padding), text)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::scan::{ScanOptions, Scanner};

    fn fixture() -> (TempDir, Tree, TuiOptions) {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("alpha")).unwrap();
        fs::write(temp.path().join("alpha/a.bin"), vec![b'a'; 4096]).unwrap();
        fs::create_dir(temp.path().join("beta")).unwrap();
        fs::write(temp.path().join("beta/b.bin"), vec![b'b'; 2048]).unwrap();
        fs::write(temp.path().join("file.bin"), vec![b'c'; 1024]).unwrap();

        let tree = Scanner::new(ScanOptions::default()).scan(&[temp.path().to_path_buf()]);
        let options = TuiOptions {
            metric: Metric::Apparent,
            unit: SizeUnit::Binary,
            color: false,
            plain: false,
            sort: SortKey::Size,
            reverse: false,
        };
        (temp, tree, options)
    }

    #[test]
    fn starts_inside_a_single_root_and_navigates() {
        let (_temp, tree, options) = fixture();
        let mut state = State::new(&tree, &options);

        assert_ne!(state.current, tree.root);
        assert!(!state.items.is_empty());

        let selected = state.selected().unwrap();
        if tree.node(selected).is_dir {
            state.enter(&tree);
            assert_eq!(state.current, selected);
            assert!(state.back(&tree));
        }
    }

    #[test]
    fn toggles_sort_and_metric_without_losing_selection() {
        let (_temp, tree, options) = fixture();
        let mut state = State::new(&tree, &options);
        let selected = state.selected().unwrap();

        state.toggle_sort(&tree);
        assert_eq!(state.sort, SortKey::Name);
        assert_eq!(state.selected(), Some(selected));

        state.toggle_reverse(&tree);
        assert!(state.reverse);
        assert_eq!(state.selected(), Some(selected));

        state.toggle_metric(&tree);
        assert_eq!(state.metric, Metric::Disk);
        assert_eq!(state.selected(), Some(selected));
    }

    #[test]
    fn builds_a_frame_that_fits_the_terminal() {
        let (_temp, tree, options) = fixture();
        let mut state = State::new(&tree, &options);
        let frame = build_frame(&tree, &mut state, &options, 80, 24);

        assert!(frame.len() <= 24);
        assert!(frame.iter().any(|line| line.contains("dulens")));
        assert!(frame.iter().any(|line| line.contains("q quit")));
        assert!(frame.iter().any(|line| line.contains("alpha/")));
        for line in &frame {
            assert!(
                display_width(line) <= 80,
                "line is too wide: {line:?} ({})",
                display_width(line)
            );
            assert!(!line.contains('\x1b'));
        }
    }

    #[test]
    fn plain_tui_has_no_decoration() {
        let (_temp, tree, mut options) = fixture();
        options.plain = true;
        options.color = true;
        let mut state = State::new(&tree, &options);
        let frame = build_frame(&tree, &mut state, &options, 80, 24);

        assert!(frame.iter().any(|line| line.contains("alpha/")));
        assert!(frame.iter().any(|line| line.starts_with("> ")));
        for line in &frame {
            assert!(!line.contains('\x1b'));
            assert!(!line.contains('%'));
            assert!(!line.contains('█'));
            assert!(!line.contains('░'));
            assert!(!line.contains('─'));
            assert!(display_width(line) <= 80);
        }
    }

    #[test]
    fn common_terminal_sizes_fit() {
        let (_temp, tree, options) = fixture();
        for width in [40, 41, 50, 60, 70, 80, 90, 100, 120] {
            for height in [8, 10, 16, 24] {
                let mut state = State::new(&tree, &options);
                let frame = build_frame(&tree, &mut state, &options, width, height);
                assert!(frame.len() <= height);
                for line in &frame {
                    assert!(
                        display_width(line) <= width,
                        "line is too wide for {width}x{height}: {line:?} ({})",
                        display_width(line)
                    );
                }
            }
        }
    }

    #[test]
    fn huge_raw_sizes_still_fit() {
        let (_temp, mut tree, mut options) = fixture();
        let root = tree.node(tree.root).children[0];
        let file = tree.node(root).children[0];
        tree.nodes[root].apparent = u64::MAX;
        tree.nodes[file].apparent = u64::MAX;
        options.unit = SizeUnit::Bytes;
        let mut state = State::new(&tree, &options);
        let frame = build_frame(&tree, &mut state, &options, 40, 16);

        for line in &frame {
            assert!(
                display_width(line) <= 40,
                "line is too wide: {line:?} ({})",
                display_width(line)
            );
        }
    }

    #[test]
    fn large_error_counts_still_fit() {
        let (_temp, mut tree, options) = fixture();
        tree.error_count = 9_999_999_999;
        let mut state = State::new(&tree, &options);
        let frame = build_frame(&tree, &mut state, &options, 40, 12);

        for line in &frame {
            assert!(
                display_width(line) <= 40,
                "line is too wide: {line:?} ({})",
                display_width(line)
            );
        }
    }

    #[test]
    fn narrow_frames_still_fit() {
        let (_temp, tree, options) = fixture();
        let mut state = State::new(&tree, &options);
        let frame = build_frame(&tree, &mut state, &options, 40, 10);

        for line in &frame {
            assert!(
                display_width(line) <= 40,
                "line is too wide: {line:?} ({})",
                display_width(line)
            );
        }
    }

    #[test]
    fn tiny_terminal_shows_a_hint() {
        let (_temp, tree, options) = fixture();
        let mut state = State::new(&tree, &options);
        let frame = build_frame(&tree, &mut state, &options, 20, 5);

        assert!(frame.iter().any(|line| line.contains("too small")));
    }
}
