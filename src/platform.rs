//! Platform layer. PathKit is pure logic: it makes no OS
//! calls on any platform (path strings are parsed and
//! rewritten, never opened), so there is nothing
//! Windows-only to declare here. This module exists to
//! keep the family layout and to be the one place a
//! future OS call would live, behind `cfg(windows)` with
//! hand-declared externs.
