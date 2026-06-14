//! Capability-confined file endpoint.
//!
//! [`FileEndpoint`] reads files, but holds **no inherent authority over the
//! filesystem**: it is jailed to a `root` directory given at construction and
//! will never serve a path outside it. The requested path comes from the `path`
//! binding captured by the resolving grammar (e.g. a `file:` template), so the
//! file's *identity* is the request — not an out-of-band argument.
//!
//! Confinement is default-deny by construction:
//! - `..` (parent-directory) and absolute path segments are rejected outright;
//! - when the target exists, its canonical path must still sit within the
//!   canonical root (symlink-safe).
//!
//! The native representation is the file's bytes (with a media type guessed from
//! the extension); byte→string/graph transforms are the transform layer's job.
//!
//! The capability in the invocation context will, once the authorization layer
//! lands, carry the specific path/verb grant; today the jail root is the
//! confinement and any capability authorizes within it.

use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use ikigai_core::{
    ArgSpec, Description, Endpoint, Error, Invocation, ReprType, Representation, Result, Verb,
};

/// A file endpoint jailed to a root directory.
pub struct FileEndpoint {
    root: PathBuf,
}

impl FileEndpoint {
    /// A file endpoint that will only ever serve paths within `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        FileEndpoint { root: root.into() }
    }

    /// Resolve a request-relative path to a real path within the root, or deny.
    fn resolve_within_root(&self, rel: &str) -> Result<PathBuf> {
        for component in Path::new(rel).components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir => {
                    return Err(deny("parent-directory (`..`) segments are not allowed"));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(deny("absolute paths are not allowed"));
                }
            }
        }
        let target = self.root.join(rel);
        // Symlink-safe containment check when the target already exists.
        if let (Ok(canonical_root), Ok(canonical_target)) =
            (self.root.canonicalize(), target.canonicalize())
        {
            if !canonical_target.starts_with(&canonical_root) {
                return Err(Error::Endpoint(
                    "resolved path escapes the endpoint root".to_string(),
                ));
            }
        }
        Ok(target)
    }

    fn read(&self, rel: &str) -> Result<Representation> {
        let path = self.resolve_within_root(rel)?;
        let bytes = std::fs::read(&path)
            .map_err(|e| Error::Endpoint(format!("read {}: {e}", path.display())))?;
        Ok(Representation::new(media_type_for(&path), bytes))
    }
}

#[async_trait]
impl Endpoint for FileEndpoint {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        match inv.request.verb {
            Verb::Meta => Ok(ikigai_vocab::describe_turtle(&self.describe())),
            Verb::Source => {
                let rel = inv
                    .bindings
                    .get("path")
                    .ok_or_else(|| Error::MissingArgument("path".to_string()))?;
                self.read(rel)
            }
            other => Err(Error::Endpoint(format!(
                "file endpoint does not support the {other:?} verb"
            ))),
        }
    }

    fn name(&self) -> &str {
        "file"
    }

    fn describe(&self) -> Description {
        Description::new("file")
            .title("Capability-confined file endpoint")
            .summary(
                "Reads a file resolved relative to the endpoint root; default-deny outside it.",
            )
            .verb(Verb::Source)
            .verb(Verb::Meta)
            .input(
                ArgSpec::new("path")
                    .summary("Path relative to the endpoint root (no `..`, no absolute paths)."),
            )
            .output("application/octet-stream")
    }
}

fn deny(detail: &str) -> Error {
    Error::InvalidArgument {
        name: "path".to_string(),
        detail: detail.to_string(),
    }
}

fn media_type_for(path: &Path) -> ReprType {
    let media = match path.extension().and_then(|e| e.to_str()) {
        Some("txt") => "text/plain",
        Some("ttl") => "text/turtle",
        Some("nt") => "application/n-triples",
        Some("json") => "application/json",
        Some("jsonld") => "application/ld+json",
        Some("html") => "text/html",
        _ => "application/octet-stream",
    };
    ReprType::new(media)
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use ikigai_core::{Bindings, Capability, Iri, Request};
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_root() -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ikigai-fs-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn source_with_path(ep: &FileEndpoint, path: &str) -> Result<Representation> {
        let req = Request::new(Verb::Source, Iri::parse("urn:file:default").unwrap());
        let mut bindings = Bindings::new();
        bindings.insert("path", path);
        let cap = Capability::root();
        let inv = Invocation::detached(&req, &bindings, &cap);
        block_on(ep.invoke(&inv))
    }

    #[test]
    fn reads_a_file_within_root() {
        let root = temp_root();
        std::fs::write(root.join("hello.txt"), b"hi there").unwrap();
        let ep = FileEndpoint::new(&root);
        let rep = source_with_path(&ep, "hello.txt").unwrap();
        assert_eq!(rep.repr_type.media_type, "text/plain");
        assert_eq!(rep.bytes, b"hi there");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_parent_dir_traversal() {
        let root = temp_root();
        let ep = FileEndpoint::new(&root);
        let err = source_with_path(&ep, "../escape").unwrap_err();
        assert!(matches!(err, Error::InvalidArgument { .. }));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_absolute_path() {
        let root = temp_root();
        let ep = FileEndpoint::new(&root);
        assert!(matches!(
            source_with_path(&ep, "/etc/hosts").unwrap_err(),
            Error::InvalidArgument { .. }
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_path_binding_is_an_error() {
        let ep = FileEndpoint::new(temp_root());
        let req = Request::new(Verb::Source, Iri::parse("urn:file:default").unwrap());
        let bindings = Bindings::new();
        let cap = Capability::root();
        let inv = Invocation::detached(&req, &bindings, &cap);
        assert!(matches!(
            block_on(ep.invoke(&inv)).unwrap_err(),
            Error::MissingArgument(_)
        ));
    }

    #[test]
    fn meta_returns_self_description() {
        let ep = FileEndpoint::new(temp_root());
        let req = Request::new(Verb::Meta, Iri::parse("urn:file:default").unwrap());
        let bindings = Bindings::new();
        let cap = Capability::root();
        let inv = Invocation::detached(&req, &bindings, &cap);
        let rep = block_on(ep.invoke(&inv)).unwrap();
        assert_eq!(rep.repr_type.media_type, "text/turtle");
        assert!(String::from_utf8(rep.bytes)
            .unwrap()
            .contains("ik:id \"file\""));
    }
}
