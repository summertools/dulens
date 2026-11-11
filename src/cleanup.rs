//! Safe discovery and removal of regenerable build caches and dependency
//! directories for common language toolchains.
//!
//! Detection is deliberately marker-based: a directory named `target` is only
//! offered when its parent looks like a Cargo, Maven, Gradle, or sbt project.
//! Dependency directories such as `node_modules` are discovered too, but are
//! not selected by default. No path is deleted until the user confirms it in
//! the interactive cleanup screen.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use globset::{GlobBuilder, GlobMatcher};
use serde::Deserialize;

use crate::scan::{ScanOptions, Scanner, Tree};

const MAX_CONFIG_BYTES: u64 = 128 * 1024;
const MAX_CANDIDATES: usize = 5000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Category {
    Build,
    Cache,
    Dependency,
}

impl Category {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Build => "BUILD",
            Self::Cache => "CACHE",
            Self::Dependency => "DEPS",
        }
    }

    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Build => "generated build output",
            Self::Cache => "regenerable tool cache",
            Self::Dependency => "downloaded dependency library",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SelectionPreset {
    /// Build output and lightweight tool caches.
    #[default]
    Build,
    /// Downloaded dependency directories and package caches.
    Dependencies,
    /// Everything, including dependency directories.
    All,
    /// Nothing selected initially.
    None,
}

