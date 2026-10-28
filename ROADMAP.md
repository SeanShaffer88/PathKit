# PathKit Roadmap

## v0.1.0 (current)
The long-path and Unicode compatibility layer behind a C ABI: lexical normalization (six path kinds, dot-segment resolution with `escapes_root` clamping and reporting), plain-language validation (reserved names, illegal characters, trailing dots/spaces, over-long and empty components, control characters), split, Windows-rule joins, case-insensitive relative paths with a clean different-root error, extended-length (`\\?\`) conversion both ways, and UTF-16/UTF-8 length reporting with legacy `MAX_PATH` flags. Pure string logic, zero dependencies, no OS calls.

## Next, in order
1. **Alternate data stream awareness**: recognize and report a `name:stream` suffix explicitly in validation and split, instead of the current behaviour (the colon is flagged as an illegal character in a component, which is safe but does not explain streams to a caller who meant one).
2. **Batch mode**: one call that normalizes or validates a JSON array of paths and returns an array of results, for file-manager-style bulk checks without per-path call overhead.
3. **8.3 short-name hint**: flag components that would change under 8.3 short-name generation (spaces, multiple dots, long names) as informational issues, for callers targeting media or tools that still surface short names.

## Explicitly not planned
- Full Unicode normalization (NFC/NFD): it needs large generated tables, which breaks the zero-dependency rule for a heuristic problem; the `combining_sequence` flag stays the honest scope.
- Filesystem-backed resolution (existence, symlink/junction targets, per-drive current directories): that is a different, OS-coupled library; PathKit's value is that its answers are lexical and identical everywhere.
- Non-Windows naming rules (POSIX reserved names, macOS rules): `posix_absolute` parsing exists only so Windows-rooted paths are classified correctly, not as a promise of POSIX validation.
