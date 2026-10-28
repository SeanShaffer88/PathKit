# QUALITY — PathKit v0.1.0

Scored from evidence only. Gate numbers are from the actual local gate run at build time (WinDevKit `tools/gate.sh`, this machine).

| Dimension | Score /5 | Evidence |
|---|---|---|
| Correctness | 4 | 28/28 tests pass, each behaviour in the header pinned by a test: dot-segment resolution and `escapes_root` clamping, UNC/device/drive-relative parsing and rebuild, reserved names incl. `CON.txt` and the `CONSOLE`/`COM10` near-misses, UTF-16 lengths for emoji (2 units) and CJK (1 unit), join/relative matrices incl. root-mismatch errors, extended round trips. Not 5: Windows itself was not used as an oracle — all expectations are from the documented Windows rules, not from running the paths through a real Windows filesystem. |
| Error handling | 4 | Every exported function returns NULL + thread-local code/message on bad input (null, non-UTF-8 by contract, empty where documented, malformed/unknown/duplicate-key options JSON); FFI tests pin codes 1, 7 and 8. `pa_relative` refuses different roots and non-absolute inputs with plain-language messages instead of inventing an answer. Not 5: no fuzzing of the parser has been run. |
| Tests | 4 | 28 tests: 21 core (all fixtures are synthetic path strings; no filesystem is touched), 4 for the JSON engine, 3 FFI end-to-end. Coverage % not measured (no coverage tool in the offline toolchain) — the per-behaviour test list in README is the evidence. |
| Code structure | 4 | One responsibility per module (`lib` core, `json`, `ffi`, empty documented `platform` stub); clippy clean on host and on `x86_64-pc-windows-msvc`, both with `-D warnings`. |
| Docs | 4 | Header documents every exported function with exact semantics, every JSON field, every issue code and every error code; README is TL;DR-first with a C example and honest limitations. Not 5: no generated API doc site, and the header is the only reference by design. |
| Performance | 4 | Pure string work, linear in path length, no allocation-heavy machinery beyond the result strings; no benchmarks run (paths are short strings in practice) — 4, not 5, on that basis. |
| Packaging | 3 | Standard family layout (cdylib+rlib+staticlib, release profile with lto/strip/panic=abort, `ci.yml` copied from the WinDevKit template). Not higher: no release has been built, published or re-downloaded — local files only at this stage, and the Windows link step is CI's job, not yet run for this repo. |

**Waivers:** none. **Fails:** none known at gate time.

## Gate transcript (summary)

- `cargo fmt --all -- --check` — clean (after one `cargo fmt` pass during the build).
- `cargo clippy --all-targets` (host) — 0 warnings, 0 errors.
- `cargo clippy --target x86_64-pc-windows-msvc --all-targets` — clean; `cargo check` for that target clean.
- `cargo test` (full suite) — **28 passed, 0 failed**.
- Gate script verdict: **GATE GREEN**.
- One test expectation was corrected during the build: the UTF-16 length of `C:/😀/file` is 10 units, not 13 as first written in the test — the library was right, the test's arithmetic was wrong, and the corrected assertion now documents the count inline.
- On-Windows runtime behaviour: **Unverified** — PathKit makes no OS calls, so there is no Windows-only behaviour to field-test beyond the CI link/build, which has not run yet for this repo.
