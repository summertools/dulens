use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, Metadata};
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};

use crate::SortKey;
use crate::format::sanitize_text;

pub type NodeId = usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Metric {
    Disk,
    Apparent,
}

impl Metric {
    pub fn label(self) -> &'static str {
        match self {
            Self::Disk => "disk",
            Self::Apparent => "apparent",
        }
    }
}

#[derive(Debug)]
pub struct FsNode {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub deduplicated: bool,
    pub disk: u64,
    pub apparent: u64,
    pub files: u64,
    pub dirs: u64,
    pub errors: u64,
    pub children: Vec<NodeId>,
}

impl FsNode {
    pub fn size(&self, metric: Metric) -> u64 {
        match metric {
            Metric::Disk => self.disk,
            Metric::Apparent => self.apparent,
        }
    }
}

#[derive(Debug)]
pub struct Tree {
    pub nodes: Vec<FsNode>,
    pub root: NodeId,
    pub warnings: Vec<String>,
    pub error_count: u64,
}

impl Tree {
    pub fn node(&self, id: NodeId) -> &FsNode {
        &self.nodes[id]
    }

    pub fn sorted_children(
        &self,
        id: NodeId,
        sort: SortKey,
        reverse: bool,
        metric: Metric,
    ) -> Vec<NodeId> {
        let mut keyed: Vec<(NodeId, String)> = self.nodes[id]
            .children
            .iter()
            .map(|&child| (child, self.nodes[child].name.to_lowercase()))
            .collect();

        keyed.sort_by(|(left_id, left_name), (right_id, right_name)| {
            let left = &self.nodes[*left_id];
            let right = &self.nodes[*right_id];
            match sort {
                SortKey::Size => right
                    .size(metric)
                    .cmp(&left.size(metric))
                    .then_with(|| left_name.cmp(right_name))
                    .then_with(|| left.name.cmp(&right.name)),
                SortKey::Name => left_name
                    .cmp(right_name)
                    .then_with(|| right.size(metric).cmp(&left.size(metric)))
                    .then_with(|| left.name.cmp(&right.name)),
            }
        });

        if reverse {
            keyed.reverse();
        }
        keyed.into_iter().map(|(id, _)| id).collect()
    }
}

#[derive(Debug, Default)]
pub struct Excludes {
    by_name: Vec<GlobMatcher>,
    by_path: Vec<GlobMatcher>,
}

impl Excludes {
    pub fn new(patterns: &[String]) -> Result<Self, String> {
        let mut excludes = Self::default();
        for pattern in patterns {
            let trimmed = pattern.trim_end_matches(['/', '\\']);
            let effective = if trimmed.is_empty() {
                pattern.as_str()
            } else {
                trimmed
            };
            let glob = GlobBuilder::new(effective)
                .literal_separator(true)
                .build()
                .map_err(|error| format!("invalid exclude pattern {pattern:?}: {error}"))?;
            if effective.contains('/') || effective.contains('\\') {
                excludes.by_path.push(glob.compile_matcher());
                if !effective.starts_with('/')
                    && !effective.starts_with("**/")
                    && let Ok(suffix) = GlobBuilder::new(&format!("**/{effective}"))
                        .literal_separator(true)
                        .build()
                {
                    excludes.by_path.push(suffix.compile_matcher());
                }
            } else {
                excludes.by_name.push(glob.compile_matcher());
            }
        }
        Ok(excludes)
    }

    fn matches(&self, path: &Path, name: &OsStr) -> bool {
        if self
            .by_name
            .iter()
            .any(|matcher| matcher.is_match(Path::new(name)))
        {
            return true;
        }
        self.by_path.iter().any(|matcher| matcher.is_match(path))
    }
}

#[derive(Debug, Default)]
pub struct ScanOptions {
    pub one_file_system: bool,
    pub excludes: Excludes,
}

pub struct Scanner {
    options: ScanOptions,
    nodes: Vec<FsNode>,
    seen_files: HashSet<(u64, u64)>,
    seen_dirs: HashSet<(u64, u64)>,
    warnings: Vec<String>,
    error_count: u64,
}

