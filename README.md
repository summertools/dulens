# dulens

**Explore disk usage with a readable tree or an interactive terminal UI.**

English | [简体中文](README.zh-CN.md)

`dulens` scans files and directories and shows where the space went. It offers a
colored tree with sizes, percentages, and bar charts, a full-screen
keyboard-driven browser, and a plain layout for scripts and logs. Normal
scanning is read-only and requires no configuration; the optional cleanup mode
only deletes explicitly selected folders after a confirmation screen.

## Contents

- [Overview](#overview)
- [Features](#features)
- [Sample output](#sample-output)
- [Installation](#installation)
- [Usage](#usage)
  - [Options](#options)
  - [Environment variables](#environment-variables)
  - [Interactive keys](#interactive-keys)
- [Cleanup mode](#cleanup-mode)
- [Understanding the output](#understanding-the-output)
- [How sizes are measured](#how-sizes-are-measured)
- [Excluding paths](#excluding-paths)
- [Exit status and warnings](#exit-status-and-warnings)
- [Plain mode](#plain-mode)
- [Color and terminal width](#color-and-terminal-width)
- [Examples](#examples)
- [Development](#development)
- [Project layout](#project-layout)
- [Platform notes and limitations](#platform-notes-and-limitations)
- [License](#license)

## Overview

By default, `dulens` reports **allocated disk usage** (similar to `du`), counts
hard links once on Unix, does not follow symbolic links, and displays two
levels below each given path. Typical uses are:

- `dulens` — a quick overview of the current directory.
- `dulens -d 3 -a ~/projects` — include files and look three levels deep.
- `dulens -i /var` — browse interactively and drill down with the keyboard.
- `dulens --plain -d 3 .` — stable, color-free output for scripts or logs.

When several paths are given, their sizes are combined. Each path appears as a
top-level root, and top-level percentages use the combined total.

## Features

- Readable tree with sizes, share of parent, and sub-cell gradient bars.
- Full-screen interactive browser with navigation, paging, sorting, and metric
  switching.
- `du`-style allocated disk usage by default, with logical apparent sizes
  available through `--apparent-size`.
- On Unix, hard links are counted once; symbolic links are never followed.
- Optional single-filesystem scans with `--one-file-system`.
- Repeatable glob excludes by entry name or path.
- Width-aware rendering that fits narrow terminals and wide Unicode names.
- Control characters and Unicode bidirectional overrides in file names are
  escaped before printing, so hostile names cannot inject terminal escapes or
  spoof the layout.
- Plain mode without colors, bars, percentages, tree connectors, or a header in
  static output.
- Safe interactive cleanup mode for marker-backed build caches and dependency
  directories across major language toolchains, with profiles, previews and
  explicit confirmation before deletion.
- Honest exit status: scan errors are reported on stderr and produce a failure
  status.

## Sample output

The example below uses `--apparent-size` so the numbers do not depend on
filesystem block allocation:

```console
$ dulens --apparent-size --all -d 2 /tmp/dulens-demo
dulens  311 KiB  ·  5 files  ·  5 dirs  ·  apparent

 311 KiB  100%  ████████████  /tmp/dulens-demo/
 240 KiB   77%  █████████▎░░  ├─ target/
 240 KiB  100%  ████████████  │  └─ debug/ …
48.0 KiB   15%  █▉░░░░░░░░░░  ├─ assets/
48.0 KiB  100%  ████████████  │  └─ logo.png
20.0 KiB    6%  ▊░░░░░░░░░░░  ├─ src/
12.0 KiB   60%  ███████▎░░░░  │  ├─ main.rs
8.00 KiB   40%  ████▊░░░░░░░  │  └─ lib.rs
3.00 KiB    1%  ▏░░░░░░░░░░░  └─ README.md
```

The same tree in plain mode:

```console
$ dulens --plain --apparent-size --all -d 2 /tmp/dulens-demo
 311 KiB  /tmp/dulens-demo/
 240 KiB    target/
 240 KiB      debug/ …
48.0 KiB    assets/
48.0 KiB      logo.png
20.0 KiB    src/
12.0 KiB      main.rs
8.00 KiB      lib.rs
3.00 KiB    README.md
```

The decorated sample was captured in an 88-column terminal; the exact bar width
depends on the terminal width. The trailing `…` marks a directory whose deeper
entries were not shown because of the `--depth` limit.

## Installation

`dulens` is a Rust binary. You need a recent stable Rust toolchain (Rust 1.88
or newer, edition 2024) and Cargo.

Build from source:

```bash
git clone https://github.com/summertools/dulens.git
cd dulens
cargo build --release
./target/release/dulens --help
```

Install the binary into Cargo's bin directory:

```bash
cargo install --path .
```

Or install directly from the repository:

```bash
cargo install --git https://github.com/summertools/dulens.git
```

After installation the command is simply `dulens`.

## Usage

```text
dulens [OPTIONS] [PATH]...
```

If no path is given, `dulens` scans the current directory (`.`). Options may
appear before or after paths.

```bash
dulens                         # current directory, depth 2
dulens -d 3 -a ~/projects      # include files, three levels deep
dulens -i /var                 # interactive browser
dulens --plain -d 3 .          # decorate nothing, stay pipe-friendly
```

Without `-i`, the result is written to stdout and warnings are written to
stderr. With `-i`, `dulens` takes over the terminal until you quit.

### Options

| Option | Default | Description |
| --- | --- | --- |
| `PATH...` | `.` | One or more paths to inspect. Multiple paths are combined; each appears as a top-level root. |
| `-d, --depth <N>` | `2` | Levels of children to display below each root. `0` shows roots only. |
| `-a, --all` | off | Show files as well as directories. Alias: `--files`. |
| `-i, --interactive` | off | Start the full-screen interactive browser. |
| `--cleanup` | off | Open the interactive build-cache and dependency cleanup checklist. Requires a terminal. |
| `--clean-profile <NAME>` | `safe` | Cleanup preset: `safe`, `deep`, `dependencies`, `all`, or a profile from the config file. |
| `--config <FILE>` | platform config | Read cleanup presets from `FILE`; requires `--cleanup`. |
| `-p, --plain` | off | Clean monochrome layout; in static output it omits the header, bars, percentages, colors, and tree connectors. Aliases: `--simple`, `--clean`. |
| `--apparent-size` | off | Count logical file lengths instead of allocated disk blocks. |
| `--si` | off | Use powers of 1000 (`kB`, `MB`, ...). Conflicts with `--bytes`. |
| `--bytes` | off | Print exact byte counts. Conflicts with `--si`. |
| `--color <WHEN>` | `auto` | Color mode: `auto`, `always`, or `never`. |
| `--no-color` | off | Disable color regardless of `--color`. |
| `-s, --sort <KEY>` | `size` | Sort by `size` (largest first) or `name` (A-Z, case-insensitive). |
| `-r, --reverse` | off | Reverse the final sort order. |
| `-x, --one-file-system` | off | Do not cross filesystem boundaries (Unix only). |
| `-E, --exclude <GLOB>` | none | Skip entries matching the glob; repeatable. |
| `-q, --quiet` | off | Suppress scan warnings. Exit status still reflects errors. |
| `-h, --help` | | Print help. |
| `-V, --version` | | Print version. |

Notes:

- `--si` uses `kB`, `MB`, `GB`, and so on; the default is binary with `KiB`,
  `MiB`, `GiB`, and so on.
- `--bytes` prints exact counts such as `1234 B`.
- `--no-color` and `--plain` both override `--color always`.
- `-i` requires a terminal on both stdin and stdout; otherwise `dulens` exits
  with an error instead of producing a broken UI.

### Environment variables

| Variable | Effect |
| --- | --- |
| `NO_COLOR` | If set to any value, disables color in `auto` mode. |
| `CLICOLOR` | `CLICOLOR=0` disables color in `auto` mode. |
| `TERM` | `TERM=dumb` disables color in `auto` mode. A value containing `256color` selects the 256-color palette. |
| `COLORTERM` | A value containing `truecolor` or `24bit` selects 24-bit color. |
| `COLUMNS` | Output width when stdout is not a terminal. Values below `20` are ignored; the default is `100`. |
| `DULENS_CONFIG` | Cleanup config path when `--config` is not given (default: `~/.config/dulens/config.toml`). |

`NO_COLOR`, `CLICOLOR`, `TERM`, and `COLORTERM` only affect `--color auto`,
which is the default; they do not override `--color always`, `--plain`, or
`--no-color`. `COLUMNS` only controls the output width when stdout is not a
terminal.

### Interactive keys

| Key | Action |
| --- | --- |
| `↑` / `k` | Move selection up |
| `↓` / `j` | Move selection down |
| `PgUp` / `PgDn` | Move one page |
| `g` / `Home` | Jump to the first entry |
| `G` / `End` | Jump to the last entry |
| `Enter` / `l` / `→` | Open the selected directory |
| `Backspace` / `h` / `←` | Go to the parent directory |
| `s` | Toggle sort key: size / name |
| `S` | Reverse the sort order |
| `a` | Toggle metric: disk / apparent |
| `q` | Quit |
| `Esc` | Go up; when already at the top, quit |
| `Ctrl+C` | Quit |

If exactly one directory root was scanned, the browser opens directly inside
it. Otherwise it starts at the synthetic "all paths" root. Interactive mode
always lists both files and directories; `--depth` and `--all` do not apply
there. Scan warnings are held until you leave the browser, then printed to
stderr unless `--quiet` is set.

## Cleanup mode

`dulens --cleanup` scans the given paths (the current directory by default) and
opens a checklist of regenerable build outputs, tool caches, and dependency
directories.

```bash
dulens --cleanup                     # safe profile, current directory
dulens --cleanup ~/projects          # scan several project trees
dulens --cleanup --clean-profile deep ~/projects
dulens --cleanup --config ~/.config/dulens/work.toml .
```

The initial selection is deliberately conservative:

- **BUILD** (selected): `target`, `build`, `cmake-build-*`, `bin`/`obj` for
  .NET, `_build`, `dist-newstyle`, `.stack-work`, `zig-out`, `.build` and
  similar marker-backed build directories. They are recreated by the normal
  language toolchain.
- **CACHE** (selected): `__pycache__`, `.pytest_cache`, `.mypy_cache`,
  `.ruff_cache`, `.gradle`, `.next`, `.nuxt`, `.svelte-kit`, `.turbo`,
  `.parcel-cache`, `.vite`, Zig caches, and similar regenerable tool caches.
- **DEPS** (not selected): `node_modules`, `vendor` for Go/PHP/Ruby, Python
  virtual environments, Elixir `deps`, iOS `Pods`, and global package stores.
  These can be restored, but reinstalling them may require network access, so
  they are never selected by default; use a profile or toggle them explicitly.

A directory is only identified when the expected project marker is present:
`Cargo.toml` for Cargo `target`, `package.json` or a lockfile for
`node_modules`, `go.mod` for Go `vendor`, `mix.exs` for Elixir, and so on. This
avoids offering unrelated directories that merely happen to be named `build`.

### Cleanup keys

| Key | Action |
| --- | --- |
| `↑` / `k`, `↓` / `j` | Move through candidates |
| `PgUp` / `PgDn`, `g` / `G` | Page, first, last |
| `Space` | Toggle the selected folder |
| `a` / `n` | Select all / none in the current filter |
| `f`, `Tab`, `←` / `→` | Cycle filters: all, build, cache, deps |
| `s` | Toggle size/name sorting |
| `Enter` | Review the final list and open confirmation |
| `y` | Confirm permanent deletion (only in the confirmation screen) |
| `n` / `Esc` / `q` | Cancel the confirmation or leave cleanup |

Nothing is deleted while browsing. The confirmation screen lists every selected
path and the total size, and only `y` performs the removal.

### Profiles and configuration

The config file is optional. The default location is
`~/.config/dulens/config.toml` (or `$XDG_CONFIG_HOME/dulens/config.toml`);
`--config` and `DULENS_CONFIG` override it. An explicit config error is reported
as a failure, while a malformed implicit config falls back to safe defaults with
a warning.

```toml
[cleanup]
profile = "safe"          # default profile
include_global = true     # include known global build/tool caches
include_global_dependencies = false  # also include global package stores
default = "build"         # build | dependencies | all | none
extra_cache = ["scratch-cache", "*-tmp-cache"]
extra_dependencies = ["vendor-custom"]

[cleanup.profiles.work]
include_global = true
include_global_dependencies = true
default = "build"
extra_cache = ["ci-cache"]
```

Built-in profiles: `safe` (global build/tool caches, dependency entries
unselected), `deep` (global dependency stores visible), `dependencies`
(dependency/cache preselection), and `all` (everything preselected). A profile
defined in the file can override `include_global`,
`include_global_dependencies`, `default`, `extra_cache`, and `extra_dependencies`; CLI `--clean-profile` wins over `cleanup.profile`.

Global build/tool caches are included in the safe profile because they are
regenerable; global package stores need `include_global_dependencies = true`
(the `deep` or `dependencies` profile). Known locations include Cargo's registry
cache, Go's build and module caches, npm/yarn/pnpm/uv/pip caches,
Gradle/Maven/NuGet stores, ccache/sccache, Dart/pub, Deno, Bazel and Xcode
DerivedData (macOS). Environment overrides such as `CARGO_HOME`, `GOCACHE`,
`GOMODCACHE`, `PIP_CACHE_DIR`, and `GRADLE_USER_HOME` are respected.

### Deletion safety

Before removal, every candidate is revalidated as a plain directory (symlinks
are refused), canonicalized, and checked against the scan root. `dulens` refuses
to delete a scan root, a path containing the current directory or home, or the
usual protected system paths. A partially failed removal is reported per path
and makes the command exit nonzero. Global caches must still match their exact
known location. Reported sizes are allocated disk usage; hard-linked or shared
content can make the actual reclaimed space smaller.

## Understanding the output

### Decorated tree

```text
dulens  311 KiB  ·  5 files  ·  5 dirs  ·  apparent

 240 KiB   77%  █████████▎░░  ├─ target/
```

- **Header** — tool name, total size, file count, directory count, optional root
  count when more than one path was scanned, optional `apparent` marker, and
  the error count when errors occurred.
- **Size** — right-aligned; the column is at least seven characters wide.
- **Share** — the entry's size as a percentage of its parent. Top-level rows
  use the combined total of all roots. Values are rounded; small nonzero
  entries show `<1%`, and when bars are shown, a nonzero entry always gets at
  least a sliver.
- **Bar** — the filled portion is drawn with full and partial block characters
  (one-eighth cell precision) and colored with a teal-to-blue-to-violet
  gradient. The remainder is drawn dimmed. Bars disappear on narrow terminals.
- **Name** — tree connectors plus the entry name.

Annotations at the end of a name:

| Marker | Meaning |
| --- | --- |
| `/` | Directory |
| `@` | Symbolic link |
| `(hardlink)` | Additional hard-link entry; its size was already counted under another name, so it shows zero |
| `…` | Deeper entries exist but were not shown at the current depth |

Names are measured by display width, so CJK and other wide characters are
handled correctly. Names that do not fit are truncated with `…`; ordinary names
are truncated on the right, while root paths are truncated on the left to keep
the most specific part visible.

### Plain layout

Plain mode prints only a right-aligned size and an indented name:

```text
 311 KiB  /tmp/dulens-demo/
 240 KiB    target/
 240 KiB      debug/ …
```

It is designed for pipes, log files, diffs, and screenshots. It still honors
`--depth`, `--all`, sorting, excludes, and the computed output width.

## How sizes are measured

### Disk usage (default)

On Unix, disk usage is `st_blocks * 512` for each entry. This is allocated
space, not logical length: a sparse file can occupy far less than its apparent
size, and directories include their own metadata blocks. This matches the
default behavior of `du`.

On non-Unix platforms the disk metric falls back to the file length.

### Apparent size

With `--apparent-size`, files contribute their logical length (`st_size`).
Directories themselves contribute zero; their total is the sum of the files
inside them. A symbolic link contributes the length of the link text, because
links are not followed.

### Hard links

On Unix, files with a link count greater than one are deduplicated by
`(device, inode)` across the whole scan. The first entry encountered keeps the
size; later entries are displayed with zero size and the `(hardlink)` marker.
The file count still includes every directory entry, so a hard link is counted
as a file even when its size is not counted again.

### Symbolic links and directory identity

Symbolic links are never followed. They are listed as entries, counted as
files, and marked with `@`. They contribute only the size of the link itself.

Directories are identified by `(device, inode)` so the same directory is not
walked twice during a single run, for example when roots overlap or when bind
mounts expose the same inode through several paths.

### Filesystem boundaries

With `-x` / `--one-file-system`, entries whose device differs from the root's
device are skipped. This is useful for scanning a home directory or a mount
point without wandering into other filesystems. The option has no effect on
platforms where device identity is unavailable.

### Counts and errors

The file count includes every non-directory entry: regular files, symbolic
links, hard-link entries, and special files. The directory count includes every
scanned directory, including the root directory itself. Entries that cannot be
read are counted as errors; they do not contribute to any size, and they cause a
failure exit status.

## Excluding paths

`-E` / `--exclude` is repeatable and uses glob syntax:

- A pattern **without** a path separator (such as `target` or `*.log`) matches
  the entry name.
- A pattern **with** a separator (such as `src/generated`) matches the entry
  path as constructed from the root you passed. An implicit `**/` variant is
  also applied (unless the pattern is absolute or already starts with `**/`),
  so the same pattern usually matches at any depth.
- A trailing `/` or `\` is ignored, so `target/` behaves like `target`.
- `*` does not cross path separators; use `**` when you need to match across
  directories.
- Excluded entries are skipped before they are scanned, so their contents are
  not counted.
- Exclusions apply to entries discovered during the walk. Paths that you pass
  explicitly on the command line are always scanned.
- An invalid glob is reported before scanning starts and exits with status 1.

Examples:

```bash
dulens -E target -E node_modules -E .git .
dulens -a -E '*.log' .
dulens -E 'src/generated' .
dulens -E '**/cache' .
```

## Exit status and warnings

| Status | Meaning |
| --- | --- |
| `0` | Success. Also returned when the output pipe is closed early, for example by `head`. |
| `1` | Runtime failure: no readable paths, any scan error, an invalid exclude pattern, interactive mode without a TTY, or another I/O error. |
| `2` | Command-line usage error reported by the argument parser, such as an unknown flag or `--si` together with `--bytes`. |

Warnings are printed to stderr in the form:

```text
dulens: warning: /path: Permission denied
```

At most 32 detailed warnings are printed per run. Any remaining errors are
summarized as `N additional error(s) were suppressed`. `--quiet` suppresses
warnings and the summary, but it does not change the exit status. The fatal
message `dulens: no readable paths` is still printed when nothing could be
scanned.

## Plain mode

`-p` / `--plain` (alias `--simple`) produces a clean,
monochrome layout. In static output it omits the header, colors, bars,
percentages, and tree connectors; each entry gets one line with a right-aligned
size and an indented name. It still respects depth, files-versus-directories,
sorting, excludes, and width. It also works in the interactive browser, where
the header remains, colors, bars, and percentages are dropped, and the current
row is marked with `>`.

Plain mode is the most predictable format for scripts and reproducible logs.
Color is disabled even if `--color always` is also given.

## Color and terminal width

Color is selected in this order:

1. `--plain` or `--no-color` disables color.
2. Otherwise `--color auto|always|never` is used.
3. In `auto` mode, color requires stdout to be a terminal, `NO_COLOR` to be
   unset, `CLICOLOR` not to be `0`, and `TERM` not to be `dumb`.
4. When color is enabled, `COLORTERM` and `TERM` are used to pick 24-bit,
   256-color, or basic ANSI output.

The output width is chosen as follows:

- If stdout is a terminal, the real terminal width is used.
- Otherwise `COLUMNS` is used when it parses as a number of at least 20.
- Otherwise the width defaults to 100 columns.

At narrow widths, `dulens` drops the bar, then the percentage column, then tree
indentation, and finally truncates names with `…`, so every line stays within
the computed width. The interactive browser requires at least 40 columns by 8
rows; below that it shows a short hint instead of the browser.

## Examples

| Command | What it does |
| --- | --- |
| `dulens` | Current directory, two levels, directories only |
| `dulens -d 3 -a ~/projects` | Include files and look three levels deep |
| `dulens --plain -d 3 ~/projects` | Clean size-and-name output |
| `dulens --apparent-size --si /var` | Logical sizes with decimal units |
| `dulens -i --apparent-size /var` | Browse logical sizes interactively |
| `dulens --cleanup` | Review build caches and dependencies in the current tree |
| `dulens --cleanup --clean-profile deep ~/projects` | Include known global caches as optional candidates |
| `dulens --sort name --reverse -E target -E node_modules .` | Reverse name order while skipping build directories |
| `dulens -x -d 1 /home` | Stay on the starting filesystem |
| `dulens --bytes --apparent-size -d 1 .` | Exact byte counts, one level |

Closing the output pipe early also works normally:

```bash
dulens -a -q . | head -n 20
```

## Development

```bash
cargo build              # debug build
cargo build --release    # optimized build
cargo test               # unit and end-to-end tests
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

The test suite covers size formatting, percentages and bars, width fitting
(including wide Unicode names and very large sizes), sorting, excludes, hard
links, symlinks, error handling, plain mode, cleanup discovery, profile
resolution, deletion guards, and CLI behavior.

## Project layout

| Path | Purpose |
| --- | --- |
| `src/main.rs` | Binary entry point; parses arguments and maps errors to exit codes |
| `src/lib.rs` | Orchestrates scanning, rendering, color and width detection, warnings, and exit status |
| `src/cli.rs` | Command-line definition and flag-to-option mapping |
| `src/scan.rs` | Filesystem walk, size accounting, hard-link and directory deduplication, excludes |
| `src/cleanup.rs` | Marker-based cache/dependency discovery, profiles, and safe deletion |
| `src/cleanup_ui.rs` | Interactive cleanup checklist and confirmation screen |
| `src/format.rs` | Size, percentage, and bar formatting; width-aware truncation; name sanitization; color palette |
| `src/render.rs` | Non-interactive tree rendering |
| `src/tui.rs` | Interactive terminal browser |
| `tests/cli.rs` | End-to-end CLI tests against the built binary |

## Platform notes and limitations

- Linux and macOS are the primary targets. On non-Unix platforms the disk
  metric falls back to logical file length, and hard-link deduplication and
  `--one-file-system` are unavailable.
- The scan is sequential and single-threaded. Very large or remote trees can
  take time to traverse.
- The result is a snapshot taken at launch; there are no live updates.
- Normal scanning is read-only. Cleanup mode (`--cleanup`) is the only path
  that modifies the filesystem, and it never deletes anything until you select
  folders and press `y` on the confirmation screen.
- Interactive mode requires both stdin and stdout to be terminals, and needs at
  least a 40-by-8 terminal window.
- The interactive browser has no mouse support, search, filtering, or deletion.
- Only the first 32 scan warnings are described in detail.
- Paths passed explicitly on the command line are always scanned, even if they
  match an exclude pattern.

## License

`dulens` is distributed under the MIT License. The license is declared in the
`license` field of `Cargo.toml`.
