//! **`grep -r urn:iki:` is a progress meter, and this test keeps it honest.**
//!
//! The ecosystem is migrating its `urn:` namespaces under a single `urn:iki:`
//! root, one at a time. Exactly one has actually MOVED — `urn:fn:`, published as
//! `ikigai-fn` 0.2.0 — so "what has moved?" is answerable by grep. It stops being
//! answerable the moment a *fixture* names a namespace that does not exist:
//! answering the question then means opening every hit to find out whether it is a
//! migration or a test, and that cost grows with every namespace that really moves.
//!
//! This crate was the largest single source of that noise (69 hits). Fictional
//! namespaces now live under the IANA-reserved `urn:example:` (RFC 6963), which is
//! registered for exactly this purpose. `urn:iki:kernel:` was the worst of them:
//! `urn:kernel:` is a real namespace that will migrate, so that fixture was not
//! merely fictional — it would one day have been ambiguous with production usage.
//!
//! A comment alone would not hold the line; the next test author reaches for
//! `urn:iki:whatever` and the meter degrades again. So the invariant is checked
//! rather than described — and note what is checked is the *namespace set*, not
//! prose: when a second namespace genuinely migrates, this list grows by one line,
//! which is the correct signal rather than friction.
//!
//! ★ One distinction the list must not blur, added 2026-09-13 with `urn:iki:store:`.
//! A namespace can be real under this root two ways: it MIGRATED from an older name
//! (`urn:fn:` did), or it was BORN here and never had an older name (`ikigai-store`
//! 0.2.0 was — a brand-new namespace is the only kind whose migration is free, so a
//! new module should start under the root rather than migrate into it later). Both
//! are legitimate mentions and both belong in the list below; only the first is
//! evidence of migration PROGRESS. Counting the list would therefore over-report, so
//! the list is named for what it authorizes rather than for how a name got there.

use std::fs;
use std::path::Path;

/// The `urn:iki:` sub-namespaces this crate is allowed to mention. Add to this ONLY
/// when the namespace has actually been published under that name. Say which kind
/// each one is — see the module doc on migrated-versus-born.
const REAL: &[&str] = &[
    // MIGRATED from `urn:fn:` — published as `ikigai-fn` 0.2.0.
    "fn",
    // BORN here — `ikigai-store` 0.2.0 (2026-09-13) never had an older namespace.
    "store",
];

/// Every `urn:iki:<name>` occurrence in `text`, with its line number.
fn iki_namespaces(text: &str) -> Vec<(usize, String)> {
    let mut found = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let mut rest = line;
        while let Some(at) = rest.find("urn:iki:") {
            rest = &rest[at + "urn:iki:".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            // A bare `urn:iki:` is prose about the migration itself, not a namespace.
            if !name.is_empty() {
                found.push((n + 1, name));
            }
        }
    }
    found
}

