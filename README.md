# PathKit

## TL;DR

A small, zero-dependency DLL that handles the fiddly parts of Windows path strings for you: it tidies a path into a consistent form, tells you in plain words what is wrong with a path Windows would refuse, works out joined and relative paths, and converts long paths to the extended `\\?\` form that bypasses the old 260-character limit. It only works on the text of a path — it never opens a file.

```c
#include "pathkit.h"

char *j = pa_normalize("C:\\work\\..\\docs\\report.txt", NULL);
/* {"input":"C:\\work\\..\\docs\\report.txt",
 *  "normalized":"C:/docs/report.txt",
 *  "windows_form":"C:\\docs\\report.txt",
 *  "extended_form":"\\\\?\\C:\\docs\\report.txt",
 *  "is_absolute":true,"kind":"drive_absolute",
 *  "components":["docs","report.txt"],
 *  "file_name":"report.txt","extension":"txt",
 *  "parent":"C:/docs","utf16_len":18,"byte_len":18,
 *  "exceeds_legacy_max_path":false,
 *  "needs_extended_prefix":false,"issues":[]} */
pa_free_string(j);

char *v = pa_validate("C:\\CON.txt");
/* {"valid":false,"issues":[{"code":"reserved_name",
 *   "component":"CON.txt","message":"... Windows reserves ..."}]} */
pa_free_string(v);
```

## Features

- **Normalization** (`pa_normalize`) — one consistent separator, duplicate separators collapsed, `.` and `..` resolved by the rules in the header, trailing separators stripped except at a root. A `..` that would climb above a root is removed and reported (`escapes_root`), never silently kept. The result carries the normalized form, the Windows form, the extended form, the path kind (`drive_absolute`, `drive_relative`, `unc`, `device`, `posix_absolute`, `relative`), components, file name, extension, parent, and lengths.
- **Plain-language validation** (`pa_validate`) — empty paths, control characters, the characters Windows forbids (`< > : " | ? *`, with a colon allowed only after a drive letter), reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`, `LPT1`–`LPT9`, including forms like `CON.txt`), trailing dots and spaces, components over 255 characters, and empty components in the middle of a path. Each problem comes with a code, the component at fault, and a sentence explaining what to do.
- **Long-path answers** — every normalize result reports the length in UTF-16 code units (the way Windows counts) and in UTF-8 bytes, whether the Windows form passes the legacy 260-character `MAX_PATH` limit, whether it needs the extended prefix, and the extended form itself. `pa_to_extended` / `pa_from_extended` convert back and forth as plain strings.
- **Join and relative** (`pa_join`, `pa_relative`) — joining follows Windows rules (an absolute part replaces the base, a root-relative part keeps the base's drive or share, a drive-relative part joins only on the same drive). Relative paths are computed case-insensitively, and paths on different drives or shares fail cleanly with `PA_ERR_DIFFERENT_ROOT` and a plain message instead of a made-up answer.
- **Split** (`pa_split`) — root plus dot-resolved components as JSON.
- **Unicode, honestly** — lengths count characters above U+FFFF (most emoji) as 2 UTF-16 units, matching Windows. Components containing combining characters (accents stored as separate characters) are flagged with the informational `combining_sequence` code.

## Building

```sh
cargo build --release   # produces pathkit.dll / libpathkit.so / libpathkit.a
cargo test              # the full test suite
```

Rust 1.80 or later. No dependencies.

## C usage

All strings are UTF-8 and NUL-terminated. Every `char *` result is owned by the caller and must be freed with `pa_free_string`. On failure a function returns `NULL` and sets a thread-local error readable with `pa_last_error()` and `pa_last_error_code()`.

```c
#include "pathkit.h"
#include <stdio.h>

int main(void) {
    char *joined = pa_join("C:\\work\\project", "..\\shared\\logo.png");
    if (!joined) { printf("error %d: %s\n", pa_last_error_code(), pa_last_error()); return 1; }
    printf("%s\n", joined);   /* normalize-shaped JSON: normalized = C:/work/shared/logo.png */
    pa_free_string(joined);

    char *rel = pa_relative("C:\\work\\project", "C:\\work\\shared\\logo.png");
    if (rel) { printf("%s\n", rel); pa_free_string(rel); }
    /* {"relative":"../shared/logo.png", ...} */

    char *ext = pa_to_extended("C:\\very\\long\\path\\file.txt");
    printf("%s\n", ext);      /* \\?\C:\very\long\path\file.txt */
    pa_free_string(ext);
    return 0;
}
```

See `include/pathkit.h` for the exact semantics of every function, every JSON field, and every issue and error code — the header is the contract.

## Tests

The suite (28 tests) works on synthetic path strings only — no filesystem is touched. It covers: UNC, device (`\\?\`, `\\?\UNC\`, `\\.\`) and drive-relative (`C:foo`) paths, a path deeper than 260 characters and its extended-prefix flags, `..` escaping a root, reserved names (including `CON.txt` and near-misses like `CONSOLE` and `COM10`), trailing dots and spaces, emoji and CJK UTF-16 lengths, a combining-character flag, join and relative matrices (including different-drive and different-share failures), extended-form round trips, and the FFI surface end to end (null arguments, bad options JSON, error codes).

## Honest limitations

- **Lexical only.** PathKit reasons about the text of a path. It cannot tell you whether a file exists, whether a `..` crosses a symlink or junction (only the disk knows that), or what a drive-relative path like `C:foo` really points at (that depends on a per-drive current directory PathKit does not know).
- **No Unicode normalization.** Names pass through unchanged. The `combining_sequence` flag is a heuristic based on the common combining-mark ranges — it is not NFC/NFD checking, and a name can be non-NFC without combining marks in those ranges, or perfectly fine with them.
- **Validation is the documented rule set, not every rule.** It covers the classic Windows naming rules listed above. It does not check share or server name syntax on a network, filesystem-specific rules (for example a particular network filesystem's own limits), or whether a name is valid for a non-Windows system.
- **Length counting follows the documented definition.** `exceeds_legacy_max_path` means the Windows form is longer than 260 UTF-16 code units; individual programs may apply the limit differently (some count a terminating NUL against it).

License: MIT.
