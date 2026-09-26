//! The gate on `docs/formalism/README.md` — "The ikigai realisation", the formal
//! companion to Peter's Hotel. A paper has no gate; this document does: every
//! definition, theorem and deviation in it carries a **pin** naming the test or
//! doctest that holds its precondition, and this test resolves every pin against
//! the source tree. A renamed or deleted test turns the document red. A document
//! naming zero pins cannot pass, so an empty document cannot pass either.
//!
//! Three forms, each on a line of its own (or a table cell, or a `;`-separated
//! item within one):
//!
//! ```text
//! pin: `module::tests::test_name` (crates/ikigai-core/src/module.rs)
//! doctest: `Type::item`
//! UNPINNED — what a test would assert
//! ```
//!
//! - `pin:` resolves to a `fn <name>` under a `#[…test…]` attribute. With a file in
//!   parentheses, that file must contain it and the path's first segment must be the
//!   file's stem (`kernel::tests::x` lives in `kernel.rs`; `scope::x` in `tests/scope.rs`).
//! - `doctest:` resolves to a public item — `Type`, or `Type::member` for a field or
//!   method of `Type` — whose `///` block carries a fenced example.
//! - `UNPINNED` must carry a sentence after an em dash. It is counted, never resolved:
//!   the register of these is the document's finding.
//!
//! Fenced code blocks in the document are skipped, so the format can be shown there.
//! Reads only this repository's tree — no HOME, no git history, nothing a shallow CI
//! checkout lacks.

use std::fs;
use std::path::{Path, PathBuf};

/// The repository root: two levels above this crate's manifest.
fn repo_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(
        root.join("Cargo.toml").is_file(),
        "expected the workspace root at {}",
        root.display()
    );
    // Canonical, so a failure names a path a reader can open rather than one
    // with `..` segments in it.
    root.canonicalize().unwrap_or(root)
}

/// Walk `dir` for files with `ext`.
fn files_with(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            files_with(&path, ext, out);
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

#[derive(Debug)]
enum Pin {
    /// A `#[test]` function, by its last path segment, with the path's first
    /// segment and the file the document names (if any).
    Test { path: String, file: Option<String> },
    /// A public item with a doctest: `Type` or `Type::member`.
    Doctest { item: String },
    /// A claim the code cannot vouch for, with what a test would assert.
    Unpinned { claim: String },
}

/// Where a pin was written: `docs/formalism/README.md:123`.
#[derive(Debug)]
struct Sited {
    at: String,
    pin: Pin,
}

const MARKERS: [&str; 3] = ["pin: `", "doctest: `", "UNPINNED"];

/// Split a unit (a line, or a table cell) into the segments that start with a
/// marker, each marker beginning the unit or following a `;`.
fn segments(unit: &str) -> Vec<&str> {
    let unit = unit.trim();
    let mut starts = Vec::new();
    for marker in MARKERS {
        let mut from = 0;
        while let Some(at) = unit[from..].find(marker) {
            let p = from + at;
            let before = unit[..p].trim_end();
            if before.is_empty() || before.ends_with(';') {
                starts.push(p);
            }
            from = p + marker.len();
        }
    }
    starts.sort_unstable();
    starts.dedup();
    let mut out = Vec::new();
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(unit.len());
        out.push(unit[start..end].trim().trim_end_matches(';').trim());
    }
    out
}

/// Parse one segment into a pin, or explain why it is malformed.
fn parse(segment: &str) -> Result<Pin, String> {
    if let Some(rest) = segment.strip_prefix("pin: `") {
        let (path, rest) = rest
            .split_once('`')
            .ok_or_else(|| "pin: missing closing backtick".to_string())?;
        let rest = rest.trim();
        let file = if let Some(inner) = rest.strip_prefix('(') {
            let (file, tail) = inner
                .split_once(')')
                .ok_or_else(|| "pin: unclosed file parenthesis".to_string())?;
            if !tail.trim().is_empty() {
                return Err(format!("pin: trailing text after the file: `{tail}`"));
            }
            Some(file.trim().to_string())
        } else if rest.is_empty() {
            None
        } else {
            return Err(format!("pin: trailing text after the name: `{rest}`"));
        };
        if path.is_empty() {
            return Err("pin: empty name".to_string());
        }
        return Ok(Pin::Test {
            path: path.to_string(),
            file,
        });
    }
    if let Some(rest) = segment.strip_prefix("doctest: `") {
        let (item, tail) = rest
            .split_once('`')
            .ok_or_else(|| "doctest: missing closing backtick".to_string())?;
        if !tail.trim().is_empty() {
            return Err(format!("doctest: trailing text: `{tail}`"));
        }
        if item.is_empty() {
            return Err("doctest: empty item".to_string());
        }
        return Ok(Pin::Doctest {
            item: item.to_string(),
        });
    }
    if let Some(rest) = segment.strip_prefix("UNPINNED") {
        let claim = rest
            .trim()
            .strip_prefix('—')
            .ok_or_else(|| "UNPINNED must be followed by an em dash and a sentence".to_string())?
            .trim();
        if claim.is_empty() {
            return Err("UNPINNED carries no sentence".to_string());
        }
        return Ok(Pin::Unpinned {
            claim: claim.to_string(),
        });
    }
    Err(format!("not a pin: `{segment}`"))
}

