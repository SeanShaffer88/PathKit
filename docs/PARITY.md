# Parity — PathKit v0.1.0

What the C ABI offers versus what the Rust library offers, and how the
documented Windows path rules map onto the implementation. Scored only
from test and build evidence in this repo.

## C ABI ↔ Rust library

Every capability exists first as a Rust function in `src/lib.rs`; the
FFI layer in `src/ffi.rs` is a thin wrapper over it (string in, owned
string out, thread-local error). There are no C-only or Rust-only
operations.

| Capability | Rust | C ABI | Evidence |
|---|---|---|---|
| Version / last error / free string | constants + thread-local in `ffi` | `pa_version`, `pa_last_error`, `pa_last_error_code`, `pa_free_string` | FFI tests read error codes 1, 7, 8 back |
| Normalize with options | `normalize` + `parse_options` | `pa_normalize` | `basic_drive_path`, `separators_and_dot_segments`, `options_errors`, FFI normalize test |
| Validate | `validate` / `validate_json` | `pa_validate` | reserved/illegal/trailing/long/empty/control tests + FFI test |
| Join | `join` | `pa_join` | `join_matrix` + FFI test |
| Relative | `relative` | `pa_relative` | `relative_matrix`, `relative_errors` + FFI test |
| Extended conversion | `to_extended` / `from_extended` | `pa_to_extended` / `pa_from_extended` | `extended_round_trip` + FFI test |
| Split | `split_json` | `pa_split` | `split_json_shape` + FFI test |

## Windows path rules ↔ implementation

| Rule | Status | Evidence |
|---|---|---|
| Both `\` and `/` are separators on input | Covered | forward-slash inputs throughout the tests |
| Duplicate separators collapse; UNC/device keep two leading | Covered | `separators_and_dot_segments`, `unc_paths`, `device_paths` |
| `.` dropped, `..` resolved lexically | Covered | `separators_and_dot_segments`, `join_matrix` |
| `..` at an absolute root is clamped and reported (`escapes_root`) | Covered | `escapes_root_is_clamped_and_reported` (drive + UNC) |
| Leading `..` of a relative path is kept | Covered | `drive_relative_and_posix_and_relative` |
| Trailing separator stripped except at a root | Covered | `roots_and_trailing_separators`, UNC root case |
| Drive-relative paths (`C:foo`) get no root separator and no extended prefix | Covered | `drive_relative_and_posix_and_relative`, `extended_round_trip` |
| Extended form: `\\?\` for drives, `\\?\UNC\` for UNC, round-trips | Covered | `device_paths`, `extended_round_trip` |
| Legacy MAX_PATH flag at >260 UTF-16 units of the Windows form; per-component limit 255 UTF-16 units | Covered | `long_path_flags`, component-length boundary tests (255 ok, 256 flagged) |
| Reserved device names incl. with extensions, case-insensitive | Covered | `validate_reserved_names` (plus `CONSOLE`/`COM10` near-misses clean) |
| Illegal characters `< > : " \| ? *`; colon legal only as drive separator | Covered | `validate_illegal_chars_and_colon` |
| Trailing dot / trailing space per component | Covered | `validate_trailing_dot_space_long_empty_control` |
| UTF-16 counting: astral characters count 2, BMP characters count 1 | Covered | `unicode_lengths` (emoji = 2, CJK = 1) |
| Combining sequences flagged | Covered — heuristic only | `unicode_lengths`; documented as not-NFC in header, README and AGENTS.md |
| Relative paths: case-insensitive roots and components; extended/plain forms of one drive are one root | Covered | `relative_matrix` |
| Relative paths across drives/shares | Refused by design | `relative_errors` — `PA_ERR_DIFFERENT_ROOT` with a plain-language message |
| Symlink/junction resolution, existence, per-drive current directories | Gap — by design | Lexical-only scope, stated in README limitations and ROADMAP "not planned" |

## Platform parity

PathKit makes no OS calls, so host (Linux) and Windows results are the
same code path; the gate still runs clippy and check for
`x86_64-pc-windows-msvc`. The Windows link of the cdylib is verified by
CI's release build, which has not run yet for this repo — that is the
one parity item still open.
