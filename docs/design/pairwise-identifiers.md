# Pairwise identifiers for federation

**Status:** proposed, design spike (ledger #4513). No API or behavior change. Written on
2026-11-27 against core main
[`b09fcea`](https://github.com/ikigai-rs/ikigai-core/tree/b09fcea6c07568e73bb566cef285db7535d815a0),
read against how gonk's QUIC clients and ikigai-layers' participants are described in the
ledger item: each is known by one certificate fingerprint everywhere it goes. Neither of those
repositories' source was read for this note. Evidence:
[`tests/pairwise_identifiers.rs`](../../crates/ikigai-core/tests/pairwise_identifiers.rs), eight
tests from public API alone. The derivation in that file is a test-only sketch, not an agreement
to build it.

**Companions:** [Principals as capability scopes](principal-capability.md) (how a door names a
party, and why `clamp` keeps the ceiling's principal), [Sub-request
authority](sub-request-authority.md) ("Across the wire": the session capability is minted per
connection from the peer's certificate fingerprint), [Cache ejection](cache-ejection.md) §2 (a
capability fingerprint is not an identity).

## The question

A policy paper on accountable agents asks that an agent show each counterparty a stable
identifier of its own (a *pairwise pseudonymous* identifier), so services that compare notes
cannot link it. Accountability is kept because the agent's operator can still say who it was.

For ikigai, the question has four parts:

- How would a participant (a gonk client, a layer participant, a worker machine) show a
  different, stable identifier to each peer?
- What still links those identifiers for the participant's own operator?
- What does that break or simplify, such as grants keyed by fingerprint, revocation and audit
  lines?
- Is it worth doing now?

## What was reproduced

Today one party has one name at every peer. Three tests pin this, and a fourth marks where the
third stops holding:

- `reproduction_two_peers_see_the_same_principal_for_one_client`: two kernels mint a
  principal from the same certificate fingerprint, as gonk does (`urn:iki:gonk:client:<hex>`; the tests spell it under `urn:example:`, since `tests/namespaces.rs` admits only published `urn:iki:` namespaces).
  Both hand their endpoints the same string and partition their caches by it. Every record
  that holds a principal holds that one string: the log's `principal=` column, the trace,
  held content (`put_for`) and an author gate. Two operators who compare those records can
  join on it with no further work.
- `reproduction_a_carried_capability_discloses_the_upstream_principal_on_the_wire`: this
  leak is separate from certificates, and it is new to this note. A sub-request to a mounted
  remote carries its capability in serialized form (`Call::IssueAs`). If that capability
  names the upstream's caller (say, a passkey principal), the remote reads the name. A remote
  whose ceiling for this peer is SCOPED (a QUIC session minted from the peer's certificate)
  then drops it at `clamp` and keeps the principal its own channel authenticated. So the
  remote learns who the upstream was serving, and the name gives the caller no authority there.
- `reproduction_the_carried_name_is_disclosure_only_shedding_it_changes_no_clamp`: under a
  scoped ceiling, the clamp's result is the same whether or not the carried capability names a
  principal. Shedding the name before sending it changes no authority at that remote.
- `under_a_root_ceiling_the_carried_name_is_the_attribution_and_shedding_it_loses_it`: the
  limit of the previous test. Under a ROOT ceiling, `clamp` returns the carried capability,
  principal included (`root.clamp(c) = c`; the doc on `Capability::clamp` and
  [principal-capability.md](principal-capability.md): root may act as anyone). IPC is the same
  case by another route: it does not clamp, and resolves under the carried capability because
  the peer is the peercred-verified owner ([sub-request-authority.md](sub-request-authority.md)).
  There the carried name is what the remote's endpoints see as the caller. Shed it, and the
  request arrives anonymous: `principal()` is `None`, `acts_as` is false for the caller, and a
  write is logged `principal=-` and stored unattributed. That is ledger #879's failure,
  reintroduced from the sending side.

So the claim holds. It also has a second part that is cheaper to fix than the first, at
remotes with a scoped ceiling (§5).

## The threat this addresses, and the one it does not

**It addresses** a join on the identifier. Two services, run by different operators, compare
their audit lines, grant tables or cache exports. With pairwise names, they find no string in
common.

**It does not address** a join on the traffic. A participant that reaches both peers from the
same address, at correlated times, with the same software fingerprint, can still be linked by
anyone who compares connection metadata. A pairwise identifier is necessary for
unlinkability, and nowhere near enough. On-path observers are not the concern: a client
certificate in TLS 1.3, and so in QUIC, travels encrypted, so only the peer it authenticates to
sees it.

## The design

### 1. The name must change where the channel is authenticated

A peer knows a participant by whatever authenticates the channel: here, the client
certificate. A pairwise *principal* minted over one shared certificate hides nothing, because
the peer's own door does the minting and the peer sees the certificate. So per-peer identity
means a **per-peer key pair and certificate**. The principal a peer mints from that
certificate's fingerprint is then per-peer by construction. Core changes nothing:
`with_principal` takes any absolute IRI up to `MAX_PRINCIPAL_LEN`, and
`the_kernel_takes_a_pairwise_name_unchanged` pins that.

### 2. Derived, not stored: one root secret, one key per peer

There are two ways to get a key per peer:

| | Derived | Stored |
|---|---|---|
| How | per-peer seed = keyed hash of the peer's identity, under a key derived from the participant's root secret (the test uses BLAKE3 `derive_key` and `new_keyed` with a versioned context; HKDF-SHA-256 is the same shape); the seed becomes the private key | a fresh random key pair per peer, kept in the participant's key store |
| State | one secret; any peer's key can be rebuilt | one entry per peer, backed up together |
| A leaked secret | exposes every peer's key | exposes the leaked entries only |
| Rotation | change the context version for one peer, or the root secret for all | replace one entry |

**Decision: derived, with a per-peer version in the derivation's context.** The operator's
link (below) then costs nothing to keep, and one peer can be rotated without the others. The
root secret must live where the participant's private key lives today, and no more widely.

The derivation must be **stable** (one peer always gets one name) and **separated** (two peer
identities never share an input). The prototype prefixes the peer identity's length, which
`a_pairwise_name_is_stable_per_peer_and_distinct_across_peers` pins.

### 3. What "the peer's identity" is: a sector, chosen by configuration

The input to the derivation decides who can link whom. It must be:

- **Stable**: not the peer's own certificate fingerprint. When the peer rotated its
  certificate, the participant would get a new name there, and every grant the peer holds for
  it would be orphaned.
- **Chosen**: one input for services one operator runs together (a ledger and its layer
  host, say). This is the *sector identifier* idea from OpenID Connect pairwise subjects.
  Services that may already link the participant share one name, and services that may not
  get separate names.

**Decision: the peer identity is a configured IRI per federation entry** (for example
`urn:example:host:name:<name>`; the real namespace is chosen with task 3), never derived from what the peer presents. A peer the participant
has not configured gets no automatic name. Enrollment is deliberate.

### 4. What still links them, for whom

- **The participant's operator** holds the root secret and the list of configured peers, so it
  can rebuild the full table, or invert one name an audit line shows
  (`the_operator_links_the_names_and_a_peer_cannot`). Accountability works by disclosure: when
  an investigation needs the link, the operator provides it.
- **A peer** that knows the scheme, both sector IRIs and its own name, but not the root secret,
  cannot compute the participant's name at another peer.
- **Nobody else.** The participant's own audit lines should record `(peer sector, name shown)`
  so the operator does not have to rederive names to read its own logs.

### 5. Carried principals: shed at the mount, only toward a scoped ceiling

This part is independent of certificates, and what it costs depends on the ceiling the remote
applies to this peer:

| The remote's ceiling for this peer | What the remote does with the carried capability | What shedding its principal does |
|---|---|---|
| Scoped (a QUIC session minted from the peer's certificate) | `ceiling.clamp(&carried)`: keeps the ceiling's principal, drops the carried one | Nothing to authority or attribution: the clamp is identical (third reproduction test). It closes the disclosure. |
| Root (a QUIC session granted root) | `root.clamp(&carried)` is the carried capability, principal included | The request becomes anonymous there: no `principal()`, no `acts_as`, writes unattributed (ledger #879). |
| None, by design (IPC to the peercred-verified owner) | Resolves under the carried capability as it is | The same as root. |

So a mount client sending a carried capability to a remote **sheds the principal scope only
when the remote applies a scoped ceiling**: the narrowing that `attenuate` already does when
the scope is not named. A remote with a scoped ceiling discards the name at `clamp`, so
nothing is lost there. A remote with a root ceiling, or an IPC peer, trusts the sender
completely and attributes by the carried name, so the name is kept, and attribution is kept
with it. That is also where the disclosure matters least: a peer given root, or the owner over
IPC, is in the participant's own trust domain, not another operator's.

The sender cannot read the remote's ceiling, so the mount says which case it is in, by
configuration: shedding is an opt-in per mount entry, off by default, and refused for an IPC
mount. A mount to another sector (§3) is the case to turn it on for. Misconfigured toward a
root-ceiling remote, it would bring ledger #879 back silently, so the task's test pins both
sides.

If a remote ever needs to know whom the upstream serves, a pairwise name for that remote can be
carried as a door fact ([door-context.md](door-context.md)), not as authority.

Core needs nothing new for this: `Capability::attenuate` over the held grants already sheds
the name (the third reproduction test), and the fourth shows what it does under a root
ceiling. The change belongs where `Call::IssueAs` is built.

## What it breaks, and what it simplifies

| Area | Today | With pairwise identifiers |
|---|---|---|
| Grants keyed by fingerprint | One fingerprint can sit in a shared allowlist that several services read | Each peer enrolls the fingerprint it is shown, and a shared allowlist across sectors stops working, by design. Within one sector nothing changes. |
| Enrollment | Show one certificate once, anywhere | Show each peer its own certificate. That is one step per peer, which the operator was already doing at each peer. |
| Revocation by the peer | Revoke one fingerprint | Unchanged: the peer revokes the one name it knows |
| Revocation across services | One central list entry revokes everywhere | The operator revokes N names at N peers, or retires the root secret. No peer can do it for the others, which is the point, and it is the main operational cost. |
| Compromise | One key exposed | A stolen root secret exposes every peer's key. A stolen per-peer key exposes one peer. |
| Audit lines | `principal=urn:iki:gonk:client:<fp>`, the same at every service | Each service logs its own name. Joining across services needs the operator's table. The 512-byte principal bound is not at risk: the sketch's IRI, `urn:example:pairwise:digest:<64 hex>`, is 92 bytes. Its namespace is illustrative; choosing a real one is part of task 4. |
| Cache | One partition per principal per peer | The same: one name per peer, one partition per peer (`two_peers_see_two_names_and_each_peer_keeps_one_partition`) |
| Held content (`put_for`) | Scoped to the principal at that peer | The same: it was already per peer |
| Chains (A to B to C) | B shows C its one fingerprint, and may carry A's principal along | B shows C its name for C. A's name is shed at B's mount when C's ceiling for B is scoped, and kept when it is root (§5). |
| Worker machines | One fingerprint per machine, known to every service it reaches | One name per service. The operator tells its machines apart through its own table. |

What gets simpler: once grants are known to be per-peer, no code may treat a principal IRI as
globally meaningful. [cache-ejection.md](cache-ejection.md) §2 already asks for that discipline
for capability fingerprints.

## Is it worth doing now?

**Not the certificate part. Yes to shedding carried principals toward a scoped ceiling.**

For per-peer certificates, three things argue against doing it now:

- **Who compares notes today.** As the item describes it, ikigai's federation is a small set
  of participants whose peers are mostly run by the same operator, who could link them anyway.
- **The traffic join.** It still links a participant through addresses and timing (above).
- **The cost.** Per-peer enrollment, revocation fan-out and a new secret to protect fall on
  every participant.

These would change the answer:

- a peer run by an operator the participant does not trust with its other relationships (an
  open marketplace of layers, a third-party ledger);
- a participant that also hides its network path;
- a policy or regulatory requirement for unlinkable agent identifiers.

Shedding carried principals toward remotes with a scoped ceiling is cheap and makes no one's
authority smaller there, and the leak it closes exists today. Toward a root ceiling it would
cost attribution, so it is not done there (§5). Keeping principal IRIs opaque and per-peer costs nothing now, and it keeps
the certificate step open for later.

### Tasks

Do these now. Each is a change in the repository named, not in core:

1. **Shed the principal from a carried capability at a mount whose remote applies a scoped
   ceiling.** ikigai-quic's client, where `Call::IssueAs` is built: on a mount configured to
   shed (opt-in per mount entry, off by default, refused for IPC), send
   `carried.attenuate(<carried grants without the principal scope>)`. Add tests that, with
   shedding on, the serialized frame names no principal and a scoped-ceiling remote's clamped
   capability is unchanged; and that, with it off, a root-ceiling remote still attributes the
   request to the carried principal.
2. **Keep principals opaque in shared configuration.** Across gonk and ikigai-layers, do not
   introduce an allowlist, registry or grant table that is keyed by one client fingerprint
   and read by services in different sectors. A comment at each existing grant table is
   enough.
3. **Name the sector.** In the participant's federation configuration (gonk client, layer
   participant), give each configured peer a stable sector IRI (namespace to be chosen there), even though
   nothing derives from it yet. This is the input a later derivation needs, and it gives audit
   lines a peer name that does not change when the peer's certificate rotates.

Do these later, only if one of the triggers above holds:

4. A per-peer key derivation in the participant's client (gonk and ikigai-layers), keyed by
   the sector IRI and a per-peer version, with the operator table written to the participant's
   own audit log as `(sector, name shown)`.
5. Per-peer enrollment and a revocation command that walks the operator's table.
6. A note in [principal-capability.md](principal-capability.md) that one party may have
   several principals across peers, and that `acts_as` is per peer.

## Upgrade from older versions

Nothing changes. This note and its tests add no API, alter no persisted record and emit
nothing new. If task 1 is done later, it is off by default, so an existing mount configuration
behaves as before. A mount that turns it on stops sending the carried principal: a
scoped-ceiling remote that logged it stops seeing it, and by the clamp rule it never acted on
it. A root-ceiling or IPC remote does act on it, which is why shedding stays off for those.