/// Every pin in one Markdown file, with its line; malformed ones are errors.
fn pins_in(doc: &Path, text: &str, errors: &mut Vec<String>) -> Vec<Sited> {
    let mut pins = Vec::new();
    let mut in_fence = false;
    for (n, line) in text.lines().enumerate() {
        let at = format!("{}:{}", doc.display(), n + 1);
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let units: Vec<&str> = if trimmed.starts_with('|') {
            trimmed.split('|').collect()
        } else {
            let mut unit = trimmed;
            for prefix in ["- ", "* ", "> "] {
                unit = unit.strip_prefix(prefix).unwrap_or(unit);
            }
            vec![unit]
        };
        for unit in units {
            let unit = unit.trim();
            // A bare marker without the backtick is a typo in a pin, not prose.
            if (unit.starts_with("pin:") && !unit.starts_with("pin: `"))
                || (unit.starts_with("doctest:") && !unit.starts_with("doctest: `"))
            {
                errors.push(format!("{at}: malformed pin `{unit}`"));
                continue;
            }
            for segment in segments(unit) {
                match parse(segment) {
                    Ok(pin) => pins.push(Sited {
                        at: at.clone(),
                        pin,
                    }),
                    Err(why) => errors.push(format!("{at}: {why}")),
                }
            }
        }
    }
    pins
}

/// Strip the qualifiers that may precede `fn`.
fn after_qualifiers(line: &str) -> &str {
    let mut t = line.trim_start();
    loop {
        let mut changed = false;
        for q in [
            "pub(crate) ",
            "pub(super) ",
            "pub ",
            "async ",
            "const ",
            "unsafe ",
        ] {
            if let Some(rest) = t.strip_prefix(q) {
                t = rest;
                changed = true;
            }
        }
        if !changed {
            return t;
        }
    }
}

/// Whether `t` (qualifiers stripped) declares `fn <name>` followed by `(` or `<`.
fn declares_fn(t: &str, name: &str) -> bool {
    t.strip_prefix("fn ")
        .and_then(|r| r.strip_prefix(name))
        .is_some_and(|r| r.starts_with('(') || r.starts_with('<'))
}

/// The attribute and doc lines immediately above line `i`, nearest first.
fn preamble(lines: &[&str], i: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut j = i;
    while j > 0 {
        j -= 1;
        let l = lines[j].trim();
        if l.is_empty() || l.starts_with("///") || l.starts_with("//") || l.starts_with("#[") {
            out.push(l.to_string());
        } else {
            break;
        }
    }
    out
}

/// `Some(true)` if `name` is declared as a test fn in `text`, `Some(false)` if
/// declared but not under a test attribute, `None` if not declared at all.
fn test_fn_in(text: &str, name: &str) -> Option<bool> {
    let lines: Vec<&str> = text.lines().collect();
    let mut found = None;
    for (i, line) in lines.iter().enumerate() {
        if declares_fn(after_qualifiers(line), name) {
            // `#[test]`, or a runtime's `#[tokio::test]` / `#[tokio::test(flavor = …)]`;
            // never `#[cfg(test)]`, which gates a helper without making it a test.
            let is_test = preamble(&lines, i)
                .iter()
                .any(|l| l == "#[test]" || (l.starts_with("#[") && l.contains("::test")));
            if is_test {
                return Some(true);
            }
            found = Some(false);
        }
    }
    found
}

/// Whether the doc block above line `i` carries a fenced example.
fn has_fence(lines: &[&str], i: usize) -> bool {
    preamble(lines, i)
        .iter()
        .any(|l| l.starts_with("///") && l.contains("```"))
}

