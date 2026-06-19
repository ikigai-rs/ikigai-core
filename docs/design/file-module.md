# Capability-scoped files & stores (`ikigai-fs`) — design

**Status:** settled 2026-06-18; **v1 implemented** 2026-06-19 — the standalone
`ikigai-fs` crate (SOURCE/SINK/EXISTS/DELETE, jail + path-ACL, strings-default,
native `std::fs` with a stubbed wasm `localStorage` backend) and the CLI wiring
(`sink` command, `read-only`/`read`/`write`/`delete`/`agent` cap profiles).
Companion to [`resolution-architecture.md`](resolution-architecture.md) and the
capability work (`ikigai-core 0.1.7`). This document is the rationale; the code is
the spec where they differ.

Files are the single most dangerous endpoint in the system — arbitrary
filesystem read *and write* — so the **security model is the point**, not an
afterthought. This module is also the first concrete consumer of three threads:
directory-scoped **capabilities** (#17), the **SINK/write** verb the REPL has
never had, and **transreption**'s first real `string ⇄ bytes` case (#28).

---

## Verbs

Core already has `Source` / `Sink` / `Exists` / `Delete` / `Meta`, with the
mutating verbs (`Sink`, `Delete`) correctly **uncacheable**. The module
implements `Source` (read), `Sink` (write/replace), `Exists`, `Delete`. Reads are
also uncacheable — a file is a *live fact* — which sidesteps stale-after-write
until the golden thread (#23) adds dependency-tracked invalidation.

Today's `ikigai-fs::FileEndpoint` is `Source`-only, jailed to a root, and — the
gap this closes — does **not** check the capability (its own doc notes the auth
layer was pending; it has landed).

---

## Security: two layers, both required every time

1. **The jail — structural, set at mount time.** `FileEndpoint::new(root)` is
   handed a directory and physically will not serve anything outside it: it
   rejects `..`, absolute paths, and symlink-escape *before any capability is
   consulted*. Fixed at mount, independent of who is asking — even `root`
   capability cannot escape it. (Already implemented.)
2. **The capability ACL — dynamic, per request.** The path-ACL the current
   capability grants (below).

**Access requires both: under the jail *and* allowed by the capability.** The
jail is the hard wall a capability bug can never punch through; the capability
scopes *within* it. The owner's local mount uses a generous root (home, or a
workspace dir); agents are narrowed by capability inside it. Reaching a different
tree means mounting a second endpoint — an operator decision, not a per-request
one.

---

## The capability path-ACL

A file capability is a set of rules `(verb, path, allow | deny)`, carried as
`urn:cap:` scopes:

- **Grant a directory** (default — everything under it, recursively):
  `urn:cap:fs:read:/Users/brian/workspace`
- **Limit to a few items** (allowlist) — grant only the items, *not* the parent:
  `urn:cap:fs:read:/Users/brian/workspace/public` + `…/shared`
- **Exclude a subtree or file** (denylist) — grant the parent *and* add a deny;
  a `-` before the path marks an exclusion:
  `urn:cap:fs:read:/Users/brian/workspace` + `urn:cap:fs:read:-/Users/brian/workspace/secret`

**Resolution: longest-prefix match.** For a `(verb, path)`, find every rule whose
directory contains the path, take the most specific (longest) one, and its
allow/deny decides. So `deny /workspace/secret` beats `allow /workspace` for
anything under `secret`, and an `allow /workspace/secret/shared` can re-open a
subtree inside a denied one. No matching rule → **default-deny**. Root → allow
all (within the jail). It's `.gitignore` / ACL semantics.

**Owner-minted rules (decision (a)).** The owner (`root`) mints an agent's rule
set directly — `root.attenuate([allow…, deny…])` yields exactly that set — and
the endpoint honors it. The flat-scope `Capability` is **untouched**; the fs
module does the path-aware matching, where path semantics belong. This covers the
delegation cases ("read Documents except `private`"). What it does *not* support
— an agent adding exclusions to a held capability *before re-delegating* — is the
**macaroon-caveat** evolution of attenuation (see *Future*).

---

## The endpoint declares its capability requirements

A mount is defined with a **policy**, not hardcoded behavior: which verbs it
honors and the capability namespace gating them — e.g. a *read-only* mount honors
only `Source` and refuses `Sink`/`Delete` regardless of capability. The policy
goes into the endpoint's **self-description**, so `describe` (and the dashboard,
#20) shows *what capability a resource demands* — the requirement is part of the
contract, not a surprise at call time.

---

## CLI capability profiles

Friendly `cap` labels alongside `freebusy`:

| profile | grants |
|---------|--------|
| `read-only` / `read` | `urn:cap:fs:read:<root>` |
| `write` | read + `urn:cap:fs:write:<root>` |
| `delete` | read + write + `urn:cap:fs:delete:<root>` |
| `agent` | `freebusy` + `read-only` — the "what I delegate" preset |

So `cap read-only` drops the session (or an agent) to read-only file access — the
same gesture as `cap freebusy`. (Scope form is **verb-first**:
`urn:cap:fs:<action>:<path>`, matching the path-ACL above.)

---

## Strings by default, transrept to bytes

`Source` hands you a **string** by default (decode the bytes as text — the common
case for config, notes, data); ask for `application/octet-stream` and you get the
raw **byte array** instead. `Sink` is the inverse: a string (encode) or raw bytes.
This is **transreption's (#28) first real case** — the `string ⇄ bytes`
transreptor, the smallest one there is.

- **Now:** endpoint-side content-negotiation — the endpoint inspects the requested
  type (`as` / Accept) and returns string-or-bytes itself. No new machinery.
- **Later:** lift it into a real `string ⇄ bytes` transreptor in #28's type graph,
  which the file endpoint (and everything else) reuses.

---

## One module, per-platform backend

Modeled on `ikigai-personal`'s platform seam — **one crate, a cfg-gated backend:**

- **native** (`cfg(not(target_family = "wasm"))`): `std::fs`, jailed (today's code).
- **wasm** (`cfg(target_family = "wasm")`): `web_sys` `localStorage` — the path
  maps to a namespaced key (`ikigai:fs:<root>:<path>`); `Source` = `getItem`,
  `Sink` = `setItem`, `Delete` = `removeItem`, `Exists` = key present.

Same `file:` / `urn:file:` contract, same capability scopes, same verb dispatch —
only the backend differs. So it **links in the browser too** (the `std::fs` code
is cfg'd out; `web_sys` *is* the "link in JavaScript" — it generates the
`localStorage` glue). Demo: write a note in the tab with `sink`, reload, read it
back with `source` — persisted in `localStorage`.

---

## CLI integration

- Make `ikigai-fs` a proper linked module (like `ikigai-fn` / `ikigai-personal`):
  `space(root)` + capability-gated endpoints behind a `file:{path}` grammar,
  mounted into the CLI's local kernel jailed to a workspace dir.
- Add a **`sink` engine command** — the write half the REPL has never had:
  `sink file:///notes.txt <content>`, or `source … | sink file:///notes.txt`.
- Mount **local / IPC-only** at first. A remote QUIC peer writing your files is
  the same auth-gated case as personal data over the wire (#36).

---

## Decisions

**Settled:** jail + capability (both, always); owner-minted rule sets (a);
longest-prefix path ACL with `-` exclusions; strings-by-default with
transrept-to-bytes; one cfg-gated crate, native fs + wasm localStorage.

**Recommendations on the small knobs** (override freely):
- **Crate home** — extract `ikigai-fs` to a standalone sibling crate, consistent
  with `ikigai-fn` / `ikigai-personal` (it currently lives in the core workspace).
- **v1 verbs** — `Source` + `Sink` first; `Delete` + `Exists` as fast-follows.
- **Deny syntax** — a leading `-` on the path inside the scope token.

---

## Future / connected threads

- **Macaroon caveats — the next shape of attenuation (#17).** `Capability` grows
  *grants that intersect* **plus** *caveats that accumulate* on `attenuate`
  (denials, path-bounds, time-bounds) — both narrowing, composably, at any
  delegation depth. Files surface it first, but it's also what QUIC
  crypto-attenuation (#36) and time-bounded grants will want. Decision (b),
  deferred.
- **Golden thread (#23).** `Sink` invalidates the cached `Source` of that file;
  until then reads are uncacheable to stay correct.
- **Transreption (#28).** The `string ⇄ bytes` transreptor; the endpoint-side
  version lifts into the type graph.
- **Remote (#36).** Files over QUIC need remote auth + crypto-attenuation before
  exposure.
- **Typed-argument pull.** `toUpper file:///x.txt` pulls a file reference *through*
  the capability check — the file module is what makes that real and safe.
