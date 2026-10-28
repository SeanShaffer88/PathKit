//! PathKit — a long-path and Unicode compatibility layer
//! behind a small C ABI.
//!
//! PathKit parses, normalizes, validates and converts
//! Windows-style path *strings*. It never opens a file:
//! every operation is lexical, deterministic and testable
//! on any platform. It exists so each application does not
//! re-implement (differently) the same edge cases:
//!
//! - `/` and `\` are both separators on input; output has
//!   one consistent separator per form.
//! - `.` and `..` are resolved lexically (no filesystem
//!   access; symlinks are not, and cannot be, resolved).
//! - A `..` that would climb above the root of an
//!   absolute path is clamped and reported as an
//!   `escapes_root` issue — it is never silently kept.
//! - Long paths: the result reports UTF-16 and UTF-8
//!   lengths, whether the Windows form passes the legacy
//!   260-character `MAX_PATH` limit, and the extended
//!   (`\\?\`) form that bypasses that limit.
//! - Validation speaks in plain language: reserved device
//!   names, characters Windows forbids, trailing dots and
//!   spaces, over-long components and empty components
//!   each get a named issue and a sentence a user can act
//!   on.
//! - Unicode: lengths count UTF-16 code units, so a
//!   character outside the Basic Multilingual Plane (most
//!   emoji) counts as 2, exactly as Windows counts it.
//!   Combining-character sequences are flagged with the
//!   informational `combining_sequence` code. That is a
//!   *heuristic*: full Unicode normalization (NFC/NFD)
//!   needs normalization tables this zero-dependency
//!   library deliberately does not carry, and PathKit
//!   does not claim to perform it. Input is valid UTF-8
//!   by the ABI contract, so lone surrogates cannot occur
//!   in a string that reaches this code.

pub mod ffi;
pub mod json;
pub mod platform;

pub const VERSION: &str = "0.1.0";
pub const VERSION_MAJOR: u32 = 0;
pub const VERSION_MINOR: u32 = 1;
pub const VERSION_PATCH: u32 = 0;

// Error codes (mirror include/pathkit.h).
pub const PA_OK: i32 = 0;
pub const ERR_INVALID_ARG: i32 = 1;
pub const ERR_NOT_SUPPORTED: i32 = 2;
pub const ERR_NOT_FOUND: i32 = 3;
pub const ERR_ACCESS_DENIED: i32 = 4;
pub const ERR_PLATFORM: i32 = 5;
pub const ERR_INTERNAL: i32 = 6;
pub const ERR_PARSE: i32 = 7;
/// `pa_relative`: the two paths have different roots
/// (different drive letters or different UNC shares), so
/// no relative path between them exists.
pub const ERR_DIFFERENT_ROOT: i32 = 8;

pub type PaResult<T> = Result<T, (i32, String)>;

/// The legacy `MAX_PATH` limit, in UTF-16 code units.
pub const LEGACY_MAX_PATH: usize = 260;
/// The per-component limit, in UTF-16 code units.
pub const MAX_COMPONENT_UTF16: usize = 255;

/// Escape a string for inclusion inside JSON double quotes.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", json_escape(s))
}

fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

// ---------------------------------------------------------------------------
// Path model
// ---------------------------------------------------------------------------

/// The shape of a path's root. The string values are the
/// `kind` field of the normalize/split JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    DriveAbsolute,
    DriveRelative,
    Unc,
    Device,
    PosixAbsolute,
    Relative,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::DriveAbsolute => "drive_absolute",
            Kind::DriveRelative => "drive_relative",
            Kind::Unc => "unc",
            Kind::Device => "device",
            Kind::PosixAbsolute => "posix_absolute",
            Kind::Relative => "relative",
        }
    }
    /// Absolute kinds have a root a `..` cannot climb
    /// above. Drive-relative paths are not absolute: their
    /// `..` segments are kept, because the drive's current
    /// directory (which they would climb into) is unknown.
    pub fn is_absolute(&self) -> bool {
        matches!(
            self,
            Kind::DriveAbsolute | Kind::Unc | Kind::Device | Kind::PosixAbsolute
        )
    }
}

/// One problem found in a path, in plain language.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub code: String,
    /// The component the issue is about, when there is one.
    pub component: Option<String>,
    pub message: String,
}

impl Issue {
    fn new(code: &str, component: Option<&str>, message: String) -> Issue {
        Issue {
            code: code.to_string(),
            component: component.map(|s| s.to_string()),
            message,
        }
    }
    fn to_json(&self, with_component: bool) -> String {
        if with_component {
            let comp = match &self.component {
                Some(c) => json_str(c),
                None => "null".to_string(),
            };
            format!(
                "{{\"code\":{},\"component\":{},\"message\":{}}}",
                json_str(&self.code),
                comp,
                json_str(&self.message)
            )
        } else {
            format!(
                "{{\"code\":{},\"message\":{}}}",
                json_str(&self.code),
                json_str(&self.message)
            )
        }
    }
}

/// A parsed path: its kind, the root pieces needed to
/// rebuild it, and the raw segments after the root.
#[derive(Clone, Debug)]
struct Parsed {
    kind: Kind,
    /// Device sub-form: "drive", "unc" or "other"
    /// (`\\.\...`). Empty for non-device kinds.
    device_form: &'static str,
    drive: Option<char>,
    /// UNC (and device-UNC) server and share.
    server: Option<String>,
    share: Option<String>,
    /// Segments after the root, separators removed, NOT
    /// yet dot-resolved (validation needs the raw form).
    raw_segments: Vec<String>,
}

fn is_sep(c: char) -> bool {
    c == '\\' || c == '/'
}

fn leading_seps(s: &str) -> usize {
    s.chars().take_while(|&c| is_sep(c)).count()
}

/// Split a remainder string on either separator.
fn split_segs(s: &str) -> Vec<String> {
    if s.is_empty() {
        return Vec::new();
    }
    s.split(is_sep).map(|p| p.to_string()).collect()
}

