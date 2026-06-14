/// An unforgeable handle conferring authority to resolve and invoke resources.
///
/// ikigai uses no ambient authority: an endpoint receives the capabilities it
/// needs explicitly through its request context, never reaching for globals.
///
/// In M0 this is the *shape* only — a real minting authority (with attenuation,
/// delegation, and per-verb grants) arrives with the authorization layer.
/// [`Capability::root`] is a development-only, all-permitting stub. The handle
/// cannot be constructed from arbitrary data outside this crate, which is what
/// makes it unforgeable.
#[derive(Clone, Debug)]
pub struct Capability {
    kind: Kind,
}

#[derive(Clone, Debug)]
enum Kind {
    /// All-permitting development capability (permissive stub).
    Root,
}

impl Capability {
    /// A development-only capability that permits everything.
    ///
    /// Placeholder until the authorization layer introduces a real minting
    /// authority; do not rely on it for access control.
    pub fn root() -> Self {
        Capability { kind: Kind::Root }
    }

    /// Whether this is the development root capability.
    pub fn is_root(&self) -> bool {
        matches!(self.kind, Kind::Root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_is_root() {
        assert!(Capability::root().is_root());
    }
}
