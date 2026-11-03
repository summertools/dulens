use std::fmt::Write as _;

use crate::SortKey;
use crate::format::{
    Palette, SizeUnit, bar_cells, bar_text, bar_units, count_noun, display_width, percent,
    sanitize_text, truncate_left, truncate_right,
};
use crate::scan::{FsNode, Metric, NodeId, Tree};

#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub depth: usize,
    pub all: bool,
    pub metric: Metric,
    pub unit: SizeUnit,
    pub color: bool,
    pub plain: bool,
    pub width: usize,
    pub sort: SortKey,
    pub reverse: bool,
}

struct Row {
    id: NodeId,
    depth: usize,
    prefix: String,
    parent_size: u64,
    has_more: bool,
    is_root: bool,
}

pub fn render(tree: &Tree, options: &RenderOptions) -> String {
    if options.plain {
        return render_plain(tree, options);
    }

    let (rows, total_size) = collect_rows(tree, options);
    let palette = Palette::new(options.color);
    let width = options.width.max(1);

    let mut out = String::new();
    let root = tree.node(tree.root);
    let size_text = options.unit.format(total_size);
    let mut summary = format!(
        "  ·  {}  ·  {}",
        count_noun(root.files, "file", "files"),
        count_noun(root.dirs, "dir", "dirs")
    );
    if root.children.len() > 1 {
        let _ = write!(
            summary,
            "  ·  {}",
            count_noun(root.children.len() as u64, "root", "roots")
        );
    }
    if options.metric == Metric::Apparent {
        summary.push_str("  ·  apparent");
    }
    let errors = if tree.error_count > 0 {
        format!("  ·  {}", count_noun(tree.error_count, "error", "errors"))
    } else {
        String::new()
    };
    let plain_header = format!("dulens  {size_text}{summary}{errors}");
    if display_width(&plain_header) <= width {
        out.push_str(&palette.title("dulens"));
        out.push_str("  ");
        out.push_str(&palette.accent(&size_text));
        out.push_str(&palette.dim(&summary));
        out.push_str(&palette.warning(&errors));
    } else {
        out.push_str(&palette.bold(&truncate_right(&plain_header, width)));
    }
    out.push_str("\n\n");

    if rows.is_empty() {
        return out;
    }

    let sizes: Vec<String> = rows
        .iter()
        .map(|row| options.unit.format(tree.node(row.id).size(options.metric)))
        .collect();
    let shares: Vec<String> = rows
        .iter()
        .map(|row| percent(tree.node(row.id).size(options.metric), row.parent_size))
        .collect();
    let size_width = sizes
        .iter()
        .map(|text| text.len())
        .max()
        .unwrap_or(1)
        .max(7);
    let share_width = shares.iter().map(|text| text.len()).max().unwrap_or(1);
    let desired_bar = if width >= 120 {
        24
    } else if width >= 90 {
        18
    } else if width >= 70 {
        12
    } else {
        8
    };
    let min_name_width = 8;
    let size_block = size_width + 2;
    let share_block = share_width + 2;
    let show_share = width >= size_block + share_block + min_name_width;
    let base_before_bar = size_block + if show_share { share_block } else { 0 };
    let available_for_bar = width.saturating_sub(base_before_bar + min_name_width);
    let bar_width = if show_share && available_for_bar >= 6 {
        desired_bar.min(available_for_bar - 2)
    } else {
        0
    };
    let columns_width = base_before_bar + if bar_width > 0 { bar_width + 2 } else { 0 };
    let name_width = width.saturating_sub(columns_width);
    let prefixed_name_width = 8;
    let flat_depth = rows
        .iter()
        .filter_map(|row| {
            let prefix_width = display_width(&row.prefix);
            (name_width <= prefix_width + prefixed_name_width).then_some(row.depth)
        })
        .min();

    for (index, row) in rows.iter().enumerate() {
        let node = tree.node(row.id);
        let size = node.size(options.metric);
        let fraction = if row.parent_size == 0 {
            0.0
        } else {
            (size as f64 / row.parent_size as f64).clamp(0.0, 1.0)
        };
        let units = bar_units(size, row.parent_size, bar_width);
        let filled_bar = palette.bar(&bar_text(units, bar_width), fraction);
        let remainder_bar = palette.dim(&"░".repeat(bar_width - bar_cells(units)));

        let mut name = display_name(node);
        if row.has_more {
            name.push_str(" …");
        }

        let prefix_width = display_width(&row.prefix);
        let (shown_prefix, name_width) = if flat_depth.is_some_and(|depth| row.depth >= depth)
            || name_width <= prefix_width + prefixed_name_width
        {
            ("", name_width)
        } else {
            (row.prefix.as_str(), name_width - prefix_width)
        };
        let shown_name = if row.is_root {
            truncate_left(&name, name_width)
        } else {
            truncate_right(&name, name_width)
        };
        let styled_name = if node.errors > 0 {
            palette.warning(&shown_name)
        } else if node.is_dir {
            palette.directory(&shown_name)
        } else if node.is_symlink {
            palette.link(&shown_name)
        } else {
            palette.file(&shown_name)
        };

        if bar_width > 0 {
            let _ = writeln!(
                out,
                "{:>size_width$}  {:>share_width$}  {}{}  {}{}",
                sizes[index],
                shares[index],
                filled_bar,
                remainder_bar,
                palette.dim(shown_prefix),
                styled_name,
            );
        } else if show_share {
            let _ = writeln!(
                out,
                "{:>size_width$}  {:>share_width$}  {}{}",
                sizes[index],
                shares[index],
                palette.dim(shown_prefix),
                styled_name,
            );
        } else {
            let _ = writeln!(
                out,
                "{:>size_width$}  {}{}",
                sizes[index],
                palette.dim(shown_prefix),
                styled_name,
            );
        }
    }

    out
}