fn drive_at(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if first.is_ascii_alphabetic() && chars.next() == Some(':') {
        Some(first)
    } else {
        None
    }
}

/// Parse `input` into root + raw segments. Pure string
/// work; never fails (odd strings parse as relative).
fn parse(input: &str) -> Parsed {
    let bytes = input.as_bytes();
    let seps = leading_seps(input);
    // Device forms: \\?\..., \\.\..., and the same with
    // forward slashes. Byte checks are safe: every byte
    // tested here is ASCII, so slice points are too.
    if seps >= 2
        && bytes.len() >= 4
        && (bytes[2] == b'?' || bytes[2] == b'.')
        && is_sep(bytes[3] as char)
    {
        let marker = bytes[2];
        let rest = &input[4..];
        if marker == b'.' {
            return Parsed {
                kind: Kind::Device,
                device_form: "other",
                drive: None,
                server: None,
                share: None,
                raw_segments: split_segs(rest),
            };
        }
        // \\?\UNC\server\share\...
        if rest.len() >= 4
            && rest[..3].eq_ignore_ascii_case("UNC")
            && is_sep(rest.as_bytes()[3] as char)
        {
            let after = &rest[4..];
            let mut segs = split_segs(after);
            let server = if segs.is_empty() {
                None
            } else {
                Some(segs.remove(0))
            };
            let share = if segs.is_empty() {
                None
            } else {
                Some(segs.remove(0))
            };
            return Parsed {
                kind: Kind::Device,
                device_form: "unc",
                drive: None,
                server,
                share,
                raw_segments: segs,
            };
        }
        if let Some(d) = drive_at(rest) {
            let after_drive = &rest[2..];
            let trimmed = after_drive.trim_start_matches(is_sep);
            return Parsed {
                kind: Kind::Device,
                device_form: "drive",
                drive: Some(d),
                server: None,
                share: None,
                raw_segments: split_segs(trimmed),
            };
        }
        return Parsed {
            kind: Kind::Device,
            device_form: "other",
            drive: None,
            server: None,
            share: None,
            raw_segments: split_segs(rest),
        };
    }
    if seps >= 2 {
        // UNC: \\server\share\...
        let after = input.trim_start_matches(is_sep);
        let mut segs = split_segs(after);
        let server = if segs.is_empty() || (segs.len() == 1 && segs[0].is_empty()) {
            None
        } else {
            Some(segs.remove(0))
        };
        let share = if segs.is_empty() {
            None
        } else {
            Some(segs.remove(0))
        };
        if server.is_none() {
            segs.clear();
        }
        return Parsed {
            kind: Kind::Unc,
            device_form: "",
            drive: None,
            server,
            share,
            raw_segments: segs,
        };
    }
    if let Some(d) = drive_at(input) {
        let rest = &input[2..];
        if rest.starts_with(is_sep) {
            return Parsed {
                kind: Kind::DriveAbsolute,
                device_form: "",
                drive: Some(d),
                server: None,
                share: None,
                raw_segments: split_segs(rest.trim_start_matches(is_sep)),
            };
        }
        return Parsed {
            kind: Kind::DriveRelative,
            device_form: "",
            drive: Some(d),
            server: None,
            share: None,
            raw_segments: split_segs(rest),
        };
    }
    if seps == 1 {
        return Parsed {
            kind: Kind::PosixAbsolute,
            device_form: "",
            drive: None,
            server: None,
            share: None,
            raw_segments: split_segs(input.trim_start_matches(is_sep)),
        };
    }
    Parsed {
        kind: Kind::Relative,
        device_form: "",
        drive: None,
        server: None,
        share: None,
        raw_segments: split_segs(input),
    }
}

/// Resolve `.`/`..` and empty segments lexically.
fn resolve(parsed: &Parsed) -> (Vec<String>, Vec<Issue>) {
    let mut out: Vec<String> = Vec::new();
    let mut issues = Vec::new();
    let absolute = parsed.kind.is_absolute();
    for seg in &parsed.raw_segments {
        match seg.as_str() {
            "" | "." => {}
            ".." => {
                if matches!(out.last().map(|s| s.as_str()), Some(s) if s != "..") {
                    out.pop();
                } else if absolute {
                    issues.push(Issue::new(
                        "escapes_root",
                        Some(".."),
                        "A \"..\" here would go above the top of the path, so it was removed. \
                         The path stays at its root."
                            .to_string(),
                    ));
                } else {
                    out.push("..".to_string());
                }
            }
            s => out.push(s.to_string()),
        }
    }
    (out, issues)
}