/// Walk `dir` for `.rs` files. Reads only inside this crate — no HOME, no git
/// history, nothing a shallow CI checkout lacks.
fn rust_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn urn_iki_appears_only_for_namespaces_that_actually_migrated() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("tests"), &mut files);
    assert!(!files.is_empty(), "found no sources to scan under {root:?}");

    let mut offenders = Vec::new();
    for file in &files {
        // This file names the reserved namespace in prose; scanning it would be
        // self-referential.
        if file.file_name().is_some_and(|f| f == "namespaces.rs") {
            continue;
        }
        let text = fs::read_to_string(file).expect("readable source");
        for (line, name) in iki_namespaces(&text) {
            if !REAL.contains(&name.as_str()) {
                let rel = file.strip_prefix(root).unwrap_or(file);
                offenders.push(format!("  {}:{line}  urn:iki:{name}", rel.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "`urn:iki:` names a namespace that has not migrated — fictional namespaces \
         belong under `urn:example:` (RFC 6963), or add the name to REAL once it \
         is really published:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_detector_sees_what_it_claims_to() {
    // The guard is only worth having if it actually fires, so prove both directions
    // on synthetic text rather than trusting the corpus to stay interesting.
    let hits = iki_namespaces(
        "prefix urn:fn: urn:iki:fn:\n\
         let t = table.prefix(\"urn:store:\", \"urn:iki:store:\");\n\
         // the urn:iki: migration\n\
         two urn:iki:vault: and urn:iki:fs: on one line\n",
    );
    assert_eq!(
        hits,
        vec![
            (1, "fn".to_string()),
            (2, "store".to_string()),
            (4, "vault".to_string()),
            (4, "fs".to_string()),
        ],
        "bare `urn:iki:` prose must not count, and a line may carry several"
    );
}

// ---------------------------------------------------------------- `urn:fn:`

/// The files allowed to *execute* `urn:fn:` names, and why: they are the worked
/// example of the alias mechanism itself, where the whole point is that the legacy
/// name still resolves. Everywhere else in this crate a `urn:fn:` occurrence must
/// sit in a comment — core has no `ikigai-fn` dependency, so any name it actually
/// binds, requests or matches under that prefix is a name it does not own.
const ALIAS_SUBJECT_MATTER: &[&str] = &["alias.rs"];

/// Lines of `text` that put `urn:fn:` in executable position, with line numbers. A
/// line counts as commentary when it *starts* with `//` (so `//`, `///` and `//!`,
/// including a `///` doctest line, are all commentary).
fn executed_urn_fn(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.contains("urn:fn:") && !line.trim_start().starts_with("//"))
        .map(|(n, line)| (n + 1, line.trim().to_string()))
        .collect()
}

/// ikigai-core once carried 125 `urn:fn:` occurrences and no `ikigai-fn` dependency:
/// 71 of them were fixtures binding core's *own* `builtins` at another module's
/// name, so a rename in `ikigai-fn` reached into this crate for no reason. They now
/// live at `urn:test:` (fixtures) and `urn:example:` (illustration), which takes
/// this crate out of the `urn:fn:` migration permanently — wave two included.
///
/// What is left is deliberate: `alias.rs` demonstrates `urn:fn:` → `urn:iki:fn:`,
/// and a handful of doc comments describe that migration. A comment cannot bind
/// anything, so the line this test holds is **executable code** — which is exactly
/// the defect it was written for (`bind(Exact::new("urn:fn:toUpper"), …)`).
///
/// ⚠ Known limit, stated rather than hidden: a `/* … */` block comment reads as
/// code here and would be reported. That is the safe direction — it fails loud.
#[test]
fn urn_fn_is_executed_only_where_the_alias_migration_is_the_subject() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("tests"), &mut files);
    assert!(!files.is_empty(), "found no sources to scan under {root:?}");

    let mut offenders = Vec::new();
    for file in &files {
        let name = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        // This file names the prefix in prose and in the detector's own fixtures.
        if name == "namespaces.rs" || ALIAS_SUBJECT_MATTER.contains(&name.as_str()) {
            continue;
        }
        let text = fs::read_to_string(file).expect("readable source");
        for (line, source) in executed_urn_fn(&text) {
            let rel = file.strip_prefix(root).unwrap_or(file);
            offenders.push(format!("  {}:{line}  {source}", rel.display()));
        }
    }

    assert!(
        offenders.is_empty(),
        "ikigai-core does not depend on `ikigai-fn` and must not bind, request or \
         match a name under its namespace. Fixtures belong at `urn:test:`, \
         illustrations at `urn:example:` (RFC 6963):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_executable_position_detector_sees_what_it_claims_to() {
    // Prove both directions on synthetic text: comments about the migration are
    // fine at any indentation, a binding is not.
    let source = [
        "//! `urn:fn:toUpper` -> `urn:iki:fn:toUpper` is the one real migration.",
        "    /// let table = AliasTable::new().prefix(\"urn:fn:\", \"urn:iki:fn:\");",
        "        // a plain indented comment about urn:fn:",
        "        .bind(Exact::new(\"urn:fn:toUpper\"), builtins::to_upper())",
    ]
    .join("\n");
    assert_eq!(
        executed_urn_fn(&source),
        vec![(
            4,
            ".bind(Exact::new(\"urn:fn:toUpper\"), builtins::to_upper())".to_string()
        )],
        "only the binding is in executable position"
    );
}
