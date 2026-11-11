# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-11-11

### Added

- Interactive cleanup mode: `dulens --cleanup` scans build caches, tool caches,
  and dependency directories, preselects safe build/cache entries, and lets you
  browse, filter, deselect, and confirm removal before anything is deleted.
- Marker-backed detection for Cargo, Go, Node/npm/yarn/pnpm/bun, Python, Java,
  Kotlin, Scala, C/C++, .NET, Swift, Ruby, PHP, Elixir/Erlang, Haskell, Zig,
  Dart/Flutter, Terraform, R, Crystal, OCaml, Elm, Unity, Unreal, and common
  global package/cache locations.
- Cleanup profiles `safe`, `deep`, `dependencies`, and `all`, plus custom
  profiles, extra cache/dependency globs, and global-cache toggles in
  `~/.config/dulens/config.toml` (or `--config` / `DULENS_CONFIG`).
- Defensive deletion: canonical path containment, symlink refusal, protected
  path checks, explicit confirmation, and per-path error reporting.

### Changed

- `--clean` remains the compatibility alias for `--plain`; the new cleanup
  entry point is `--cleanup` (alias `--clean-caches`).
- Added the new full-screen cleanup checklist with category filters, size/name
  sorting, selection summaries, and an ASCII-safe text fallback.

## [0.1.0] - 2026-11-07

### Added

- Initial release of `dulens`, a read-only disk-usage explorer for the terminal.
- Readable tree view with sizes, share of parent, gradient bars, depth control, sorting, glob excludes, and plain output.
- Interactive full-screen browser with navigation, paging, sorting, and metric switching.
- Allocated disk usage by default, with logical apparent sizes available through `--apparent-size`.
- Binary, decimal, and exact-byte size formats.
- Unix hard-link deduplication, symbolic-link safety, and single-filesystem scans.
- Prebuilt binaries for Linux x86-64, Linux ARM64, macOS Intel, and macOS Apple Silicon.
- English and Simplified Chinese documentation.

### Notes

- Linux binaries are statically linked against musl and run on any modern Linux distribution.
- macOS binaries target macOS 11 or newer.
- SHA-256 checksums are published next to each archive.
- Linux x86-64: `dulens-v0.1.0-x86_64-unknown-linux-musl.tar.gz`
- Linux ARM64: `dulens-v0.1.0-aarch64-unknown-linux-musl.tar.gz`
- macOS Intel: `dulens-v0.1.0-x86_64-apple-darwin.tar.gz`
- macOS Apple Silicon: `dulens-v0.1.0-aarch64-apple-darwin.tar.gz`

[Unreleased]: https://github.com/summertools/dulens/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/summertools/dulens/releases/tag/v0.2.0
[0.1.0]: https://github.com/summertools/dulens/releases/tag/v0.1.0