/// Build one form of the path from resolved components.
/// The extended form always uses backslashes (the form
/// Windows requires); other forms use `sep`.
fn build(parsed: &Parsed, comps: &[String], sep: char, extended: bool) -> String {
    let s = if extended { '\\' } else { sep };
    // Join components onto a root. `bare_root` roots keep
    // no trailing separator when empty (only "C:" of a
    // drive-relative path — handled by its own arm below,
    // so every root passed here is non-bare).
    let join = |root: String, comps: &[String]| -> String {
        if comps.is_empty() {
            if root.is_empty() {
                ".".to_string()
            } else if root.ends_with(s) {
                root
            } else {
                format!("{root}{s}")
            }
        } else if root.is_empty() {
            comps.join(&s.to_string())
        } else if root.ends_with(s) {
            format!("{root}{}", comps.join(&s.to_string()))
        } else {
            format!("{root}{s}{}", comps.join(&s.to_string()))
        }
    };
    match parsed.kind {
        Kind::Device => match parsed.device_form {
            "drive" => {
                let d = parsed.drive.unwrap_or('C');
                if extended {
                    join(format!("\\\\?\\{d}:"), comps)
                } else {
                    join(format!("{s}{s}?{s}{d}:"), comps)
                }
            }
            "unc" => {
                let srv = parsed.server.clone().unwrap_or_default();
                let shr = parsed.share.clone().unwrap_or_default();
                if extended {
                    join(format!("\\\\?\\UNC\\{srv}\\{shr}"), comps)
                } else {
                    join(format!("{s}{s}?{s}UNC{s}{srv}{s}{shr}"), comps)
                }
            }
            _ => join(format!("{s}{s}.{s}"), comps),
        },
        Kind::Unc => {
            let srv = parsed.server.clone().unwrap_or_default();
            let shr = parsed.share.clone().unwrap_or_default();
            if srv.is_empty() {
                return format!("{s}{s}");
            }
            if shr.is_empty() {
                return join(format!("{s}{s}{srv}"), comps);
            }
            if extended {
                join(format!("\\\\?\\UNC\\{srv}\\{shr}"), comps)
            } else {
                join(format!("{s}{s}{srv}{s}{shr}"), comps)
            }
        }
        Kind::DriveAbsolute => {
            let d = parsed.drive.unwrap_or('C');
            if extended {
                join(format!("\\\\?\\{d}:"), comps)
            } else {
                join(format!("{d}:"), comps)
            }
        }
        Kind::DriveRelative => {
            let d = parsed.drive.unwrap_or('C');
            // No separator between "C:" and the first
            // component — that is what makes it relative.
            if comps.is_empty() {
                format!("{d}:")
            } else {
                format!("{d}:{}", comps.join(&s.to_string()))
            }
        }
        Kind::PosixAbsolute => {
            if comps.is_empty() {
                format!("{s}")
            } else {
                format!("{s}{}", comps.join(&s.to_string()))
            }
        }
        Kind::Relative => {
            if comps.is_empty() {
                ".".to_string()
            } else {
                comps.join(&s.to_string())
            }
        }
    }
}

/// The root of a parsed path, in normalized (`/`) form:
/// `"C:/"`, `"//server/share/"`, `"C:"`, `"/"`, or `""`.
fn root_normalized(parsed: &Parsed) -> String {
    match parsed.kind {
        Kind::Relative => String::new(),
        Kind::DriveRelative => format!("{}:", parsed.drive.unwrap_or('C')),
        _ => build(parsed, &[], '/', false),
    }
}

fn file_name_of(comps: &[String]) -> String {
    comps.last().cloned().unwrap_or_default()
}

fn extension_of(file_name: &str) -> String {
    match file_name.rfind('.') {
        Some(i) if i > 0 && i + 1 < file_name.len() => file_name[i + 1..].to_string(),
        _ => String::new(),
    }
}

/// Does a component contain a combining character?
/// Heuristic ranges for combining marks; see the crate
/// docs for why this is not full NFC detection.
fn has_combining(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c as u32,
            0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
    })
}

fn combining_issue(component: &str) -> Issue {
    Issue::new(
        "combining_sequence",
        Some(component),
        format!(
            "The name \"{component}\" contains combining characters (accents stored as separate \
             characters added to a letter). Some programs treat that as a different name from the \
             same-looking name stored as one character. This is information only, not an error."
        ),
    )
}

// ---------------------------------------------------------------------------
// Normalize
// ---------------------------------------------------------------------------

/// Options for `normalize` (the `pa_normalize` JSON).
#[derive(Clone, Debug)]
pub struct NormalizeOptions {
    /// Separator used in the `normalized` field. The
    /// `windows_form` always uses `\` and `extended_form`
    /// always uses `\`. Default: `/`.
    pub separator: char,
}

impl Default for NormalizeOptions {
    fn default() -> Self {
        NormalizeOptions { separator: '/' }
    }
}

/// Parse the options JSON: `null`/absent means defaults.
/// Known keys: `separator` — `"/"`, `"\\"`, `"forward"` or
/// `"backslash"`. Unknown keys and duplicate keys are
/// `ERR_PARSE`; a wrong-typed value is `ERR_INVALID_ARG`.
pub fn parse_options(text: Option<&str>) -> PaResult<NormalizeOptions> {
    let mut opts = NormalizeOptions::default();
    let text = match text {
        None => return Ok(opts),
        Some(t) if t.trim().is_empty() => return Ok(opts),
        Some(t) => t,
    };
    let value =
        json::parse(text).map_err(|e| (ERR_PARSE, format!("options are not valid JSON: {e}")))?;
    let obj = value
        .as_object()
        .ok_or((ERR_PARSE, "options must be a JSON object".to_string()))?;
    for (key, _) in obj {
        if key != "separator" {
            return Err((ERR_PARSE, format!("unknown options key \"{key}\"")));
        }
    }
    if let Some(v) = value.get("separator") {
        match v.as_str() {
            Some("/") | Some("forward") => opts.separator = '/',
            Some("\\") | Some("backslash") => opts.separator = '\\',
            Some(other) => {
                return Err((
                    ERR_INVALID_ARG,
                    format!("\"separator\" must be \"/\" or \"\\\\\", got \"{other}\""),
                ))
            }
            None => {
                return Err((
                    ERR_INVALID_ARG,
                    format!("\"separator\" must be a string, got {}", v.type_name()),
                ))
            }
        }
    }
    Ok(opts)
}

/// A normalized path and everything the header promises
/// about it.
#[derive(Clone, Debug)]
pub struct Normalized {
    pub input: String,
    pub normalized: String,
    pub windows_form: String,
    pub extended_form: String,
    pub is_absolute: bool,
    pub kind: Kind,
    pub components: Vec<String>,
    pub file_name: String,
    pub extension: String,
    pub parent: String,
    pub utf16_len: usize,
    pub byte_len: usize,
    pub exceeds_legacy_max_path: bool,
    pub needs_extended_prefix: bool,
    pub issues: Vec<Issue>,
}