/// Whether `t` (qualifiers stripped) declares the item `name` (struct, enum, trait,
/// fn, type, const, static).
fn declares_item(t: &str, name: &str) -> bool {
    for kw in [
        "struct ", "enum ", "trait ", "fn ", "type ", "const ", "static ",
    ] {
        if let Some(rest) = t.strip_prefix(kw) {
            if let Some(after) = rest.strip_prefix(name) {
                return after.is_empty()
                    || after.starts_with(|c: char| {
                        c.is_whitespace() || matches!(c, '{' | '<' | '(' | ';' | ':' | '=')
                    });
            }
        }
    }
    false
}

/// Whether `t` (qualifiers stripped) declares field or method `member`.
fn declares_member(t: &str, member: &str) -> bool {
    declares_fn(t, member)
        || t.strip_prefix(member)
            .is_some_and(|after| after.starts_with(':') && !after.starts_with("::"))
}

/// Resolve a doctest pin over the source files: `Type` needs a fence on the
/// type's own doc block; `Type::member` needs the file that defines `Type` to
/// declare `member` under a doc block with a fence.
fn doctest_exists(sources: &[(PathBuf, String)], item: &str) -> Result<(), String> {
    let (ty, member) = match item.split_once("::") {
        Some((ty, member)) => (ty, Some(member)),
        None => (item, None),
    };
    // The first file declaring `ty` is the file examined: modules here are one type
    // per file, and a type declared twice would be a different problem.
    for (path, text) in sources {
        let lines: Vec<&str> = text.lines().collect();
        let defines_type = lines.iter().any(|l| declares_item(after_qualifiers(l), ty));
        if !defines_type {
            continue;
        }
        for (i, line) in lines.iter().enumerate() {
            let t = after_qualifiers(line);
            let hit = match member {
                None => declares_item(t, ty),
                Some(m) => declares_member(t, m),
            };
            if hit && has_fence(&lines, i) {
                return Ok(());
            }
        }
        return Err(match member {
            None => format!(
                "`{ty}` ({}) has no fenced example in its doc block",
                path.display()
            ),
            Some(m) => format!(
                "`{ty}::{m}` ({}) is not declared under a doc block with a fenced example",
                path.display()
            ),
        });
    }
    Err(format!("`{item}`: no source file declares `{ty}`"))
}

#[test]
fn every_pin_in_the_formalism_resolves_and_the_document_is_not_empty() {
    let root = repo_root();
    let docs_dir = root.join("docs/formalism");
    assert!(
        docs_dir.is_dir(),
        "docs/formalism is missing at {} — the formal companion is gone, or this test is \
         running outside the repository",
        docs_dir.display()
    );

    let mut docs = Vec::new();
    files_with(&docs_dir, "md", &mut docs);
    assert!(!docs.is_empty(), "docs/formalism holds no Markdown");

    let mut errors = Vec::new();
    let mut pins = Vec::new();
    for doc in &docs {
        let text = fs::read_to_string(doc).expect("readable document");
        let rel = doc.strip_prefix(&root).unwrap_or(doc);
        pins.extend(pins_in(rel, &text, &mut errors));
    }

    let mut source_paths = Vec::new();
    files_with(
        &root.join("crates/ikigai-core/src"),
        "rs",
        &mut source_paths,
    );
    let mut test_paths = Vec::new();
    files_with(
        &root.join("crates/ikigai-core/tests"),
        "rs",
        &mut test_paths,
    );
    let read = |paths: &[PathBuf]| -> Vec<(PathBuf, String)> {
        paths
            .iter()
            .map(|p| (p.clone(), fs::read_to_string(p).expect("readable source")))
            .collect()
    };
    let sources = read(&source_paths);
    let all: Vec<(PathBuf, String)> = sources
        .iter()
        .chain(read(&test_paths).iter())
        .cloned()
        .collect();

    let (mut tests, mut doctests, mut unpinned) = (0usize, 0usize, 0usize);
    for Sited { at, pin } in &pins {
        match pin {
            Pin::Test { path, file } => {
                tests += 1;
                let name = path.rsplit("::").next().unwrap_or(path);
                match file {
                    Some(file) => {
                        let full = root.join(file);
                        let Ok(text) = fs::read_to_string(&full) else {
                            errors.push(format!("{at}: `{path}` names a missing file {file}"));
                            continue;
                        };
                        if let Some((first, _)) = path.split_once("::") {
                            let stem = full.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                            if first != stem {
                                errors.push(format!(
                                    "{at}: `{path}` says module `{first}` but points at {file}"
                                ));
                            }
                        }
                        match test_fn_in(&text, name) {
                            Some(true) => {}
                            Some(false) => errors.push(format!(
                                "{at}: `{name}` exists in {file} but is not a #[test]"
                            )),
                            None => errors.push(format!("{at}: `{name}` is not in {file}")),
                        }
                    }
                    None => {
                        let hit = all
                            .iter()
                            .any(|(_, text)| test_fn_in(text, name) == Some(true));
                        if !hit {
                            errors.push(format!("{at}: no test named `{name}` in the tree"));
                        }
                    }
                }
            }
            Pin::Doctest { item } => {
                doctests += 1;
                if let Err(why) = doctest_exists(&sources, item) {
                    errors.push(format!("{at}: doctest {why}"));
                }
            }
            Pin::Unpinned { claim } => {
                unpinned += 1;
                let _ = claim;
            }
        }
    }

    println!(
        "formalism pins: {tests} tests, {doctests} doctests, {unpinned} UNPINNED across {} document(s)",
        docs.len()
    );
    assert!(
        errors.is_empty(),
        "{} pin(s) do not resolve:\n  {}",
        errors.len(),
        errors.join("\n  ")
    );
    assert!(
        tests + doctests > 0,
        "the formal companion names no pins at all — an empty document cannot pass"
    );
}