enum NodeResult {
    Node(NodeId),
    Skipped,
    Failed,
}

const MAX_WARNINGS: usize = 32;

impl Scanner {
    pub fn new(options: ScanOptions) -> Self {
        Self {
            options,
            nodes: Vec::new(),
            seen_files: HashSet::new(),
            seen_dirs: HashSet::new(),
            warnings: Vec::new(),
            error_count: 0,
        }
    }

    pub fn scan(mut self, paths: &[PathBuf]) -> Tree {
        let mut roots = Vec::new();
        for path in paths {
            let display_name = path.display().to_string();
            let mut root_device = None;
            if let NodeResult::Node(id) = self.scan_node(path, display_name, true, &mut root_device)
            {
                roots.push(id);
            }
        }

        let root_id = self.nodes.len();
        let mut disk = 0u64;
        let mut apparent = 0u64;
        let mut files = 0u64;
        let mut dirs = 0u64;
        let mut errors = 0u64;
        for &child in &roots {
            let node = &self.nodes[child];
            disk = disk.saturating_add(node.disk);
            apparent = apparent.saturating_add(node.apparent);
            files = files.saturating_add(node.files);
            dirs = dirs.saturating_add(node.dirs);
            errors = errors.saturating_add(node.errors);
        }

        self.nodes.push(FsNode {
            name: "all paths".to_string(),
            path: PathBuf::new(),
            is_dir: true,
            is_symlink: false,
            deduplicated: false,
            disk,
            apparent,
            files,
            dirs,
            errors,
            children: roots.clone(),
        });

        Tree {
            nodes: self.nodes,
            root: root_id,
            warnings: self.warnings,
            error_count: self.error_count,
        }
    }

    fn scan_node(
        &mut self,
        path: &Path,
        name: String,
        is_root: bool,
        root_device: &mut Option<u64>,
    ) -> NodeResult {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                self.warn(path, &error);
                return NodeResult::Failed;
            }
        };

        if !is_root
            && self.options.one_file_system
            && let (Some(expected), Some(identity)) = (*root_device, identity(&metadata))
            && identity.0 != expected
        {
            return NodeResult::Skipped;
        }

        if metadata.is_dir() {
            if let Some(identity) = identity(&metadata)
                && identity.1 != 0
                && !self.seen_dirs.insert((identity.0, identity.1))
            {
                return NodeResult::Skipped;
            }

            if is_root
                && root_device.is_none()
                && let Some(identity) = identity(&metadata)
            {
                *root_device = Some(identity.0);
            }

            let own_disk = disk_size(&metadata);
            let own_apparent = 0;
            let id = self.nodes.len();
            self.nodes.push(FsNode {
                name,
                path: path.to_path_buf(),
                is_dir: true,
                is_symlink: false,
                deduplicated: false,
                disk: own_disk,
                apparent: own_apparent,
                files: 0,
                dirs: 0,
                errors: 0,
                children: Vec::new(),
            });

            let mut child_ids = Vec::new();
            let mut local_errors = 0u64;
            let mut entries = Vec::new();
            match fs::read_dir(path) {
                Ok(reader) => {
                    for entry in reader {
                        match entry {
                            Ok(entry) => {
                                let child_path = entry.path();
                                let child_name = entry.file_name();
                                if !self.options.excludes.matches(&child_path, &child_name) {
                                    entries.push((child_path, child_name));
                                }
                            }
                            Err(error) => {
                                self.warn(path, &error);
                                local_errors = local_errors.saturating_add(1);
                            }
                        }
                    }
                }
                Err(error) => {
                    self.warn(path, &error);
                    local_errors = local_errors.saturating_add(1);
                }
            }

            entries.sort_by(|left, right| left.0.cmp(&right.0));
            for (child_path, child_name) in entries {
                let display_name = child_name.to_string_lossy().into_owned();
                match self.scan_node(&child_path, display_name, false, root_device) {
                    NodeResult::Node(child_id) => child_ids.push(child_id),
                    NodeResult::Skipped => {}
                    NodeResult::Failed => {
                        local_errors = local_errors.saturating_add(1);
                    }
                }
            }

            let mut disk = own_disk;
            let mut apparent = own_apparent;
            let mut files = 0u64;
            let mut dirs = 1u64;
            let mut errors = local_errors;
            for &child_id in &child_ids {
                let child = &self.nodes[child_id];
                disk = disk.saturating_add(child.disk);
                apparent = apparent.saturating_add(child.apparent);
                files = files.saturating_add(child.files);
                dirs = dirs.saturating_add(child.dirs);
                errors = errors.saturating_add(child.errors);
            }

            let node = &mut self.nodes[id];
            node.children = child_ids;
            node.disk = disk;
            node.apparent = apparent;
            node.files = files;
            node.dirs = dirs;
            node.errors = errors;

            NodeResult::Node(id)
        } else {
            let is_symlink = metadata.file_type().is_symlink();
            let mut deduplicated = false;
            if let Some(identity) = identity(&metadata)
                && identity.1 != 0
                && identity.2 > 1
                && !self.seen_files.insert((identity.0, identity.1))
            {
                deduplicated = true;
            }

            let id = self.nodes.len();
            self.nodes.push(FsNode {
                name,
                path: path.to_path_buf(),
                is_dir: false,
                is_symlink,
                deduplicated,
                disk: if deduplicated {
                    0
                } else {
                    disk_size(&metadata)
                },
                apparent: if deduplicated { 0 } else { metadata.len() },
                files: 1,
                dirs: 0,
                errors: 0,
                children: Vec::new(),
            });
            NodeResult::Node(id)
        }
    }

    fn warn(&mut self, path: &Path, error: &std::io::Error) {
        self.error_count = self.error_count.saturating_add(1);
        if self.warnings.len() < MAX_WARNINGS {
            let path = sanitize_text(&path.display().to_string());
            let message = sanitize_text(&error.to_string());
            self.warnings.push(format!("{path}: {message}"));
        }
    }
}

