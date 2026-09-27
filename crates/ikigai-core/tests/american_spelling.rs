//! American spelling in what this repository writes: the formal document, the design
//! notes, the code comments and the vocabulary's prose (which is published at
//! <https://ikigai-rs.dev/ns>). A short denylist of spellings that are British and
//! nothing else — no word on it has an American reading, so a hit is never a false one.
//!
//! Scanned: every `.md` under `docs/` and `crates/`, the root `README.md`, every `.rs`
//! under `crates/`, and `crates/ikigai-vocab/src/vocabulary.ttl`. Words are split on
//! anything that is not a letter, so a `snake_case` test name is checked part by part.
//!
//! **A direct quote keeps its own spelling.** Put `spelling: quote` anywhere on the
//! line — in Markdown an HTML comment (`<!-- spelling: quote -->`) renders as nothing —
//! and the line is skipped. External API names (reportlab's `drawCentredString`) are
//! out of scope by construction: nothing here matches inside a longer word.
//!
//! This file is excluded from the scan, since it has to spell the denylist out.
//! Excluded from the packaged crate for the same reason as `formalism_pins.rs`: it
//! reads `docs/`, which lives above the crate.

use std::fs;
use std::path::{Path, PathBuf};

/// Any word that begins with one of these is British: behaviour(al), colour(ful),
/// honour(ed), neighbour(hood), favour(ite), flavour(s).
const OUR_STEMS: [&str; 6] = [
    "behaviour",
    "colour",
    "honour",
    "neighbour",
    "favour",
    "flavour",
];

/// `-ise` stems that are British only with one of `ISE_SUFFIXES` after them. Stem and
/// suffix together, so `realism` and `formalism` never match.
const ISE_STEMS: [&str; 11] = [
    "realis",
    "serialis",
    "normalis",
    "initialis",
    "recognis",
    "organis",
    "summaris",
    "authoris",
    "optimis",
    "memois",
    "categoris",
];
const ISE_SUFFIXES: [&str; 9] = [
    "e", "ed", "es", "ing", "ation", "ations", "er", "ers", "ably",
];

/// Whole words.
const EXACT: [&str; 17] = [
    "modelled",
    "modelling",
    "labelled",
    "labelling",
    "travelled",
    "travelling",
    "cancelled",
    "cancelling",
    "artefact",
    "artefacts",
    "catalogue",
    "catalogues",
    "centre",
    "centres",
    "analyse",
    "analysed",
    "analysing",
];

const QUOTE_MARKER: &str = "spelling: quote";

fn is_british(word: &str) -> bool {
    let w = word.to_ascii_lowercase();
    OUR_STEMS.iter().any(|s| w.starts_with(s))
        || EXACT.contains(&w.as_str())
        || ISE_STEMS.iter().any(|s| {
            w.strip_prefix(s)
                .is_some_and(|rest| ISE_SUFFIXES.contains(&rest))
        })
}

/// The repository root: two levels above this crate's manifest.
fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(
        root.join("Cargo.toml").is_file(),
        "expected the workspace root at {}",
        root.display()
    );
    root.canonicalize().unwrap_or(root)
}

fn walk(dir: &Path, exts: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            walk(&path, exts, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| exts.contains(&e))
        {
            out.push(path);
        }
    }
}

#[test]
fn the_denylist_matches_what_it_should_and_nothing_else() {
    for british in [
        "behaviour",
        "Behavioural",
        "colours",
        "honoured",
        "neighbourhood",
        "realised",
        "Realisation",
        "serialisers",
        "modelled",
        "artefact",
    ] {
        assert!(is_british(british), "{british} should be refused");
    }
    for fine in [
        "behavior",
        "realism",
        "formalism",
        "realize",
        "serialize",
        "analysis",
        "center",
        "drawCentredString",
        "modeled",
        "otherwise",
        "promise",
    ] {
        assert!(!is_british(fine), "{fine} is not British-only");
    }
}

#[test]
fn what_this_repository_writes_is_spelled_the_american_way() {
    let root = repo_root();
    let mut files = Vec::new();
    walk(&root.join("docs"), &["md"], &mut files);
    let docs = files.len();
    walk(&root.join("crates"), &["md", "rs"], &mut files);
    files.push(root.join("README.md"));
    files.push(root.join("crates/ikigai-vocab/src/vocabulary.ttl"));
    assert!(
        docs > 0,
        "docs/ holds no Markdown — the scan would pass vacuously"
    );

    let this = Path::new(file!())
        .file_name()
        .expect("this test has a file name")
        .to_owned();
    let mut hits = Vec::new();
    for path in &files {
        if path.ends_with(Path::new("tests").join(&this)) {
            continue;
        }
        let text =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        for (n, line) in text.lines().enumerate() {
            if line.contains(QUOTE_MARKER) {
                continue;
            }
            for word in line.split(|c: char| !c.is_ascii_alphabetic()) {
                if !word.is_empty() && is_british(word) {
                    let rel = path.strip_prefix(&root).unwrap_or(path);
                    hits.push(format!("{}:{}: {word}", rel.display(), n + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "{} British spelling(s) — write American (behavior, color, honor, realize, \
         serialize, modeled, artifact), or mark a direct quote with `{QUOTE_MARKER}` on \
         its line:\n  {}",
        hits.len(),
        hits.join("\n  ")
    );
}
