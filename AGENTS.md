# PathKit — agent notes

- Pure logic only: no OS calls, no filesystem access, ever. `src/platform.rs` stays an empty documented stub; a "helpful" change that stats or canonicalizes a path on disk breaks the lexical contract (and the determinism of every test).
- The `..` rules are the contract, not a style choice: at the root of an absolute path a `..` is clamped AND reported as `escapes_root`; in a relative or drive-relative path leading `..` segments are kept. Do not unify these — each half exists because the other behaviour is wrong for that kind.
- Lengths are UTF-16 code units where Windows counts (component limit 255, `exceeds_legacy_max_path` at >260 of the *Windows* form). Counting Rust chars or bytes instead reintroduces exactly the long-path bugs this kit exists to prevent.
- `combining_sequence` is a documented heuristic over combining-mark ranges, NOT Unicode normalization. Never upgrade the wording (header, README, issue message) to claim NFC/NFD detection without actually shipping normalization tables — which would break the zero-dependency rule and needs a deliberate decision first.
- The JSON engine (src/json.rs) is the family design — duplicate keys in one object are a PARSE ERROR by design; do not "fix" that into last-wins. Options parsing follows it: unknown keys are `ERR_PARSE`, wrong types are `ERR_INVALID_ARG`.
- Case-insensitivity is scoped: root keys (drive letter, UNC server+share) and relative-path component matching are case-insensitive; component text is otherwise preserved exactly as typed. Do not lowercase output paths.
- Gate: WinDevKit tools/gate.sh (fmt + clippy both targets + full tests). Check/clippy do not link — CI's real Windows build is the link gate.