impl SelectionPreset {
    #[must_use]
    pub const fn selects(self, category: Category) -> bool {
        match self {
            Self::All => true,
            Self::None => false,
            Self::Build => matches!(category, Category::Build | Category::Cache),
            Self::Dependencies => matches!(category, Category::Dependency | Category::Cache),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub include_global: bool,
    pub include_global_dependencies: bool,
    pub default_selection: SelectionPreset,
    pub profile: String,
    extra_cache: Vec<GlobMatcher>,
    extra_dependencies: Vec<GlobMatcher>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            include_global: false,
            include_global_dependencies: false,
            default_selection: SelectionPreset::Build,
            profile: "safe".to_owned(),
            extra_cache: Vec::new(),
            extra_dependencies: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: PathBuf,
    /// Project root (for deletion containment) or the exact global cache path.
    pub root: PathBuf,
    pub size: u64,
    pub category: Category,
    pub language: String,
    pub reason: String,
    pub global: bool,
    pub selected: bool,
}

impl Candidate {
    #[must_use]
    pub fn display_path(&self) -> String {
        let text = self.path.display().to_string();
        if let Some(home) = home_dir() {
            let home = home.display().to_string();
            if let Some(rest) = text.strip_prefix(&home)
                && (rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\'))
            {
                return format!("~{rest}");
            }
        }
        text
    }
}

#[derive(Debug, Default)]
pub struct Detection {
    pub candidates: Vec<Candidate>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct CleanupReport {
    pub deleted: usize,
    pub freed: u64,
    pub errors: Vec<String>,
}

impl CleanupReport {
    #[must_use]
    pub fn failed(&self) -> bool {
        !self.errors.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    cleanup: CleanupFile,
}

#[derive(Debug, Default, Deserialize)]
struct CleanupFile {
    profile: Option<String>,
    include_global: Option<bool>,
    include_global_dependencies: Option<bool>,
    default: Option<SelectionPreset>,
    #[serde(default)]
    extra_cache: Vec<String>,
    #[serde(default)]
    extra_dependencies: Vec<String>,
    #[serde(default)]
    profiles: HashMap<String, CleanupProfileFile>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct CleanupProfileFile {
    include_global: Option<bool>,
    include_global_dependencies: Option<bool>,
    default: Option<SelectionPreset>,
    #[serde(default)]
    extra_cache: Vec<String>,
    #[serde(default)]
    extra_dependencies: Vec<String>,
}

/// Load cleanup settings and resolve the chosen profile.
///
/// `explicit` is the `--config` path. A missing implicit config is fine;
/// malformed files produce warnings and safe defaults. Explicit errors are
/// returned so scripts do not silently use the wrong profile.
pub fn load_settings(
    explicit: Option<&Path>,
    requested_profile: Option<&str>,
) -> Result<(Settings, Vec<String>), String> {
    let mut warnings = Vec::new();
    let path = if let Some(path) = explicit {
        Some(path.to_path_buf())
    } else if let Some(path) = env::var_os("DULENS_CONFIG") {
        Some(PathBuf::from(path))
    } else {
        default_config_path()
    };

    let Some(path) = path else {
        let profile_name = requested_profile.unwrap_or("safe");
        let settings = builtin_profile(profile_name)
            .ok_or_else(|| format!("unknown cleanup profile {profile_name:?}"))?;
        return Ok((settings, warnings));
    };
    let explicit_path = explicit.is_some() || env::var_os("DULENS_CONFIG").is_some();
    let text = match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_CONFIG_BYTES => {
            match fs::read_to_string(&path) {
                Ok(text) => Some(text),
                Err(error) => {
                    if explicit_path {
                        return Err(format!("cannot read {}: {error}", safe_path(&path)));
                    }
                    warnings.push(format!("cannot read {}: {error}", safe_path(&path)));
                    None
                }
            }
        }
        Ok(metadata) if !metadata.is_file() => {
            if explicit_path {
                return Err(format!("{} is not a regular file", safe_path(&path)));
            }
            warnings.push(format!("{} is not a regular file", safe_path(&path)));
            None
        }
        Ok(_) => {
            let message = format!(
                "{} is larger than {} KiB",
                safe_path(&path),
                MAX_CONFIG_BYTES / 1024
            );
            if explicit_path {
                return Err(message);
            }
            warnings.push(message);
            None
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !explicit_path => None,
        Err(error) => {
            if explicit_path {
                return Err(format!("cannot read {}: {error}", safe_path(&path)));
            }
            warnings.push(format!("cannot read {}: {error}", safe_path(&path)));
            None
        }
    };

    let file = if let Some(text) = text {
        match toml::from_str::<ConfigFile>(&text) {
            Ok(file) => file,
            Err(error) => {
                if explicit_path {
                    return Err(format!("invalid config {}: {error}", safe_path(&path)));
                }
                warnings.push(format!("invalid config {}: {error}", safe_path(&path)));
                ConfigFile::default()
            }
        }
    } else {
        ConfigFile::default()
    };

    let profile_name = requested_profile
        .map(str::to_owned)
        .or_else(|| file.cleanup.profile.clone())
        .unwrap_or_else(|| "safe".to_owned());

    let builtin = builtin_profile(&profile_name);
    let config_profile = file.cleanup.profiles.get(&profile_name);
    if builtin.is_none() && config_profile.is_none() {
        return Err(format!("unknown cleanup profile {profile_name:?}"));
    }
    let mut settings = builtin.unwrap_or_default();
    // Top-level settings form the base; a named profile can override them.
    apply_top_level(&mut settings, &file.cleanup, &mut warnings);
    if let Some(profile) = config_profile {
        apply_profile(&mut settings, profile, &mut warnings);
    }
    settings.profile = profile_name;
    Ok((settings, warnings))
}

fn builtin_profile(name: &str) -> Option<Settings> {
    match name {
        "safe" => Some(Settings {
            include_global: true,
            ..Settings::default()
        }),
        "deep" => Some(Settings {
            include_global: true,
            include_global_dependencies: true,
            default_selection: SelectionPreset::Build,
            profile: "deep".to_owned(),
            ..Settings::default()
        }),
        "dependencies" | "deps" => Some(Settings {
            include_global: true,
            include_global_dependencies: true,
            default_selection: SelectionPreset::Dependencies,
            profile: "dependencies".to_owned(),
            ..Settings::default()
        }),
        "all" => Some(Settings {
            include_global: true,
            include_global_dependencies: true,
            default_selection: SelectionPreset::All,
            profile: "all".to_owned(),
            ..Settings::default()
        }),
        _ => None,
    }
}

fn apply_top_level(settings: &mut Settings, file: &CleanupFile, warnings: &mut Vec<String>) {
    if let Some(global) = file.include_global {
        settings.include_global = global;
    }
    if let Some(global_deps) = file.include_global_dependencies {
        settings.include_global_dependencies = global_deps;
    }
    if let Some(selection) = file.default {
        settings.default_selection = selection;
    }
    compile_extra(
        &file.extra_cache,
        &mut settings.extra_cache,
        "extra_cache",
        warnings,
    );
    compile_extra(
        &file.extra_dependencies,
        &mut settings.extra_dependencies,
        "extra_dependencies",
        warnings,
    );
}

fn apply_profile(
    settings: &mut Settings,
    profile: &CleanupProfileFile,
    warnings: &mut Vec<String>,
) {
    if let Some(global) = profile.include_global {
        settings.include_global = global;
    }
    if let Some(global_deps) = profile.include_global_dependencies {
        settings.include_global_dependencies = global_deps;
    }
    if let Some(selection) = profile.default {
        settings.default_selection = selection;
    }
    compile_extra(
        &profile.extra_cache,
        &mut settings.extra_cache,
        "extra_cache",
        warnings,
    );
    compile_extra(
        &profile.extra_dependencies,
        &mut settings.extra_dependencies,
        "extra_dependencies",
        warnings,
    );
}

fn compile_extra(
    patterns: &[String],
    target: &mut Vec<GlobMatcher>,
    field: &str,
    warnings: &mut Vec<String>,
) {
    for pattern in patterns.iter().take(128) {
        let pattern = pattern.trim();
        if pattern.is_empty()
            || pattern.len() > 128
            || pattern.contains('/')
            || pattern.contains('\\')
            || pattern == "."
            || pattern == ".."
            || pattern.chars().any(char::is_control)
        {
            warnings.push(format!("{field}: ignored invalid pattern {pattern:?}"));
            continue;
        }
        match GlobBuilder::new(pattern).literal_separator(true).build() {
            Ok(glob) => target.push(glob.compile_matcher()),
            Err(error) => warnings.push(format!("{field}: invalid pattern {pattern:?}: {error}")),
        }
    }
}

#[must_use]
pub fn default_config_path() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir).join("dulens").join("config.toml"));
    }
    home_dir().map(|home| home.join(".config").join("dulens").join("config.toml"))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn safe_path(path: &Path) -> String {
    crate::format::sanitize_text(&path.display().to_string())
}

// ---------------------------------------------------------------------------
// Project rules
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum NameMatch {
    Exact(&'static str),
    Prefix(&'static str),
    Suffix(&'static str),
}

impl NameMatch {
    fn matches(self, name: &str) -> bool {
        match self {
            Self::Exact(value) => name == value,
            Self::Prefix(value) => name.starts_with(value) && name.len() > value.len(),
            Self::Suffix(value) => name.ends_with(value) && name.len() > value.len(),
        }
    }
}

#[derive(Clone, Copy)]
enum Markers {
    Any,
    Files(&'static [&'static str]),
    Suffixes(&'static [&'static str]),
}

struct Rule {
    name: NameMatch,
    markers: Markers,
    category: Category,
    language: &'static str,
    reason: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        name: NameMatch::Exact("target"),
        markers: Markers::Files(&["Cargo.toml"]),
        category: Category::Build,
        language: "rust",
        reason: "recreated by cargo build",
    },
    Rule {
        name: NameMatch::Exact("target"),
        markers: Markers::Files(&["pom.xml"]),
        category: Category::Build,
        language: "java",
        reason: "recreated by maven",
    },
    Rule {
        name: NameMatch::Exact("target"),
        markers: Markers::Files(&["build.sbt"]),
        category: Category::Build,
        language: "scala",
        reason: "recreated by sbt",
    },
    Rule {
        name: NameMatch::Exact("target"),
        markers: Markers::Files(&["build.gradle", "build.gradle.kts"]),
        category: Category::Build,
        language: "jvm",
        reason: "recreated by gradle",
    },
    Rule {
        name: NameMatch::Exact("build"),
        markers: Markers::Files(&["CMakeLists.txt"]),
        category: Category::Build,
        language: "c/c++",
        reason: "recreated by cmake",
    },
    Rule {
        name: NameMatch::Exact("build"),
        markers: Markers::Files(&["meson.build"]),
        category: Category::Build,
        language: "c/c++",
        reason: "recreated by meson",
    },
    Rule {
        name: NameMatch::Exact("build"),
        markers: Markers::Files(&["build.gradle", "build.gradle.kts"]),
        category: Category::Build,
        language: "jvm",
        reason: "recreated by gradle",
    },
    Rule {
        name: NameMatch::Exact("build"),
        markers: Markers::Files(&["pubspec.yaml"]),
        category: Category::Build,
        language: "dart",
        reason: "recreated by flutter/dart build",
    },
    Rule {
        name: NameMatch::Exact("build"),
        markers: Markers::Files(&["pyproject.toml", "setup.py"]),
        category: Category::Build,
        language: "python",
        reason: "recreated by the python build",
    },
    Rule {
        name: NameMatch::Prefix("cmake-build-"),
        markers: Markers::Files(&["CMakeLists.txt"]),
        category: Category::Build,
        language: "c/c++",
        reason: "recreated by cmake",
    },
    Rule {
        name: NameMatch::Exact("CMakeFiles"),
        markers: Markers::Files(&["CMakeLists.txt"]),
        category: Category::Build,
        language: "c/c++",
        reason: "recreated by cmake",
    },
    Rule {
        name: NameMatch::Exact("bin"),
        markers: Markers::Suffixes(&[".csproj", ".sln", ".fsproj", ".vbproj"]),
        category: Category::Build,
        language: ".net",
        reason: "recreated by dotnet build",
    },
    Rule {
        name: NameMatch::Exact("obj"),
        markers: Markers::Suffixes(&[".csproj", ".sln", ".fsproj", ".vbproj"]),
        category: Category::Build,
        language: ".net",
        reason: "recreated by dotnet build",
    },
    Rule {
        name: NameMatch::Exact("node_modules"),
        markers: Markers::Files(&[
            "package.json",
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
            "bun.lockb",
        ]),
        category: Category::Dependency,
        language: "node",
        reason: "reinstalled by npm/yarn/pnpm/bun",
    },
    Rule {
        name: NameMatch::Exact(".next"),
        markers: Markers::Files(&["package.json"]),
        category: Category::Cache,
        language: "next.js",
        reason: "recreated by next build",
    },
    Rule {
        name: NameMatch::Exact(".nuxt"),
        markers: Markers::Files(&["package.json"]),
        category: Category::Cache,
        language: "nuxt",
        reason: "recreated by nuxt build",
    },
    Rule {
        name: NameMatch::Exact(".svelte-kit"),
        markers: Markers::Files(&["package.json", "svelte.config.js"]),
        category: Category::Cache,
        language: "svelte",
        reason: "recreated by sveltekit build",
    },
    Rule {
        name: NameMatch::Exact(".turbo"),
        markers: Markers::Files(&["package.json"]),
        category: Category::Cache,
        language: "turborepo",
        reason: "recreated by turbo",
    },
    Rule {
        name: NameMatch::Exact(".parcel-cache"),
        markers: Markers::Files(&["package.json"]),
        category: Category::Cache,
        language: "parcel",
        reason: "recreated by parcel",
    },
    Rule {
        name: NameMatch::Exact(".vite"),
        markers: Markers::Files(&["package.json"]),
        category: Category::Cache,
        language: "vite",
        reason: "recreated by vite",
    },
    Rule {
        name: NameMatch::Exact(".angular"),
        markers: Markers::Files(&["package.json"]),
        category: Category::Cache,
        language: "angular",
        reason: "recreated by the angular cli",
    },
    Rule {
        name: NameMatch::Exact(".nx"),
        markers: Markers::Files(&["package.json"]),
        category: Category::Cache,
        language: "nx",
        reason: "recreated by nx",
    },
    Rule {
        name: NameMatch::Exact(".cache"),
        markers: Markers::Files(&["package.json"]),
        category: Category::Cache,
        language: "node",
        reason: "recreated by the package tool",
    },
    Rule {
        name: NameMatch::Exact("__pycache__"),
        markers: Markers::Any,
        category: Category::Cache,
        language: "python",
        reason: "recreated by the python interpreter",
    },
    Rule {
        name: NameMatch::Exact(".mypy_cache"),
        markers: Markers::Any,
        category: Category::Cache,
        language: "python",
        reason: "recreated by mypy",
    },
    Rule {
        name: NameMatch::Exact(".pytest_cache"),
        markers: Markers::Any,
        category: Category::Cache,
        language: "python",
        reason: "recreated by pytest",
    },
    Rule {
        name: NameMatch::Exact(".ruff_cache"),
        markers: Markers::Any,
        category: Category::Cache,
        language: "python",
        reason: "recreated by ruff",
    },
    Rule {
        name: NameMatch::Exact(".hypothesis"),
        markers: Markers::Any,
        category: Category::Cache,
        language: "python",
        reason: "recreated by hypothesis",
    },
    Rule {
        name: NameMatch::Exact(".pytype"),
        markers: Markers::Any,
        category: Category::Cache,
        language: "python",
        reason: "recreated by pytype",
    },
    Rule {
        name: NameMatch::Exact(".pyre"),
        markers: Markers::Any,
        category: Category::Cache,
        language: "python",
        reason: "recreated by pyre",
    },
    Rule {
        name: NameMatch::Exact(".tox"),
        markers: Markers::Any,
        category: Category::Dependency,
        language: "python",
        reason: "recreated by tox",
    },
    Rule {
        name: NameMatch::Exact(".nox"),
        markers: Markers::Any,
        category: Category::Dependency,
        language: "python",
        reason: "recreated by nox",
    },
    Rule {
        name: NameMatch::Exact(".venv"),
        markers: Markers::Any,
        category: Category::Dependency,
        language: "python",
        reason: "recreated from the project dependencies",
    },
    Rule {
        name: NameMatch::Exact("venv"),
        markers: Markers::Any,
        category: Category::Dependency,
        language: "python",
        reason: "recreated from the project dependencies",
    },
    Rule {
        name: NameMatch::Exact("vendor"),
        markers: Markers::Files(&["go.mod"]),
        category: Category::Dependency,
        language: "go",
        reason: "restored by go mod vendor",
    },
    Rule {
        name: NameMatch::Exact("vendor"),
        markers: Markers::Files(&["composer.json"]),
        category: Category::Dependency,
        language: "php",
        reason: "restored by composer install",
    },
    Rule {
        name: NameMatch::Exact("vendor"),
        markers: Markers::Files(&["Gemfile"]),
        category: Category::Dependency,
        language: "ruby",
        reason: "restored by bundle install",
    },
    Rule {
        name: NameMatch::Exact("deps"),
        markers: Markers::Files(&["mix.exs"]),
        category: Category::Dependency,
        language: "elixir",
        reason: "restored by mix deps.get",
    },
    Rule {
        name: NameMatch::Exact("_build"),
        markers: Markers::Files(&["mix.exs"]),
        category: Category::Build,
        language: "elixir",
        reason: "recreated by mix compile",
    },
    Rule {
        name: NameMatch::Exact("_build"),
        markers: Markers::Files(&["rebar.config"]),
        category: Category::Build,
        language: "erlang",
        reason: "recreated by rebar3 compile",
    },
    Rule {
        name: NameMatch::Exact("dist-newstyle"),
        markers: Markers::Suffixes(&[".cabal"]),
        category: Category::Build,
        language: "haskell",
        reason: "recreated by cabal build",
    },
    Rule {
        name: NameMatch::Exact(".stack-work"),
        markers: Markers::Files(&["stack.yaml"]),
        category: Category::Build,
        language: "haskell",
        reason: "recreated by stack build",
    },
    Rule {
        name: NameMatch::Exact("zig-cache"),
        markers: Markers::Files(&["build.zig"]),
        category: Category::Cache,
        language: "zig",
        reason: "recreated by zig build",
    },
    Rule {
        name: NameMatch::Exact(".zig-cache"),
        markers: Markers::Files(&["build.zig"]),
        category: Category::Cache,
        language: "zig",
        reason: "recreated by zig build",
    },
    Rule {
        name: NameMatch::Exact("zig-out"),
        markers: Markers::Files(&["build.zig"]),
        category: Category::Build,
        language: "zig",
        reason: "recreated by zig build",
    },
    Rule {
        name: NameMatch::Exact(".dart_tool"),
        markers: Markers::Files(&["pubspec.yaml"]),
        category: Category::Cache,
        language: "dart",
        reason: "recreated by dart/flutter pub get",
    },
    Rule {
        name: NameMatch::Exact("Pods"),
        markers: Markers::Files(&["Podfile"]),
        category: Category::Dependency,
        language: "ios",
        reason: "restored by pod install",
    },
    Rule {
        name: NameMatch::Exact("Carthage"),
        markers: Markers::Files(&["Cartfile"]),
        category: Category::Dependency,
        language: "ios",
        reason: "restored by carthage update",
    },
    Rule {
        name: NameMatch::Exact(".build"),
        markers: Markers::Files(&["Package.swift"]),
        category: Category::Build,
        language: "swift",
        reason: "recreated by swift build",
    },
    Rule {
        name: NameMatch::Exact(".gradle"),
        markers: Markers::Files(&["build.gradle", "build.gradle.kts", "settings.gradle"]),
        category: Category::Cache,
        language: "jvm",
        reason: "recreated by gradle",
    },
    Rule {
        name: NameMatch::Exact(".terraform"),
        markers: Markers::Files(&["terraform.tfstate", ".terraform.lock.hcl"]),
        category: Category::Dependency,
        language: "terraform",
        reason: "restored by terraform init",
    },
    Rule {
        name: NameMatch::Exact("bower_components"),
        markers: Markers::Files(&["bower.json"]),
        category: Category::Dependency,
        language: "bower",
        reason: "restored by bower install",
    },
    Rule {
        name: NameMatch::Suffix(".egg-info"),
        markers: Markers::Files(&["pyproject.toml", "setup.py"]),
        category: Category::Build,
        language: "python",
        reason: "recreated by the python build",
    },
    Rule {
        name: NameMatch::Exact(".bloop"),
        markers: Markers::Files(&["build.sbt"]),
        category: Category::Cache,
        language: "scala",
        reason: "recreated by sbt/bloop",
    },
    Rule {
        name: NameMatch::Exact(".metals"),
        markers: Markers::Files(&["build.sbt"]),
        category: Category::Cache,
        language: "scala",
        reason: "recreated by metals",
    },
    Rule {
        name: NameMatch::Exact(".scala-build"),
        markers: Markers::Files(&["build.sbt"]),
        category: Category::Build,
        language: "scala",
        reason: "recreated by scala-cli",
    },
    Rule {
        name: NameMatch::Exact("renv"),
        markers: Markers::Files(&["renv.lock"]),
        category: Category::Dependency,
        language: "r",
        reason: "restored by renv::restore",
    },
    Rule {
        name: NameMatch::Exact(".shards"),
        markers: Markers::Files(&["shard.yml"]),
        category: Category::Dependency,
        language: "crystal",
        reason: "restored by shards install",
    },
    Rule {
        name: NameMatch::Exact("_build"),
        markers: Markers::Files(&["dune-project"]),
        category: Category::Build,
        language: "ocaml",
        reason: "recreated by dune build",
    },
    Rule {
        name: NameMatch::Exact("elm-stuff"),
        markers: Markers::Files(&["elm.json"]),
        category: Category::Dependency,
        language: "elm",
        reason: "restored by elm make",
    },
    Rule {
        name: NameMatch::Exact("DerivedDataCache"),
        markers: Markers::Suffixes(&[".uproject"]),
        category: Category::Cache,
        language: "unreal",
        reason: "recreated by the unreal editor",
    },
    Rule {
        name: NameMatch::Exact("Intermediate"),
        markers: Markers::Suffixes(&[".uproject"]),
        category: Category::Build,
        language: "unreal",
        reason: "recreated by the unreal editor",
    },
    Rule {
        name: NameMatch::Exact("Library"),
        markers: Markers::Files(&["ProjectSettings/ProjectVersion.txt"]),
        category: Category::Build,
        language: "unity",
        reason: "recreated by the unity editor",
    },
];

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

pub fn detect(tree: &Tree, settings: &Settings) -> Detection {
    let mut detection = Detection::default();
    let root_node = tree.root;
    let roots: Vec<_> = tree.node(root_node).children.clone();
    for root in roots {
        let node = tree.node(root);
        if !node.is_dir {
            continue;
        }
        walk_project(tree, node, true, settings, &mut detection);
    }

    if settings.include_global
        && let Some(home) = home_dir()
    {
        for global in global_candidates(&home) {
            if global.category == Category::Dependency && !settings.include_global_dependencies {
                continue;
            }
            if is_plain_directory(&global.path) {
                match measure(&global.path) {
                    Some(size) if size > 0 => detection.candidates.push(Candidate {
                        path: global.path.clone(),
                        root: global.path,
                        size,
                        category: global.category,
                        language: global.language.to_owned(),
                        reason: global.reason.to_owned(),
                        global: true,
                        selected: settings.default_selection.selects(global.category),
                    }),
                    Some(_) => {}
                    None => detection.warnings.push(format!(
                        "could not measure global cache {}",
                        global.path.display()
                    )),
                }
            }
        }
    }

    deduplicate_nested(&mut detection.candidates);
    if detection.candidates.len() > MAX_CANDIDATES {
        detection
            .candidates
            .sort_by_key(|candidate| std::cmp::Reverse(candidate.size));
        detection.candidates.truncate(MAX_CANDIDATES);
        detection.warnings.push(format!(
            "showing the {MAX_CANDIDATES} largest cleanup candidates"
        ));
    }
    detection
        .candidates
        .sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
    detection
}

fn walk_project(
    tree: &Tree,
    node: &crate::scan::FsNode,
    is_scan_root: bool,
    settings: &Settings,
    detection: &mut Detection,
) {
    if !is_scan_root && let Some(candidate) = match_rule(tree, node, settings) {
        detection.candidates.push(candidate);
        return;
    }
    for child in &node.children {
        let child_node = tree.node(*child);
        if child_node.is_dir {
            walk_project(tree, child_node, false, settings, detection);
        }
    }
}

fn match_rule(tree: &Tree, node: &crate::scan::FsNode, settings: &Settings) -> Option<Candidate> {
    if node.is_symlink || !node.is_dir {
        return None;
    }
    let name = node.name.as_str();
    let parent = node.path.parent()?;
    if let Some(rule) = RULES.iter().find(|rule| rule.name.matches(name))
        && markers_match(parent, rule.markers)
    {
        return Some(Candidate {
            path: node.path.clone(),
            root: root_for_node(tree, node),
            size: node.disk,
            category: rule.category,
            language: rule.language.to_owned(),
            reason: rule.reason.to_owned(),
            global: false,
            selected: settings.default_selection.selects(rule.category),
        });
    }
    if settings
        .extra_cache
        .iter()
        .any(|matcher| matcher.is_match(Path::new(name)))
    {
        return Some(custom_candidate(tree, node, settings, Category::Cache));
    }
    if settings
        .extra_dependencies
        .iter()
        .any(|matcher| matcher.is_match(Path::new(name)))
    {
        return Some(custom_candidate(tree, node, settings, Category::Dependency));
    }
    None
}

fn custom_candidate(
    tree: &Tree,
    node: &crate::scan::FsNode,
    settings: &Settings,
    category: Category,
) -> Candidate {
    Candidate {
        path: node.path.clone(),
        root: root_for_node(tree, node),
        size: node.disk,
        category,
        language: "custom".to_owned(),
        reason: "configured directory pattern".to_owned(),
        global: false,
        selected: settings.default_selection.selects(category),
    }
}

fn root_for_node(tree: &Tree, node: &crate::scan::FsNode) -> PathBuf {
    // The scan root is the top-level path; use the node's own path when a
    // caller has handed us a single directory, otherwise walk to the root by
    // matching the longest tree-root child prefix.
    tree.node(tree.root)
        .children
        .iter()
        .map(|id| &tree.node(*id).path)
        .filter(|root| node.path.starts_with(root))
        .max_by_key(|root| root.components().count())
        .cloned()
        .unwrap_or_else(|| node.path.clone())
}

fn markers_match(parent: &Path, markers: Markers) -> bool {
    match markers {
        Markers::Any => true,
        Markers::Files(files) => files.iter().any(|file| {
            fs::symlink_metadata(parent.join(file)).is_ok_and(|metadata| metadata.is_file())
        }),
        Markers::Suffixes(suffixes) => {
            let Ok(entries) = fs::read_dir(parent) else {
                return false;
            };
            entries.flatten().any(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                suffixes.iter().any(|suffix| name.ends_with(suffix))
            })
        }
    }
}

#[derive(Clone)]
struct GlobalCandidate {
    path: PathBuf,
    category: Category,
    language: &'static str,
    reason: &'static str,
}

fn cache_home(home: &Path) -> PathBuf {
    env_path("XDG_CACHE_HOME").unwrap_or_else(|| home.join(".cache"))
}

fn data_home(home: &Path) -> PathBuf {
    env_path("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"))
}

fn global_candidates(home: &Path) -> Vec<GlobalCandidate> {
    let mut out = Vec::new();
    let cache = cache_home(home);
    let data = data_home(home);
    let cargo = env_path("CARGO_HOME").unwrap_or_else(|| home.join(".cargo"));
    push_global(
        &mut out,
        cargo.join("registry").join("cache"),
        Category::Dependency,
        "rust",
        "downloaded crate archives",
    );
    push_global(
        &mut out,
        env_path("GOCACHE").unwrap_or_else(|| cache.join("go-build")),
        Category::Cache,
        "go",
        "recreated by go build",
    );
    push_global(
        &mut out,
        env_path("GOMODCACHE").unwrap_or_else(|| home.join("go").join("pkg").join("mod")),
        Category::Dependency,
        "go",
        "downloaded go modules",
    );
    let npm = env_path("NPM_CONFIG_CACHE")
        .or_else(|| env_path("npm_config_cache"))
        .unwrap_or_else(|| home.join(".npm"));
    push_global(
        &mut out,
        npm.join("_cacache"),
        Category::Cache,
        "node",
        "recreated by npm",
    );
    push_global(
        &mut out,
        env_path("YARN_CACHE_FOLDER").unwrap_or_else(|| cache.join("yarn")),
        Category::Cache,
        "node",
        "recreated by yarn",
    );
    push_global(
        &mut out,
        env_path("PNPM_HOME")
            .map(|path| path.join("store"))
            .unwrap_or_else(|| data.join("pnpm").join("store")),
        Category::Dependency,
        "node",
        "recreated by pnpm install",
    );
    push_global(
        &mut out,
        env_path("PIP_CACHE_DIR").unwrap_or_else(|| cache.join("pip")),
        Category::Cache,
        "python",
        "recreated by pip",
    );
    push_global(
        &mut out,
        env_path("UV_CACHE_DIR").unwrap_or_else(|| cache.join("uv")),
        Category::Cache,
        "python",
        "recreated by uv",
    );
    push_global(
        &mut out,
        env_path("GRADLE_USER_HOME")
            .map(|path| path.join("caches"))
            .unwrap_or_else(|| home.join(".gradle").join("caches")),
        Category::Dependency,
        "jvm",
        "recreated by gradle",
    );
    push_global(
        &mut out,
        home.join(".m2").join("repository"),
        Category::Dependency,
        "java",
        "recreated by maven",
    );
    push_global(
        &mut out,
        env_path("NUGET_PACKAGES").unwrap_or_else(|| home.join(".nuget").join("packages")),
        Category::Dependency,
        ".net",
        "recreated by dotnet restore",
    );
    push_global(
        &mut out,
        env_path("CCACHE_DIR").unwrap_or_else(|| cache.join("ccache")),
        Category::Cache,
        "c/c++",
        "recreated by ccache",
    );
    push_global(
        &mut out,
        env_path("SCCACHE_DIR").unwrap_or_else(|| cache.join("sccache")),
        Category::Cache,
        "c/c++",
        "recreated by sccache",
    );
    push_global(
        &mut out,
        env_path("PUB_CACHE").unwrap_or_else(|| home.join(".pub-cache")),
        Category::Dependency,
        "dart",
        "recreated by dart pub get",
    );
    push_global(
        &mut out,
        env_path("BUN_INSTALL")
            .map(|path| path.join("install").join("cache"))
            .unwrap_or_else(|| home.join(".bun").join("install").join("cache")),
        Category::Cache,
        "node",
        "recreated by bun install",
    );
    push_global(
        &mut out,
        env_path("DENO_DIR").unwrap_or_else(|| cache.join("deno")),
        Category::Cache,
        "deno",
        "recreated by deno",
    );
    push_global(
        &mut out,
        cache.join("bazel"),
        Category::Cache,
        "bazel",
        "recreated by bazel",
    );
    #[cfg(target_os = "macos")]
    push_global(
        &mut out,
        home.join("Library")
            .join("Developer")
            .join("Xcode")
            .join("DerivedData"),
        Category::Cache,
        "swift",
        "recreated by xcode",
    );
    out
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn push_global(
    out: &mut Vec<GlobalCandidate>,
    path: PathBuf,
    category: Category,
    language: &'static str,
    reason: &'static str,
) {
    if path.is_absolute() {
        out.push(GlobalCandidate {
            path,
            category,
            language,
            reason,
        });
    }
}

fn measure(path: &Path) -> Option<u64> {
    let tree = Scanner::new(ScanOptions::default()).scan(&[path.to_path_buf()]);
    let root = tree.node(tree.root).children.first()?;
    Some(tree.node(*root).disk)
}

fn deduplicate_nested(candidates: &mut Vec<Candidate>) {
    if candidates.len() < 2 {
        return;
    }
    // Compare canonical paths so relative input paths and symlinked workspaces
    // cannot produce two overlapping cleanup entries.
    let mut entries: Vec<(Candidate, PathBuf)> = candidates
        .drain(..)
        .map(|candidate| {
            let key = fs::canonicalize(&candidate.path).unwrap_or_else(|_| candidate.path.clone());
            (candidate, key)
        })
        .collect();
    entries.sort_by(|(left, left_key), (right, right_key)| {
        left_key
            .components()
            .count()
            .cmp(&right_key.components().count())
            .then_with(|| right.size.cmp(&left.size))
    });
    let mut kept: Vec<(Candidate, PathBuf)> = Vec::with_capacity(entries.len());
    for (candidate, key) in entries {
        if kept.iter().any(|(_, other_key)| key.starts_with(other_key)) {
            continue;
        }
        kept.push((candidate, key));
    }
    *candidates = kept.into_iter().map(|(candidate, _)| candidate).collect();
}

// ---------------------------------------------------------------------------
// Deletion
// ---------------------------------------------------------------------------

pub fn delete_selected(candidates: &[Candidate]) -> CleanupReport {
    let mut report = CleanupReport::default();
    for candidate in candidates.iter().filter(|candidate| candidate.selected) {
        match remove_candidate(candidate) {
            Ok(()) => {
                report.deleted += 1;
                report.freed = report.freed.saturating_add(candidate.size);
            }
            Err(error) => report
                .errors
                .push(format!("{}: {error}", candidate.path.display())),
        }
    }
    report
}

fn remove_candidate(candidate: &Candidate) -> Result<(), String> {
    let path = &candidate.path;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err("not a removable directory (symlinks are refused)".to_owned());
    }

    let canonical = fs::canonicalize(path).map_err(|error| error.to_string())?;
    let root = fs::canonicalize(&candidate.root).map_err(|error| error.to_string())?;
    if candidate.global {
        if canonical != root {
            return Err("global cache changed location; refusing to delete".to_owned());
        }
    } else {
        if canonical == root {
            return Err("refusing to delete the scan root".to_owned());
        }
        if !canonical.starts_with(&root) {
            return Err("path escaped the scanned root; refusing to delete".to_owned());
        }
    }

    let cwd = env::current_dir().ok();
    let home = home_dir();
    if let Some(cwd) = &cwd
        && cwd.starts_with(&canonical)
    {
        return Err("refusing to delete a directory containing the current directory".to_owned());
    }
    if let Some(home) = &home
        && home.starts_with(&canonical)
    {
        return Err("refusing to delete a directory containing the home directory".to_owned());
    }
    for protected in protected_paths(cwd.as_deref(), home.as_deref()) {
        if canonical == protected {
            return Err(format!(
                "refusing to delete protected path {}",
                protected.display()
            ));
        }
    }

    // Re-check after canonicalization: never follow a symlink swap.
    let current = fs::symlink_metadata(&canonical).map_err(|error| error.to_string())?;
    if !current.file_type().is_dir() || current.file_type().is_symlink() {
        return Err("candidate is no longer a plain directory".to_owned());
    }
    fs::remove_dir_all(&canonical).map_err(|error| error.to_string())
}

fn protected_paths(cwd: Option<&Path>, home: Option<&Path>) -> Vec<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/"),
        PathBuf::from("/proc"),
        PathBuf::from("/sys"),
        PathBuf::from("/dev"),
        PathBuf::from("/etc"),
        PathBuf::from("/boot"),
        PathBuf::from("/usr"),
        PathBuf::from("/var"),
        PathBuf::from("/tmp"),
        PathBuf::from("/home"),
        PathBuf::from("/root"),
    ];
    if let Some(cwd) = cwd {
        paths.push(cwd.to_path_buf());
    }
    if let Some(home) = home {
        paths.push(home.to_path_buf());
    }
    paths
}