/// Normalize `input`. This never fails for a string;
/// problems are reported in `issues`.
pub fn normalize(input: &str, opts: &NormalizeOptions) -> Normalized {
    if input.is_empty() {
        return Normalized {
            input: String::new(),
            normalized: String::new(),
            windows_form: String::new(),
            extended_form: String::new(),
            is_absolute: false,
            kind: Kind::Relative,
            components: Vec::new(),
            file_name: String::new(),
            extension: String::new(),
            parent: String::new(),
            utf16_len: 0,
            byte_len: 0,
            exceeds_legacy_max_path: false,
            needs_extended_prefix: false,
            issues: vec![Issue::new(
                "empty",
                None,
                "The path is empty. There is no file or folder to point at.".to_string(),
            )],
        };
    }
    let parsed = parse(input);
    let (comps, mut issues) = resolve(&parsed);
    for comp in &comps {
        if has_combining(comp) {
            issues.push(combining_issue(comp));
        }
    }
    let normalized = build(&parsed, &comps, opts.separator, false);
    let windows_form = build(&parsed, &comps, '\\', false);
    let extended_form = build(&parsed, &comps, '\\', true);
    let file_name = file_name_of(&comps);
    let extension = extension_of(&file_name);
    let parent = if comps.is_empty() {
        String::new()
    } else if comps.len() == 1 {
        match parsed.kind {
            Kind::Relative | Kind::DriveRelative => String::new(),
            _ => build(&parsed, &[], opts.separator, false),
        }
    } else {
        build(&parsed, &comps[..comps.len() - 1], opts.separator, false)
    };
    let win_utf16 = utf16_len(&windows_form);
    let exceeds = win_utf16 > LEGACY_MAX_PATH;
    let needs_extended = exceeds
        && matches!(parsed.kind, Kind::DriveAbsolute | Kind::Unc)
        && extended_form != windows_form;
    let utf16_len = utf16_len(&normalized);
    let byte_len = normalized.len();
    Normalized {
        input: input.to_string(),
        normalized,
        windows_form,
        extended_form,
        is_absolute: parsed.kind.is_absolute(),
        kind: parsed.kind,
        components: comps,
        file_name,
        extension,
        parent,
        utf16_len,
        byte_len,
        exceeds_legacy_max_path: exceeds,
        needs_extended_prefix: needs_extended,
        issues,
    }
}

fn components_json(comps: &[String]) -> String {
    let parts: Vec<String> = comps.iter().map(|c| json_str(c)).collect();
    format!("[{}]", parts.join(","))
}

impl Normalized {
    pub fn to_json(&self) -> String {
        let issues: Vec<String> = self.issues.iter().map(|i| i.to_json(false)).collect();
        format!(
            "{{\"input\":{},\"normalized\":{},\"windows_form\":{},\"extended_form\":{},\"is_absolute\":{},\"kind\":{},\"components\":{},\"file_name\":{},\"extension\":{},\"parent\":{},\"utf16_len\":{},\"byte_len\":{},\"exceeds_legacy_max_path\":{},\"needs_extended_prefix\":{},\"issues\":[{}]}}",
            json_str(&self.input),
            json_str(&self.normalized),
            json_str(&self.windows_form),
            json_str(&self.extended_form),
            self.is_absolute,
            json_str(self.kind.as_str()),
            components_json(&self.components),
            json_str(&self.file_name),
            json_str(&self.extension),
            json_str(&self.parent),
            self.utf16_len,
            self.byte_len,
            self.exceeds_legacy_max_path,
            self.needs_extended_prefix,
            issues.join(",")
        )
    }
}

// ---------------------------------------------------------------------------
// Validate
// ---------------------------------------------------------------------------

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Validate `input`, returning every issue found, in
/// path order. An empty list means the path is clean.
pub fn validate(input: &str) -> Vec<Issue> {
    let mut issues = Vec::new();
    if input.is_empty() {
        issues.push(Issue::new(
            "empty",
            None,
            "The path is empty. There is no file or folder to point at.".to_string(),
        ));
        return issues;
    }
    for c in input.chars() {
        if (c as u32) < 0x20 || c as u32 == 0x7F {
            issues.push(Issue::new(
                "control_char",
                None,
                format!(
                    "The path contains a hidden control character (U+{:04X}). Windows cannot \
                     use it in a name; it usually means the text was copied from somewhere odd. \
                     Type the path again by hand.",
                    c as u32
                ),
            ));
            break;
        }
    }
    let parsed = parse(input);
    // Names to check: the UNC server/share (they are names
    // too), then every raw segment after the root.
    let mut names: Vec<String> = Vec::new();
    if let Some(s) = &parsed.server {
        if !s.is_empty() {
            names.push(s.clone());
        }
    }
    if let Some(s) = &parsed.share {
        if !s.is_empty() {
            names.push(s.clone());
        }
    }
    // Empty-component detection uses the raw segments:
    // trailing empties come from a trailing separator and
    // are harmless; an interior empty one is an issue.
    let raw = &parsed.raw_segments;
    let mut last_non_empty = raw.len();
    while last_non_empty > 0 && raw[last_non_empty - 1].is_empty() {
        last_non_empty -= 1;
    }
    for (i, seg) in raw.iter().enumerate() {
        if seg.is_empty() && i < last_non_empty {
            issues.push(Issue::new(
                "empty_component",
                Some(""),
                "The path has an empty name between two separators (a double slash in the \
                 middle of the path). Remove the extra separator."
                    .to_string(),
            ));
        }
    }
    names.extend(raw.iter().cloned());
    for comp in &names {
        if comp.is_empty() || comp == "." || comp == ".." {
            continue;
        }
        for c in comp.chars() {
            if matches!(c, '<' | '>' | '"' | '|' | '?' | '*') {
                issues.push(Issue::new(
                    "illegal_char",
                    Some(comp),
                    format!(
                        "The name \"{comp}\" contains '{c}', which Windows does not allow in \
                         file or folder names. Remove or replace that character."
                    ),
                ));
            }
            if c == ':' {
                issues.push(Issue::new(
                    "illegal_char",
                    Some(comp),
                    format!(
                        "The name \"{comp}\" contains a colon ':'. A colon is only allowed \
                         right after a drive letter, like \"C:\". Remove it from this name."
                    ),
                ));
            }
        }
        let base = comp.split('.').next().unwrap_or("");
        if RESERVED.contains(&base.to_uppercase().as_str()) {
            issues.push(Issue::new(
                "reserved_name",
                Some(comp),
                format!(
                    "The name \"{comp}\" starts with \"{base}\", which Windows reserves for a \
                     device (an old name for the screen, a printer or a port). Windows will \
                     refuse to create a file with this name. Pick a different name, such as \
                     \"{base}2\"."
                ),
            ));
        }
        if comp.ends_with('.') {
            issues.push(Issue::new(
                "trailing_dot",
                Some(comp),
                format!(
                    "The name \"{comp}\" ends with a dot. Windows removes a trailing dot, so \
                     the name you get would not be the name you asked for. Remove the dot."
                ),
            ));
        }
        if comp.ends_with(' ') {
            issues.push(Issue::new(
                "trailing_space",
                Some(comp),
                format!(
                    "The name \"{comp}\" ends with a space. Windows removes a trailing space, \
                     so the name you get would not be the name you asked for. Remove the space."
                ),
            ));
        }
        let len16 = utf16_len(comp);
        if len16 > MAX_COMPONENT_UTF16 {
            issues.push(Issue::new(
                "component_too_long",
                Some(comp),
                format!(
                    "The name \"{comp}\" is {len16} characters long (counting the way Windows \
                     counts). One name may be at most 255 characters. Shorten it."
                ),
            ));
        }
        if has_combining(comp) {
            issues.push(combining_issue(comp));
        }
    }
    issues
}

