//! Keep repository sources and documentation aligned with the plan/script split.

use std::{path::Path, process::Command};

#[test]
fn repository_uses_the_plan_script_split() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(&root)
        .output()
        .expect("list repository files");
    assert!(files.status.success(), "git ls-files failed");

    // Assemble the retired spelling so this guard does not match itself.
    let retired = ["urn:", "program:"].concat();
    let mut matches = Vec::new();
    for name in files
        .stdout
        .split(|byte| *byte == 0)
        .filter(|s| !s.is_empty())
    {
        let name = std::str::from_utf8(name).expect("UTF-8 repository path");
        let bytes = std::fs::read(root.join(name)).expect("read repository file");
        // Check every tracked text file, including sources, docs and fixtures.
        if let Ok(text) = std::str::from_utf8(&bytes) {
            for (line, text) in text.lines().enumerate() {
                if text.contains(&retired) {
                    matches.push(format!("{name}:{}", line + 1));
                }
            }
        }
    }
    assert!(
        matches.is_empty(),
        "retired plan namespace; describe the plan/script split instead:\n{}",
        matches.join("\n")
    );
}