/// Validate that a path is a plain directory and resolve a display-safe
/// canonical path without changing it. Used by tests and diagnostics.
#[must_use]
pub fn is_plain_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_dir() && !metadata.file_type().is_symlink())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::scan::{ScanOptions, Scanner};

    fn tree_for(temp: &TempDir) -> Tree {
        Scanner::new(ScanOptions::default()).scan(&[temp.path().to_path_buf()])
    }

    #[test]
    fn detects_marker_backed_build_and_dependency_dirs() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "").unwrap();
        fs::create_dir(temp.path().join("target")).unwrap();
        fs::write(temp.path().join("target/artifact"), vec![b'x'; 4096]).unwrap();
        fs::write(temp.path().join("package.json"), "{}").unwrap();
        fs::create_dir(temp.path().join("node_modules")).unwrap();
        fs::write(temp.path().join("node_modules/mod.js"), vec![b'y'; 2048]).unwrap();
        fs::create_dir(temp.path().join("__pycache__")).unwrap();
        fs::write(temp.path().join("__pycache__/x.pyc"), vec![b'z'; 1024]).unwrap();

        let tree = tree_for(&temp);
        let settings = Settings::default();
        let detection = detect(&tree, &settings);
        let target = detection
            .candidates
            .iter()
            .find(|candidate| candidate.path.ends_with("target"))
            .expect("target candidate");
        assert_eq!(target.category, Category::Build);
        assert!(target.selected);
        let node = detection
            .candidates
            .iter()
            .find(|candidate| candidate.path.ends_with("node_modules"))
            .expect("node_modules candidate");
        assert_eq!(node.category, Category::Dependency);
        assert!(!node.selected);
        let pycache = detection
            .candidates
            .iter()
            .find(|candidate| candidate.path.ends_with("__pycache__"))
            .expect("pycache candidate");
        assert_eq!(pycache.category, Category::Cache);
        assert!(pycache.selected);
    }

    #[test]
    fn unmarked_generic_directories_are_not_offered() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("target")).unwrap();
        fs::create_dir(temp.path().join("build")).unwrap();
        fs::create_dir(temp.path().join("node_modules")).unwrap();
        let detection = detect(&tree_for(&temp), &Settings::default());
        assert!(detection.candidates.is_empty());
    }

    #[test]
    fn nested_candidates_are_collapsed_to_the_shallowest() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("Cargo.toml"), "").unwrap();
        fs::create_dir_all(temp.path().join("target/debug/__pycache__")).unwrap();
        let detection = detect(&tree_for(&temp), &Settings::default());
        let paths: Vec<_> = detection
            .candidates
            .iter()
            .map(|candidate| candidate.path.clone())
            .collect();
        assert_eq!(paths, vec![temp.path().join("target")]);
    }

    #[test]
    fn profiles_parse_and_merge() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            r#"