/// The `pa_validate` JSON shape.
pub fn validate_json(input: &str) -> String {
    let issues = validate(input);
    let parts: Vec<String> = issues.iter().map(|i| i.to_json(true)).collect();
    format!(
        "{{\"path\":{},\"valid\":{},\"issues\":[{}]}}",
        json_str(input),
        issues.is_empty(),
        parts.join(",")
    )
}

// ---------------------------------------------------------------------------
// Split, join, relative, extended conversion
// ---------------------------------------------------------------------------

/// The `pa_split` JSON shape.
pub fn split_json(input: &str) -> PaResult<String> {
    if input.is_empty() {
        return Err((ERR_INVALID_ARG, "path must not be empty".to_string()));
    }
    let parsed = parse(input);
    let (comps, _) = resolve(&parsed);
    Ok(format!(
        "{{\"path\":{},\"kind\":{},\"is_absolute\":{},\"root\":{},\"components\":{}}}",
        json_str(input),
        json_str(parsed.kind.as_str()),
        parsed.kind.is_absolute(),
        json_str(&root_normalized(&parsed)),
        components_json(&comps)
    ))
}

/// Join `part` onto `base` following Windows rules (see
/// the header) and return the normalized result.
pub fn join(base: &str, part: &str) -> PaResult<Normalized> {
    let opts = NormalizeOptions::default();
    if part.is_empty() {
        return Ok(normalize(base, &opts));
    }
    if base.is_empty() {
        return Ok(normalize(part, &opts));
    }
    let bp = parse(base);
    let pp = parse(part);
    let combined: String = match pp.kind {
        // A fully rooted part replaces the base.
        Kind::DriveAbsolute | Kind::Unc | Kind::Device => part.to_string(),
        Kind::PosixAbsolute => match bp.kind {
            Kind::DriveAbsolute | Kind::DriveRelative => {
                format!("{}:{}", bp.drive.unwrap_or('C'), part)
            }
            Kind::Unc | Kind::Device => {
                let root = root_normalized(&bp);
                format!("{root}{}", part.trim_start_matches(is_sep))
            }
            _ => part.to_string(),
        },
        Kind::DriveRelative => {
            let same_drive = matches!(bp.kind, Kind::DriveAbsolute | Kind::DriveRelative)
                && bp.drive.map(|d| d.to_ascii_uppercase())
                    == pp.drive.map(|d| d.to_ascii_uppercase());
            if same_drive {
                // Same drive: the part is relative to that
                // drive's current directory, which `base`
                // stands in for.
                format!("{base}\\{}", &part[2..])
            } else {
                // A different drive's current directory is
                // unknowable here; the part stands alone.
                part.to_string()
            }
        }
        Kind::Relative => format!("{base}\\{part}"),
    };
    let mut result = normalize(&combined, &opts);
    result.input = combined;
    Ok(result)
}

/// The root key two paths must share for a relative path
/// to exist between them, compared case-insensitively.
/// Device-drive roots key the same as plain drive roots,
/// and device-UNC the same as plain UNC.
fn root_key(p: &Parsed) -> String {
    match p.kind {
        Kind::DriveAbsolute | Kind::DriveRelative => {
            format!("drive:{}", p.drive.unwrap_or('C').to_ascii_uppercase())
        }
        Kind::Device if p.device_form == "drive" => {
            format!("drive:{}", p.drive.unwrap_or('C').to_ascii_uppercase())
        }
        Kind::Unc => format!(
            "unc:{}:{}",
            p.server.clone().unwrap_or_default().to_uppercase(),
            p.share.clone().unwrap_or_default().to_uppercase()
        ),
        Kind::Device if p.device_form == "unc" => format!(
            "unc:{}:{}",
            p.server.clone().unwrap_or_default().to_uppercase(),
            p.share.clone().unwrap_or_default().to_uppercase()
        ),
        Kind::Device => "device:other".to_string(),
        Kind::PosixAbsolute => "posix:".to_string(),
        Kind::Relative => "relative:".to_string(),
    }
}