/// The parser must see what it claims to, or the gate above is hollow: a
/// malformed marker is an error, a fenced example is skipped, table cells and
/// `;`-separated items are units, and prose mentioning a marker mid-line is not.
#[test]
fn the_pin_parser_sees_what_it_claims_to() {
    let doc = Path::new("x.md");
    let text = "\
prose that mentions pin: `nothing` mid-line is ignored
    pin: `kernel::tests::alpha` (crates/ikigai-core/src/kernel.rs)
    doctest: `Resolved::canonical`
    UNPINNED — a test would assert something
| a | pin: `scope::beta` (crates/ikigai-core/tests/scope.rs); doctest: `Scope` |
- UNPINNED — in a list
```text
pin: `inside::a_fence` (nowhere.rs)
```
pin: not a pin
UNPINNED without a dash
";
    let mut errors = Vec::new();
    let pins = pins_in(doc, text, &mut errors);
    let kinds: Vec<&str> = pins
        .iter()
        .map(|s| match &s.pin {
            Pin::Test { path, .. } => path.as_str(),
            Pin::Doctest { item } => item.as_str(),
            Pin::Unpinned { claim } => claim.as_str(),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "kernel::tests::alpha",
            "Resolved::canonical",
            "a test would assert something",
            "scope::beta",
            "Scope",
            "in a list",
        ],
        "{pins:?}"
    );
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[0].contains("x.md:10") && errors[0].contains("malformed"));
    assert!(errors[1].contains("x.md:11") && errors[1].contains("em dash"));

    // The resolvers, against fixtures rather than the live tree.
    let src = "\
/// Doc.
///
/// ```
/// let x = 1;
/// ```
pub struct Widget {
    /// Field doc, no fence.
    pub plain: u32,
    /// ```
    /// let y = 2;
    /// ```
    pub shown: u32,
}

impl Widget {
    /// ```
    /// let z = 3;
    /// ```
    pub fn build<T>(t: T) -> T { t }
}

#[cfg(test)]
mod tests {
    #[test]
    fn is_a_test() {}

    fn not_a_test() {}

    #[tokio::test]
    async fn is_an_async_test(
    ) {}
}
";
    assert_eq!(test_fn_in(src, "is_a_test"), Some(true));
    assert_eq!(test_fn_in(src, "is_an_async_test"), Some(true));
    assert_eq!(test_fn_in(src, "not_a_test"), Some(false));
    assert_eq!(test_fn_in(src, "absent"), None);
    let sources = vec![(PathBuf::from("widget.rs"), src.to_string())];
    assert!(doctest_exists(&sources, "Widget").is_ok());
    assert!(doctest_exists(&sources, "Widget::shown").is_ok());
    assert!(doctest_exists(&sources, "Widget::build").is_ok());
    assert!(doctest_exists(&sources, "Widget::plain").is_err());
    assert!(doctest_exists(&sources, "Gadget").is_err());
}