[cleanup]
profile = "work"
include_global = false
extra_cache = ["scratch-cache"]

[cleanup.profiles.work]
default = "dependencies"
include_global = true
extra_dependencies = ["vendor-custom"]
"#,
        )
        .unwrap();
        let (settings, warnings) = load_settings(Some(&path), None).unwrap();
        assert!(warnings.is_empty());
        assert!(settings.include_global);
        assert_eq!(settings.default_selection, SelectionPreset::Dependencies);
        assert_eq!(settings.profile, "work");
    }

    #[test]
    fn unknown_profile_is_an_error() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(&path, "[cleanup]\nprofile = \"safe\"\n").unwrap();
        assert!(load_settings(Some(&path), Some("missing")).is_err());
    }

    #[test]
    fn deletion_refuses_symlinks_and_roots() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("artifact"), "x").unwrap();
        let candidate = Candidate {
            path: target.clone(),
            root: root.clone(),
            size: 1,
            category: Category::Build,
            language: "rust".to_owned(),
            reason: "test".to_owned(),
            global: false,
            selected: true,
        };
        let report = delete_selected(std::slice::from_ref(&candidate));
        assert_eq!(report.deleted, 1);
        assert!(!target.exists());

        let outside = temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let escaped = Candidate {
            path: outside.clone(),
            root: target.clone(),
            size: 1,
            category: Category::Build,
            language: "rust".to_owned(),
            reason: "test".to_owned(),
            global: false,
            selected: true,
        };
        let report = delete_selected(&[escaped]);
        assert!(report.failed());
        assert!(outside.exists());
    }

    #[test]
    fn deletion_only_touches_selected_candidates() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("artifact"), "x").unwrap();
        let candidate = Candidate {
            path: target.clone(),
            root: root.clone(),
            size: 1,
            category: Category::Build,
            language: "rust".to_owned(),
            reason: "test".to_owned(),
            global: false,
            selected: false,
        };
        let report = delete_selected(&[candidate]);
        assert_eq!(report.deleted, 0);
        assert!(target.exists());
    }

    #[test]
    fn deletion_rejects_scan_roots_and_symlink_candidates() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().to_path_buf();
        let root_candidate = Candidate {
            path: root.clone(),
            root: root.clone(),
            size: 1,
            category: Category::Build,
            language: "rust".to_owned(),
            reason: "test".to_owned(),
            global: false,
            selected: true,
        };
        let report = delete_selected(&[root_candidate]);
        assert!(report.failed());
        assert!(root.exists());

        #[cfg(unix)]
        {
            let real = root.join("real");
            fs::create_dir(&real).unwrap();
            let link = root.join("target-link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let linked = Candidate {
                path: link,
                root: root.clone(),
                size: 1,
                category: Category::Build,
                language: "rust".to_owned(),
                reason: "test".to_owned(),
                global: false,
                selected: true,
            };
            let report = delete_selected(&[linked]);
            assert!(report.failed());
            assert!(real.exists());
        }
    }

    #[test]
    fn custom_patterns_are_matched_by_name() {
        let temp = TempDir::new().unwrap();
        fs::create_dir(temp.path().join("my-cache")).unwrap();
        fs::write(temp.path().join("my-cache/x"), "x").unwrap();
        let settings = Settings {
            extra_cache: vec![
                GlobBuilder::new("*-cache")
                    .build()
                    .unwrap()
                    .compile_matcher(),
            ],
            ..Settings::default()
        };
        let detection = detect(&tree_for(&temp), &settings);
        assert_eq!(detection.candidates.len(), 1);
        assert_eq!(detection.candidates[0].category, Category::Cache);
        assert!(detection.candidates[0].selected);
    }

    #[test]
    fn plain_directory_check_rejects_symlinks() {
        let temp = TempDir::new().unwrap();
        let real = temp.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = temp.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(is_plain_directory(&real));
        #[cfg(unix)]
        assert!(!is_plain_directory(&link));
    }
}