/// Compute the relative path from `from_dir` to `to`.
/// Returns the `pa_relative` JSON on success.
pub fn relative(from_dir: &str, to: &str) -> PaResult<String> {
    if from_dir.is_empty() || to.is_empty() {
        return Err((ERR_INVALID_ARG, "paths must not be empty".to_string()));
    }
    let fp = parse(from_dir);
    let tp = parse(to);
    if !fp.kind.is_absolute() || !tp.kind.is_absolute() {
        return Err((
            ERR_INVALID_ARG,
            "Both paths must be complete paths that start at a drive, a network share or \
             the top of a disk (like \"C:\\\\work\"), before a relative path between them \
             can be worked out."
                .to_string(),
        ));
    }
    if root_key(&fp) != root_key(&tp) {
        return Err((
            ERR_DIFFERENT_ROOT,
            format!(
                "\"{from_dir}\" and \"{to}\" start from different drives or network shares, \
                 so there is no relative path between them. Use the full path instead."
            ),
        ));
    }
    let (from_comps, _) = resolve(&fp);
    let (to_comps, _) = resolve(&tp);
    let mut common = 0;
    while common < from_comps.len()
        && common < to_comps.len()
        && from_comps[common].eq_ignore_ascii_case(&to_comps[common])
    {
        common += 1;
    }
    let mut rel: Vec<String> = Vec::new();
    for _ in common..from_comps.len() {
        rel.push("..".to_string());
    }
    rel.extend(to_comps[common..].iter().cloned());
    let rel_str = if rel.is_empty() {
        ".".to_string()
    } else {
        rel.join("/")
    };
    let rel_win = rel_str.replace('/', "\\");
    Ok(format!(
        "{{\"from_dir\":{},\"to\":{},\"relative\":{},\"windows_form\":{},\"components\":{},\"is_same\":{}}}",
        json_str(from_dir),
        json_str(to),
        json_str(&rel_str),
        json_str(&rel_win),
        components_json(&rel),
        rel.is_empty()
    ))
}

/// Convert to the extended-length form (`\\?\...`).
/// Paths that cannot take the prefix (relative,
/// drive-relative, root-relative and `\\.\` device
/// paths) are returned in plain Windows form, unchanged
/// in meaning. Already-extended paths pass through.
pub fn to_extended(path: &str) -> PaResult<String> {
    if path.is_empty() {
        return Err((ERR_INVALID_ARG, "path must not be empty".to_string()));
    }
    let n = normalize(path, &NormalizeOptions::default());
    Ok(n.extended_form)
}