#[cfg(unix)]
fn identity(metadata: &Metadata) -> Option<(u64, u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino(), metadata.nlink()))
}

#[cfg(not(unix))]
fn identity(_metadata: &Metadata) -> Option<(u64, u64, u64)> {
    None
}

#[cfg(unix)]
fn disk_size(metadata: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn disk_size(metadata: &Metadata) -> u64 {
    metadata.len()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn scan(path: &Path) -> Tree {
        Scanner::new(ScanOptions::default()).scan(&[path.to_path_buf()])
    }

    fn root_node(tree: &Tree) -> &FsNode {
        tree.node(tree.node(tree.root).children[0])
    }

    fn child<'a>(tree: &'a Tree, node: &FsNode, name: &str) -> &'a FsNode {
        let id = node
            .children
            .iter()
            .copied()
            .find(|&id| tree.node(id).name == name)
            .expect("child exists");
        tree.node(id)
    }

    #[test]
    fn counts_files_and_directories() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("a.txt"), vec![b'a'; 100]).unwrap();
        fs::write(temp.path().join("b.txt"), vec![b'b'; 200]).unwrap();
        fs::create_dir(temp.path().join("sub")).unwrap();
        fs::write(temp.path().join("sub/c.txt"), vec![b'c'; 300]).unwrap();

        let tree = scan(temp.path());
        let root = root_node(&tree);

        assert_eq!(root.files, 3);
        assert_eq!(root.dirs, 2);
        assert_eq!(child(&tree, root, "a.txt").apparent, 100);
        assert_eq!(child(&tree, root, "b.txt").apparent, 200);
        assert_eq!(child(&tree, root, "sub").files, 1);
        assert_eq!(child(&tree, root, "sub").apparent, 300);
        assert_eq!(tree.error_count, 0);

        assert_eq!(root.apparent, 100 + 200 + 300);
    }

    #[cfg(unix)]
    #[test]
    fn uses_allocated_disk_blocks() {
        use std::os::unix::fs::MetadataExt;

        let temp = TempDir::new().unwrap();
        let path = temp.path().join("data");
        fs::write(&path, vec![b'x'; 10]).unwrap();

        let tree = scan(temp.path());
        let node = child(&tree, root_node(&tree), "data");
        let metadata = fs::metadata(&path).unwrap();

        assert_eq!(node.disk, metadata.blocks() * 512);
        assert_eq!(node.apparent, 10);
    }

    #[cfg(unix)]
    #[test]
    fn counts_hardlinks_once() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("original"), vec![b'x'; 4096]).unwrap();
        fs::hard_link(temp.path().join("original"), temp.path().join("linked")).unwrap();

        let tree = scan(temp.path());
        let root = root_node(&tree);
        let original = child(&tree, root, "original");
        let linked = child(&tree, root, "linked");

        assert_eq!(root.files, 2);
        assert_eq!(
            original.apparent + linked.apparent,
            4096,
            "the hard link must only be counted once"
        );
        assert!(!linked.deduplicated);
        assert!(original.deduplicated);
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinks() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("inside"), vec![b'z'; 512]).unwrap();
        std::os::unix::fs::symlink(&target, temp.path().join("shortcut")).unwrap();

        let tree = scan(temp.path());
        let root = root_node(&tree);
        let shortcut = child(&tree, root, "shortcut");

        assert!(shortcut.is_symlink);
        assert!(!shortcut.is_dir);
        assert_eq!(root.files, 2);
        assert_eq!(tree.error_count, 0);
    }

    #[test]
    fn applies_excludes_and_stops_counting() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("keep")).unwrap();
        fs::write(temp.path().join("keep/data"), vec![b'k'; 64]).unwrap();
        fs::create_dir(temp.path().join("skip")).unwrap();
        fs::write(temp.path().join("skip/huge"), vec![b's'; 8192]).unwrap();

        let excludes = Excludes::new(&["skip".to_string()]).unwrap();
        let tree = Scanner::new(ScanOptions {
            excludes,
            ..ScanOptions::default()
        })
        .scan(&[temp.path().to_path_buf()]);

        let root = root_node(&tree);
        assert_eq!(root.files, 1);
        assert!(root.children.iter().all(|&id| tree.node(id).name != "skip"));
        assert_eq!(child(&tree, root, "keep").files, 1);
    }

    #[test]
    fn excludes_nested_path_patterns() {
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("src/generated")).unwrap();
        fs::write(temp.path().join("src/generated/huge"), vec![b'x'; 8192]).unwrap();
        fs::write(temp.path().join("src/keep"), vec![b'k'; 32]).unwrap();

        let excludes = Excludes::new(&["src/generated".to_string()]).unwrap();
        let tree = Scanner::new(ScanOptions {
            excludes,
            ..ScanOptions::default()
        })
        .scan(&[temp.path().to_path_buf()]);

        let root = root_node(&tree);
        let src = child(&tree, root, "src");
        assert!(
            src.children
                .iter()
                .all(|&id| tree.node(id).name != "generated")
        );
        assert_eq!(src.files, 1);
    }

    #[test]
    fn reports_missing_roots() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("missing");
        let tree = scan(&missing);

        assert!(tree.node(tree.root).children.is_empty());
        assert_eq!(tree.error_count, 1);
        assert_eq!(tree.warnings.len(), 1);
    }

    #[test]
    fn sorts_by_size_then_name() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("small"), vec![b's'; 10]).unwrap();
        fs::write(temp.path().join("large"), vec![b'l'; 1000]).unwrap();
        fs::write(temp.path().join("medium"), vec![b'm'; 500]).unwrap();

        let tree = scan(temp.path());
        let root = root_node(&tree);
        let sorted = tree.sorted_children(
            tree.node(tree.root).children[0],
            SortKey::Size,
            false,
            Metric::Apparent,
        );
        let names: Vec<_> = sorted
            .iter()
            .map(|&id| tree.node(id).name.as_str())
            .collect();
        assert_eq!(names, ["large", "medium", "small"]);
        assert_eq!(root.files, 3);
    }
}
