/* PathKit.dll — a long-path and Unicode compatibility
 * layer: path normalization, validation and conversion
 * with plain-language errors, behind one small C ABI.
 *
 * PathKit works on path STRINGS only. It never opens a
 * file, never touches the filesystem, and makes no OS
 * calls, so every result is lexical and deterministic:
 * "." and ".." are resolved by the rules below, and
 * symlinks are NOT resolved (that would need the disk).
 *
 *   char *j = pa_normalize("C:\\a\\..\\b", NULL);
 *   // j == {"normalized":"C:/b", ...}
 *   pa_free_string(j);
 *
 * ------------------------------------------------------------------
 * PATH KINDS ("kind" in the JSON results)
 *
 *   drive_absolute  C:\a\b          rooted at a drive
 *   drive_relative  C:a\b           relative to drive C's
 *                                   current directory
 *   unc             \\srv\share\a   rooted at a share
 *   device          \\?\C:\a, \\?\UNC\srv\share\a,
 *                   \\.\a           device-namespace paths
 *   posix_absolute  /a\b or \a\b    rooted, no drive
 *                                   (a single leading
 *                                   separator)
 *   relative        a\b, ..\a, .
 *
 * Both "\" and "/" are separators on input, everywhere.
 *
 * NORMALIZATION RULES (pa_normalize, and the results of
 * pa_join, which are normalized results):
 *
 *   - Separators are unified: "normalized" uses "/" by
 *     default, "windows_form" always uses "\", and
 *     "extended_form" always uses "\".
 *   - Duplicate separators collapse, except that a UNC or
 *     device path keeps exactly its two leading ones.
 *   - "." segments are dropped; ".." removes the previous
 *     segment lexically. A ".." at the root of an absolute
 *     path would escape the root: it is REMOVED (the path
 *     is clamped to its root) and reported as an issue
 *     with code "escapes_root". Leading ".." segments of
 *     a relative (or drive-relative) path are kept — what
 *     they climb into is the caller's context, not known
 *     here.
 *   - A trailing separator is stripped, except at a root
 *     ("C:\" stays "C:\", "\\srv\share\" stays as it is).
 *   - An empty relative result (from "." or "a\..") is
 *     written "." — the current directory.
 *   - "components" are the names AFTER the root: for
 *     C:\a\b they are ["a","b"]; for \\srv\share\a the
 *     server and share belong to the root and components
 *     are ["a"]. "file_name" is the last component (""
 *     at a root), "extension" is the part after its last
 *     dot ("" for dotfiles like .gitignore and for a
 *     trailing dot), "parent" is the normalized form of
 *     the path with the last component removed ("" when
 *     there is no parent, the root form at a root).
 *   - "utf16_len" is the length of "normalized" counted
 *     in UTF-16 code units (the way Windows counts): a
 *     character above U+FFFF, such as most emoji, counts
 *     as 2. "byte_len" is its length in UTF-8 bytes.
 *   - "exceeds_legacy_max_path" is true when the WINDOWS
 *     form is longer than 260 UTF-16 code units (the
 *     legacy MAX_PATH limit). "needs_extended_prefix" is
 *     true when it exceeds that limit AND the path is a
 *     drive-absolute or UNC path that does not already
 *     have an extended/device form.
 *   - "extended_form" is the path with the \\?\ prefix
 *     (drive paths) or the \\?\UNC\ prefix (UNC paths).
 *     Paths that cannot take the prefix (relative,
 *     drive-relative, posix_absolute, \\.\ device paths)
 *     get their plain windows_form as extended_form.
 *   - "issues" holds {code, message} objects:
 *     "escapes_root" (above) and "combining_sequence"
 *     (below). Full validation is pa_validate's job.
 *
 * OPTIONS (pa_normalize's second argument; NULL or an
 * empty string means defaults). One JSON object:
 *
 *   {"separator": "\\"}
 *
 *   separator  "/" (default), "\\", "forward" or
 *              "backslash": the separator used in the
 *              "normalized" field only. windows_form and
 *              extended_form always use "\".
 *
 * Unknown keys are an error (PA_ERR_PARSE), duplicate
 * keys are a parse error too, and a wrong-typed value is
 * PA_ERR_INVALID_ARG.
 *
 * ------------------------------------------------------------------
 * VALIDATION (pa_validate) returns one JSON object:
 *
 *   {"path":"...","valid":false,"issues":[
 *     {"code":"reserved_name","component":"CON.txt",
 *      "message":"..."}]}
 *
 * "valid" is true exactly when "issues" is empty. Issue
 * codes, each with a plain-language message saying what
 * is wrong and what to do about it:
 *
 *   empty               the path is the empty string
 *   control_char        a character below U+0020, or
 *                       U+007F, appears in the path
 *   illegal_char        one of < > : " | ? * appears in a
 *                       component; a colon is legal ONLY
 *                       as the drive separator (C:), so
 *                       "C:\a:b" is flagged and "C:\ab"
 *                       is not
 *   reserved_name       a component whose part before
 *                       the first dot is CON, PRN, AUX,
 *                       NUL, COM1-COM9 or LPT1-LPT9
 *                       (any letter case, so CON.txt and
 *                       lpt9.doc are both flagged)
 *   trailing_dot        a component ends with "."
 *   trailing_space      a component ends with a space
 *   component_too_long  a component is longer than 255
 *                       UTF-16 code units
 *   empty_component     an empty name between two
 *                       separators in the middle of the
 *                       path (a trailing separator is NOT
 *                       an issue)
 *   combining_sequence  informational only: a component
 *                       contains combining characters
 *                       (see UNICODE)
 *
 * ------------------------------------------------------------------
 * JOIN (pa_join) — Windows rules, result is the same JSON
 * as pa_normalize, with "input" set to the combined,
 * not-yet-normalized string:
 *
 *   - An empty part returns the normalized base; an empty
 *     base returns the normalized part.
 *   - A drive-absolute, UNC or device part REPLACES the
 *     base completely.
 *   - A posix_absolute part (\x\y) keeps the base's root:
 *     join("C:\a\b", "\x") is "C:\x"; on a UNC base the
 *     share is kept.
 *   - A drive-relative part (C:foo) on a base with the
 *     SAME drive letter joins onto the base (the base
 *     stands in for that drive's current directory). On a
 *     different drive it stands alone: that drive's
 *     current directory cannot be known here, and the
 *     header will not pretend otherwise.
 *   - Any other part is appended to the base and the
 *     result normalized (so ".." in the part can climb
 *     out of the base's last components).
 *
 * RELATIVE (pa_relative) returns:
 *
 *   {"from_dir":"...","to":"...","relative":"../../d",
 *    "windows_form":"..\\..\\d","components":["..","..","d"],
 *    "is_same":false}
 *
 * Both paths must be absolute and share a root: the same
 * drive letter (letter case does not matter; \\?\C: and
 * C: are the same root), or the same UNC server+share
 * (case does not matter). Component matching is also
 * case-insensitive, as Windows treats names. Identical
 * paths give relative "." and is_same true. When the
 * roots differ the call FAILS (NULL return) with
 * PA_ERR_DIFFERENT_ROOT and a plain-language message;
 * when either path is not absolute it fails with
 * PA_ERR_INVALID_ARG.
 *
 * SPLIT (pa_split) returns:
 *
 *   {"path":"...","kind":"unc","is_absolute":true,
 *    "root":"//srv/share/","components":["a","b"]}
 *
 * "root" is in normalized ("/") form: "C:/" for a drive
 * root, "C:" for a drive-relative path, "//srv/share/"
 * for UNC, "/" for posix_absolute, "" for relative.
 * Components are the dot-resolved names after the root.
 *
 * EXTENDED CONVERSION. pa_to_extended(path) returns the
 * extended_form of the path (see above) as a plain
 * string; an empty path fails with PA_ERR_INVALID_ARG.
 * pa_from_extended(path) removes the prefix: \\?\C:\a
 * becomes C:\a and \\?\UNC\s\sh\a becomes \\s\sh\a; any
 * other path is returned in its plain windows_form. For
 * every path p, from_extended(to_extended(p)) equals the
 * windows_form of p.
 *
 * ------------------------------------------------------------------
 * UNICODE. All strings are UTF-8, NUL-terminated; by
 * that contract input is always valid UTF-8 and lone
 * surrogates cannot occur. Lengths are reported in
 * UTF-16 code units where Windows counts that way.
 * PathKit does NOT perform Unicode normalization: names
 * are passed through unchanged. As a heuristic, a
 * component containing characters in the common
 * combining-mark ranges is flagged with the
 * informational "combining_sequence" issue — it means
 * "this name is stored with separate accent characters,
 * which some programs treat as a different name from the
 * same-looking single-character form", nothing more.
 *
 * ERRORS. Every function that returns char* returns NULL
 * on failure and sets the thread-local last error: read
 * it with pa_last_error() (message) and
 * pa_last_error_code(). char* results are caller-owned:
 * free with pa_free_string(). pa_normalize never fails
 * for a non-null path — path problems are "issues" in
 * its JSON — it fails only for a null/non-UTF-8 argument
 * or bad options JSON.
 */