/// Remove an extended-length prefix: `\\?\C:\a` becomes
/// `C:\a` and `\\?\UNC\s\sh\a` becomes `\\s\sh\a`. Any
/// other path is returned in plain Windows form.
pub fn from_extended(path: &str) -> PaResult<String> {
    if path.is_empty() {
        return Err((ERR_INVALID_ARG, "path must not be empty".to_string()));
    }
    let parsed = parse(path);
    if parsed.kind == Kind::Device && parsed.device_form != "other" {
        let (comps, _) = resolve(&parsed);
        let plain = Parsed {
            kind: if parsed.device_form == "unc" {
                Kind::Unc
            } else {
                Kind::DriveAbsolute
            },
            device_form: "",
            drive: parsed.drive,
            server: parsed.server.clone(),
            share: parsed.share.clone(),
            raw_segments: parsed.raw_segments.clone(),
        };
        return Ok(build(&plain, &comps, '\\', false));
    }
    let n = normalize(path, &NormalizeOptions::default());
    Ok(n.windows_form)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(p: &str) -> Normalized {
        normalize(p, &NormalizeOptions::default())
    }

    #[test]
    fn basic_drive_path() {
        let n = norm(r"C:\Users\Ann\file.txt");
        assert_eq!(n.normalized, "C:/Users/Ann/file.txt");
        assert_eq!(n.windows_form, r"C:\Users\Ann\file.txt");
        assert_eq!(n.extended_form, r"\\?\C:\Users\Ann\file.txt");
        assert_eq!(n.kind, Kind::DriveAbsolute);
        assert!(n.is_absolute);
        assert_eq!(n.components, vec!["Users", "Ann", "file.txt"]);
        assert_eq!(n.file_name, "file.txt");
        assert_eq!(n.extension, "txt");
        assert_eq!(n.parent, "C:/Users/Ann");
        assert!(!n.exceeds_legacy_max_path);
        assert!(!n.needs_extended_prefix);
        assert!(n.issues.is_empty());
    }

    #[test]
    fn separators_and_dot_segments() {
        let n = norm("C:/a//b/./c/../d/");
        assert_eq!(n.normalized, "C:/a/b/d");
        assert_eq!(n.components, vec!["a", "b", "d"]);
        let opts = parse_options(Some(r#"{"separator":"\\"}"#)).unwrap();
        let n2 = normalize(r"C:\a\b", &opts);
        assert_eq!(n2.normalized, r"C:\a\b");
        assert_eq!(n2.windows_form, r"C:\a\b");
    }

    #[test]
    fn unc_paths() {
        let n = norm(r"\\server\share\dir\file.txt");
        assert_eq!(n.kind, Kind::Unc);
        assert_eq!(n.normalized, "//server/share/dir/file.txt");
        assert_eq!(n.windows_form, r"\\server\share\dir\file.txt");
        assert_eq!(n.extended_form, r"\\?\UNC\server\share\dir\file.txt");
        assert_eq!(n.components, vec!["dir", "file.txt"]);
        assert_eq!(n.parent, "//server/share/dir");
        let root = norm(r"\\server\share\");
        assert_eq!(root.windows_form, r"\\server\share\");
        assert!(root.components.is_empty());
        assert_eq!(root.file_name, "");
    }

    #[test]
    fn device_paths() {
        let n = norm(r"\\?\C:\a\b");
        assert_eq!(n.kind, Kind::Device);
        assert_eq!(n.windows_form, r"\\?\C:\a\b");
        assert_eq!(n.extended_form, r"\\?\C:\a\b");
        assert_eq!(n.components, vec!["a", "b"]);
        let u = norm(r"\\?\UNC\server\share\a");
        assert_eq!(u.kind, Kind::Device);
        assert_eq!(u.windows_form, r"\\?\UNC\server\share\a");
        assert_eq!(u.components, vec!["a"]);
        let dot = norm(r"\\.\C:\a");
        assert_eq!(dot.kind, Kind::Device);
        assert_eq!(dot.windows_form, r"\\.\C:\a");
    }

    #[test]
    fn drive_relative_and_posix_and_relative() {
        let n = norm(r"C:foo\bar");
        assert_eq!(n.kind, Kind::DriveRelative);
        assert!(!n.is_absolute);
        assert_eq!(n.windows_form, r"C:foo\bar");
        assert_eq!(n.extended_form, r"C:foo\bar");
        assert_eq!(n.components, vec!["foo", "bar"]);
        assert_eq!(n.parent, "C:foo");
        let p = norm("/a/b");
        assert_eq!(p.kind, Kind::PosixAbsolute);
        assert_eq!(p.normalized, "/a/b");
        let r = norm(r"a\b\..\c");
        assert_eq!(r.kind, Kind::Relative);
        assert_eq!(r.normalized, "a/c");
        let up = norm(r"..\a");
        assert_eq!(up.normalized, "../a");
        assert_eq!(up.components, vec!["..", "a"]);
    }

    #[test]
    fn escapes_root_is_clamped_and_reported() {
        let n = norm(r"C:\a\..\..\b");
        assert_eq!(n.normalized, "C:/b");
        assert_eq!(n.issues.len(), 1);
        assert_eq!(n.issues[0].code, "escapes_root");
        let u = norm(r"\\s\sh\..\x");
        assert_eq!(u.normalized, "//s/sh/x");
        assert_eq!(u.issues[0].code, "escapes_root");
    }

    #[test]
    fn roots_and_trailing_separators() {
        assert_eq!(norm(r"C:\").normalized, "C:/");
        assert_eq!(norm(r"C:\").file_name, "");
        assert_eq!(norm("/").normalized, "/");
        assert_eq!(norm(".").normalized, ".");
        assert_eq!(norm("a/").normalized, "a");
        assert_eq!(norm("").normalized, "");
        assert_eq!(norm("").issues[0].code, "empty");
    }

    #[test]
    fn long_path_flags() {
        let deep = format!(r"C:\{}", "a".repeat(300));
        let n = norm(&deep);
        assert!(n.exceeds_legacy_max_path);
        assert!(n.needs_extended_prefix);
        assert!(n.extended_form.starts_with(r"\\?\C:\"));
        let rn = norm(&format!(r"dir\{}", "b".repeat(300)));
        assert!(rn.exceeds_legacy_max_path);
        assert!(!rn.needs_extended_prefix);
        assert!(!norm(r"C:\a").needs_extended_prefix);
    }

    #[test]
    fn unicode_lengths() {
        let n = norm("C:\\\u{1F600}\\file");
        assert_eq!(n.utf16_len, 10); // C : / + emoji(2) + / + file(4)
        assert_eq!(n.byte_len, "C:/\u{1F600}/file".len());
        let cjk = norm("C:\\\u{4E2D}\u{6587}");
        assert_eq!(cjk.utf16_len, 5);
        let comb = norm("C:\\cafe\u{301}");
        assert!(comb.issues.iter().any(|i| i.code == "combining_sequence"));
        assert!(validate("C:\\cafe\u{301}")
            .iter()
            .any(|i| i.code == "combining_sequence"));
    }

    #[test]
    fn validate_clean_and_empty() {
        assert!(validate(r"C:\a\b.txt").is_empty());
        assert!(validate(r"\\srv\share\ok").is_empty());
        let e = validate("");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].code, "empty");
    }

    #[test]
    fn validate_reserved_names() {
        for p in [
            r"C:\CON",
            r"C:\con.txt",
            r"C:\a\NUL",
            r"C:\COM1",
            r"C:\lpt9.doc",
            r"C:\PRN",
        ] {
            let v = validate(p);
            assert!(v.iter().any(|i| i.code == "reserved_name"), "{p}: {v:?}");
        }
        assert!(validate(r"C:\CONSOLE").is_empty());
        assert!(validate(r"C:\COM10").is_empty());
        assert!(validate(r"C:\contact.txt").is_empty());
    }

    #[test]
    fn validate_illegal_chars_and_colon() {
        for p in [
            r"C:\a<b", r"C:\a>b", r"C:\a|b", r"C:\a?b", r"C:\a*b", "C:\\a\"b",
        ] {
            assert!(validate(p).iter().any(|i| i.code == "illegal_char"), "{p}");
        }
        assert!(validate(r"C:\a:b").iter().any(|i| i.code == "illegal_char"));
        assert!(validate(r"C:\ab").is_empty());
        assert!(validate(r"D:rel").is_empty());
    }

    #[test]
    fn validate_trailing_dot_space_long_empty_control() {
        assert!(validate(r"C:\dir.\file")
            .iter()
            .any(|i| i.code == "trailing_dot"));
        assert!(validate(r"C:\dir \file")
            .iter()
            .any(|i| i.code == "trailing_space"));
        let long = format!(r"C:\{}", "x".repeat(256));
        assert!(validate(&long)
            .iter()
            .any(|i| i.code == "component_too_long"));
        let exact = format!(r"C:\{}", "x".repeat(255));
        assert!(!validate(&exact)
            .iter()
            .any(|i| i.code == "component_too_long"));
        assert!(validate(r"C:\a\\b")
            .iter()
            .any(|i| i.code == "empty_component"));
        assert!(!validate(r"C:\a\")
            .iter()
            .any(|i| i.code == "empty_component"));
        assert!(validate("C:\\a\u{1}b")
            .iter()
            .any(|i| i.code == "control_char"));
    }

    #[test]
    fn join_matrix() {
        assert_eq!(join(r"C:\a\b", r"..\c").unwrap().normalized, "C:/a/c");
        assert_eq!(
            join(r"C:\a", "sub/file").unwrap().windows_form,
            r"C:\a\sub\file"
        );
        assert_eq!(join(r"C:\a", r"D:\x").unwrap().normalized, "D:/x");
        assert_eq!(join(r"C:\a", r"\\s\sh\x").unwrap().kind, Kind::Unc);
        assert_eq!(join(r"C:\a\b", r"\x\y").unwrap().normalized, "C:/x/y");
        assert_eq!(join(r"\\s\sh\a", r"\x").unwrap().normalized, "//s/sh/x");
        assert_eq!(join(r"C:\a", r"C:foo").unwrap().normalized, "C:/a/foo");
        assert_eq!(join(r"C:\a", r"D:foo").unwrap().windows_form, r"D:foo");
        assert_eq!(join(r"C:\a", "").unwrap().normalized, "C:/a");
        assert_eq!(join("", r"a\b").unwrap().normalized, "a/b");
    }

    #[test]
    fn relative_matrix() {
        let j = |a: &str, b: &str| crate::json::parse(&relative(a, b).unwrap()).unwrap();
        let v = j(r"C:\a\b\c", r"C:\a\d\e");
        assert_eq!(v.get("relative").unwrap().as_str(), Some("../../d/e"));
        assert_eq!(v.get("windows_form").unwrap().as_str(), Some(r"..\..\d\e"));
        let same = j(r"C:\a\b", r"c:\A\B");
        assert_eq!(same.get("relative").unwrap().as_str(), Some("."));
        assert_eq!(same.get("is_same").unwrap().as_bool(), Some(true));
        let child = j(r"C:\a", r"C:\a\b\c");
        assert_eq!(child.get("relative").unwrap().as_str(), Some("b/c"));
        let unc = j(r"\\S\share\a\b", r"\\s\SHARE\a");
        assert_eq!(unc.get("relative").unwrap().as_str(), Some(".."));
        let ext = j(r"\\?\C:\a\b", r"C:\a");
        assert_eq!(ext.get("relative").unwrap().as_str(), Some(".."));
    }

    #[test]
    fn relative_errors() {
        let e = relative(r"C:\a", r"D:\a").unwrap_err();
        assert_eq!(e.0, ERR_DIFFERENT_ROOT);
        assert!(e.1.contains("different drives"));
        assert_eq!(
            relative(r"\\s\one\a", r"\\s\two\a").unwrap_err().0,
            ERR_DIFFERENT_ROOT
        );
        assert_eq!(relative(r"a\b", r"C:\a").unwrap_err().0, ERR_INVALID_ARG);
        assert_eq!(relative("", r"C:\a").unwrap_err().0, ERR_INVALID_ARG);
    }

    #[test]
    fn extended_round_trip() {
        assert_eq!(to_extended(r"C:\a\b").unwrap(), r"\\?\C:\a\b");
        assert_eq!(to_extended(r"\\s\sh\a").unwrap(), r"\\?\UNC\s\sh\a");
        assert_eq!(to_extended(r"\\?\C:\a").unwrap(), r"\\?\C:\a");
        assert_eq!(to_extended(r"a\b").unwrap(), r"a\b");
        assert_eq!(to_extended(r"C:rel").unwrap(), r"C:rel");
        assert_eq!(from_extended(r"\\?\C:\a\b").unwrap(), r"C:\a\b");
        assert_eq!(from_extended(r"\\?\UNC\s\sh\a").unwrap(), r"\\s\sh\a");
        assert_eq!(from_extended(r"C:\a").unwrap(), r"C:\a");
        for p in [r"C:\a\b", r"\\s\sh\a", r"a\b"] {
            let rt = from_extended(&to_extended(p).unwrap()).unwrap();
            assert_eq!(rt, norm(p).windows_form, "{p}");
        }
        assert_eq!(to_extended("").unwrap_err().0, ERR_INVALID_ARG);
    }

    #[test]
    fn split_json_shape() {
        let v = crate::json::parse(&split_json(r"\\srv\share\a\b").unwrap()).unwrap();
        assert_eq!(v.get("kind").unwrap().as_str(), Some("unc"));
        assert_eq!(v.get("root").unwrap().as_str(), Some("//srv/share/"));
        let comps = v.get("components").unwrap().as_array().unwrap();
        assert_eq!(comps.len(), 2);
        let v2 = crate::json::parse(&split_json(r"C:foo").unwrap()).unwrap();
        assert_eq!(v2.get("root").unwrap().as_str(), Some("C:"));
        assert_eq!(split_json("").unwrap_err().0, ERR_INVALID_ARG);
    }

    #[test]
    fn normalize_json_shape() {
        let n = norm(r"C:\a\file.tar.gz");
        assert_eq!(n.extension, "gz");
        let v = crate::json::parse(&n.to_json()).unwrap();
        for key in [
            "input",
            "normalized",
            "windows_form",
            "extended_form",
            "is_absolute",
            "kind",
            "components",
            "file_name",
            "extension",
            "parent",
            "utf16_len",
            "byte_len",
            "exceeds_legacy_max_path",
            "needs_extended_prefix",
            "issues",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert_eq!(v.get("extension").unwrap().as_str(), Some("gz"));
        assert_eq!(norm(r"C:\.gitignore").extension, "");
        assert_eq!(norm(r"C:\file.").extension, "");
    }

    #[test]
    fn options_errors() {
        assert_eq!(parse_options(Some("{")).unwrap_err().0, ERR_PARSE);
        assert_eq!(
            parse_options(Some(r#"{"bogus":1}"#)).unwrap_err().0,
            ERR_PARSE
        );
        assert_eq!(
            parse_options(Some(r#"{"separator":"/","separator":"\\"}"#))
                .unwrap_err()
                .0,
            ERR_PARSE
        );
        assert_eq!(
            parse_options(Some(r#"{"separator":3}"#)).unwrap_err().0,
            ERR_INVALID_ARG
        );
        assert_eq!(
            parse_options(Some(r#"{"separator":"|"}"#)).unwrap_err().0,
            ERR_INVALID_ARG
        );
        assert!(parse_options(None).is_ok());
        assert!(parse_options(Some("")).is_ok());
    }

    #[test]
    fn escapes_json_still_works() {
        assert_eq!(json_escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
    }
}