fn collect_rows(tree: &Tree, options: &RenderOptions) -> (Vec<Row>, u64) {
    let roots = tree.sorted_children(tree.root, options.sort, options.reverse, options.metric);
    let total_size = tree.node(tree.root).size(options.metric);
    let mut rows = Vec::new();
    for &root in &roots {
        collect(
            tree, root, 0, "", true, false, total_size, options, &mut rows,
        );
    }
    (rows, total_size)
}

fn render_plain(tree: &Tree, options: &RenderOptions) -> String {
    let (rows, _) = collect_rows(tree, options);
    if rows.is_empty() {
        return String::new();
    }

    let width = options.width.max(1);
    let sizes: Vec<String> = rows
        .iter()
        .map(|row| options.unit.format(tree.node(row.id).size(options.metric)))
        .collect();
    let size_width = sizes
        .iter()
        .map(|text| display_width(text))
        .max()
        .unwrap_or(1);
    let name_budget = width.saturating_sub(size_width + 2);
    let prefixed_name_width = 8;
    let flat_depth = rows
        .iter()
        .filter_map(|row| {
            let indent_width = row.depth * 2;
            (name_budget <= indent_width + prefixed_name_width).then_some(row.depth)
        })
        .min();

    let mut out = String::new();
    for (index, row) in rows.iter().enumerate() {
        let node = tree.node(row.id);
        let indent = "  ".repeat(row.depth);
        let indent_width = row.depth * 2;
        let (shown_indent, name_width) = if flat_depth.is_some_and(|depth| row.depth >= depth)
            || name_budget <= indent_width + prefixed_name_width
        {
            ("", name_budget)
        } else {
            (indent.as_str(), name_budget - indent_width)
        };

        let mut name = display_name(node);
        if row.has_more {
            name.push_str(" …");
        }
        let shown_name = if row.is_root {
            truncate_left(&name, name_width)
        } else {
            truncate_right(&name, name_width)
        };
        let _ = writeln!(
            out,
            "{:>size_width$}  {}{}",
            sizes[index], shown_indent, shown_name,
        );
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn collect(
    tree: &Tree,
    id: NodeId,
    depth: usize,
    prefix: &str,
    is_root: bool,
    is_last: bool,
    parent_size: u64,
    options: &RenderOptions,
    rows: &mut Vec<Row>,
) {
    let connector = if is_root {
        ""
    } else if is_last {
        "└─ "
    } else {
        "├─ "
    };
    let line_prefix = format!("{prefix}{connector}");
    let children_prefix = if is_root {
        String::new()
    } else if is_last {
        format!("{prefix}   ")
    } else {
        format!("{prefix}│  ")
    };

    let node = tree.node(id);
    let has_more = depth >= options.depth
        && node.children.iter().any(|&child| {
            let child = tree.node(child);
            options.all || child.is_dir
        });
    rows.push(Row {
        id,
        depth,
        prefix: line_prefix,
        parent_size,
        has_more,
        is_root,
    });

    if depth >= options.depth || !node.is_dir {
        return;
    }

    let mut children = tree.sorted_children(id, options.sort, options.reverse, options.metric);
    if !options.all {
        children.retain(|&child| tree.node(child).is_dir);
    }
    let count = children.len();
    for (index, child) in children.into_iter().enumerate() {
        let last = index + 1 == count;
        collect(
            tree,
            child,
            depth + 1,
            &children_prefix,
            false,
            last,
            node.size(options.metric),
            options,
            rows,
        );
    }
}

fn display_name(node: &FsNode) -> String {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::scan::{ScanOptions, Scanner};

    fn fixture() -> (TempDir, Tree) {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("large")).unwrap();
        fs::write(temp.path().join("large/a.bin"), vec![b'a'; 4096]).unwrap();
        fs::create_dir(temp.path().join("small")).unwrap();
        fs::write(temp.path().join("small/b.bin"), vec![b'b'; 128]).unwrap();
        fs::write(temp.path().join("root.bin"), vec![b'r'; 256]).unwrap();
        let tree = Scanner::new(ScanOptions::default()).scan(&[temp.path().to_path_buf()]);
        (temp, tree)
    }

    fn options() -> RenderOptions {
        RenderOptions {
            depth: 2,
            all: true,
            metric: Metric::Apparent,
            unit: SizeUnit::Binary,
            color: false,
            plain: false,
            width: 100,
            sort: SortKey::Size,
            reverse: false,
        }
    }

    #[test]
    fn plain_mode_is_clean_and_monochrome() {
        let (_temp, tree) = fixture();
        let output = render(
            &tree,
            &RenderOptions {
                plain: true,
                color: true,
                ..options()
            },
        );

        assert!(!output.starts_with("dulens"));
        assert!(!output.contains('%'));
        assert!(!output.contains('█'));
        assert!(!output.contains('░'));
        assert!(!output.contains('├'));
        assert!(!output.contains('└'));
        assert!(!output.contains('│'));
        assert!(!output.contains('\x1b'));
        assert!(output.contains("large/"));
        assert!(output.contains("a.bin"));
        assert!(output.lines().count() > 1);
    }

    #[test]
    fn plain_mode_respects_depth_files_and_width() {
        let (_temp, tree) = fixture();
        for width in [20, 40, 80] {
            let output = render(
                &tree,
                &RenderOptions {
                    plain: true,
                    width,
                    depth: 1,
                    all: false,
                    ..options()
                },
            );
            assert!(output.contains("large/"));
            assert!(!output.contains("a.bin"));
            for line in output.lines() {
                assert!(
                    display_width(line) <= width,
                    "line is too wide for {width}: {line:?} ({})",
                    display_width(line)
                );
            }
        }
    }

    #[test]
    fn renders_tree_and_header() {
        let (_temp, tree) = fixture();
        let output = render(&tree, &options());

        assert!(output.starts_with("dulens"));
        assert!(output.contains("large/"));
        assert!(output.contains("a.bin"));
        assert!(output.contains("root.bin"));
        assert!(output.contains("├─ "));
        assert!(output.contains("└─ "));
        assert!(!output.contains('\x1b'));
        assert!(output.find("large/").unwrap() < output.find("small/").unwrap());
    }

    #[test]
    fn hides_files_unless_requested() {
        let (_temp, tree) = fixture();
        let output = render(
            &tree,
            &RenderOptions {
                all: false,
                ..options()
            },
        );

        assert!(output.contains("large/"));
        assert!(!output.contains("root.bin"));
    }

    #[test]
    fn respects_depth_and_marks_truncation() {
        let (_temp, tree) = fixture();
        let output = render(
            &tree,
            &RenderOptions {
                depth: 0,
                ..options()
            },
        );

        assert!(output.contains(" …"));
        assert!(!output.contains("a.bin"));
    }

    #[test]
    fn output_fits_requested_width_with_wide_names() {
        let temp = TempDir::new().unwrap();
        fs::write(
            temp.path().join("一个非常非常长的中文文件名-for-width.bin"),
            vec![b'x'; 1024],
        )
        .unwrap();
        let tree = Scanner::new(ScanOptions::default()).scan(&[temp.path().to_path_buf()]);
        let output = render(
            &tree,
            &RenderOptions {
                width: 52,
                ..options()
            },
        );

        for line in output.lines() {
            assert!(
                display_width(line) <= 52,
                "line is too wide: {line:?} ({})",
                display_width(line)
            );
        }
    }

    #[test]
    fn output_fits_narrow_terminals() {
        let (_temp, tree) = fixture();
        for width in [10, 20, 30, 40, 60] {
            let output = render(&tree, &RenderOptions { width, ..options() });
            for line in output.lines() {
                assert!(
                    display_width(line) <= width,
                    "line is too wide for {width}: {line:?} ({})",
                    display_width(line)
                );
            }
        }
    }

    #[test]
    fn all_common_widths_fit() {
        let (_temp, tree) = fixture();
        for width in 10..=120 {
            let output = render(&tree, &RenderOptions { width, ..options() });
            for line in output.lines() {
                assert!(
                    display_width(line) <= width,
                    "line is too wide for {width}: {line:?} ({})",
                    display_width(line)
                );
            }
        }
    }

    #[test]
    fn keeps_names_visible_with_huge_raw_sizes() {
        let (_temp, mut tree) = fixture();
        let root = tree.node(tree.root).children[0];
        let file = tree.node(root).children[0];
        tree.nodes[file].name = "sparse".to_string();
        tree.nodes[file].apparent = 1_000_000_000_000_000;
        tree.nodes[root].apparent = 1_000_000_000_000_000;
        tree.nodes[tree.root].apparent = 1_000_000_000_000_000;

        let output = render(
            &tree,
            &RenderOptions {
                width: 30,
                unit: SizeUnit::Bytes,
                ..options()
            },
        );

        assert!(output.contains("sparse"));
        for line in output.lines() {
            assert!(
                display_width(line) <= 30,
                "line is too wide: {line:?} ({})",
                display_width(line)
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn escapes_control_characters_in_names() {
        use std::os::unix::ffi::OsStrExt;

        let temp = TempDir::new().unwrap();
        let name = std::ffi::OsStr::from_bytes(b"evil\x1b[31m");
        fs::write(temp.path().join(name), vec![b'x'; 10]).unwrap();
        let tree = Scanner::new(ScanOptions::default()).scan(&[temp.path().to_path_buf()]);
        let output = render(&tree, &options());

        assert!(!output.contains('\x1b'));
        assert!(output.contains("evil"));
    }

    #[test]
    fn adds_color_when_enabled() {
        let (_temp, tree) = fixture();
        let output = render(
            &tree,
            &RenderOptions {
                color: true,
                ..options()
            },
        );

        assert!(output.contains("\x1b["));
    }
}
