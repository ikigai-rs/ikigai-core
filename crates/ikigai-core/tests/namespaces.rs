//! **`grep -r urn:iki:` is a progress meter, and this test keeps it honest.**
//!
//! The ecosystem is migrating its `urn:` namespaces under a single `urn:iki:`
//! root, one at a time. Exactly one has actually moved — `urn:fn:`, published as
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

use std::fs;
use std::path::Path;

/// The `urn:iki:` sub-namespaces this crate is allowed to mention. Add to this ONLY
/// when the namespace has actually been published under its new name.
const MIGRATED: &[&str] = &["fn"];

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
            if !MIGRATED.contains(&name.as_str()) {
                let rel = file.strip_prefix(root).unwrap_or(file);
                offenders.push(format!("  {}:{line}  urn:iki:{name}", rel.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "`urn:iki:` names a namespace that has not migrated — fictional namespaces \
         belong under `urn:example:` (RFC 6963), or add the name to MIGRATED once it \
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