#ifndef PATHKIT_H
#define PATHKIT_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32) && defined(PA_BUILD)
#define PA_API __declspec(dllexport)
#elif defined(_WIN32)
#define PA_API __declspec(dllimport)
#else
#define PA_API
#endif

enum {
    PA_OK = 0,
    PA_ERR_INVALID_ARG = 1,    /* null or empty argument, wrong value type, non-absolute path where one is required */
    PA_ERR_NOT_SUPPORTED = 2,
    PA_ERR_NOT_FOUND = 3,
    PA_ERR_ACCESS_DENIED = 4,
    PA_ERR_PLATFORM = 5,
    PA_ERR_INTERNAL = 6,
    PA_ERR_PARSE = 7,          /* options JSON is malformed, has an unknown key, or repeats a key */
    PA_ERR_DIFFERENT_ROOT = 8  /* pa_relative: different drive letters or UNC shares — no relative path exists */
};

PA_API const char *pa_version(void);
PA_API const char *pa_last_error(void);
PA_API int pa_last_error_code(void);
PA_API void pa_free_string(char *p);

/* Normalize a path. options_json may be NULL (defaults).
 * Returns the JSON object documented above (caller-owned;
 * free with pa_free_string()), or NULL on failure. */
PA_API char *pa_normalize(const char *path, const char *options_json);

/* Validate a path. Returns
 * {"path","valid","issues":[{code,component,message}]}
 * (caller-owned), or NULL on failure. An empty path is a
 * successful call whose JSON carries the "empty" issue. */
PA_API char *pa_validate(const char *path);

/* Join part onto base by the Windows rules above.
 * Returns the same JSON shape as pa_normalize
 * (caller-owned), or NULL on failure. */
PA_API char *pa_join(const char *base, const char *part);

/* Relative path from from_dir to to. Returns the JSON
 * documented above (caller-owned), or NULL with
 * PA_ERR_DIFFERENT_ROOT / PA_ERR_INVALID_ARG. */
PA_API char *pa_relative(const char *from_dir, const char *to);

/* Extended-length form of a path as a plain string
 * (caller-owned), or NULL on failure. */
PA_API char *pa_to_extended(const char *path);

/* Remove an extended-length prefix (caller-owned result),
 * or NULL on failure. */
PA_API char *pa_from_extended(const char *path);

/* Split a path into root + components. Returns the JSON
 * documented above (caller-owned), or NULL on failure
 * (an empty path is PA_ERR_INVALID_ARG). */
PA_API char *pa_split(const char *path);

#ifdef __cplusplus
}
#endif
#endif /* PATHKIT_H */
