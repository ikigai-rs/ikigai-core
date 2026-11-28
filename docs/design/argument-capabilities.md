# Capabilities an argument adds

Status: design proposal, not implemented (ledger #4549). Nothing in the kernel changes by
adding this note. The recommendation and the task list at the end are proposals until accepted.

## The gap

An action declares its authority as a whole. `ActionSpec.requires`
([`describe.rs:1091`](../../crates/ikigai-core/src/describe.rs)) and the flat
`Description::requires` ([`describe.rs:1858`](../../crates/ikigai-core/src/describe.rs)) are the
only places a capability is declared. `ArgSpec`
([`describe.rs:38`](../../crates/ikigai-core/src/describe.rs)) has no capability field, and its
builders (`impl ArgSpec`, [`describe.rs:596`](../../crates/ikigai-core/src/describe.rs)) have no
`requires`. `ValueNote` ([`describe.rs:323`](../../crates/ikigai-core/src/describe.rs)) can say
that a value needs another input, but not that it needs authority.

So an argument that needs more authority than its action can only be enforced inside
`invoke`. Take `urn:email:send` (cli PR 778, ledger #3027). It declares `urn:cap:email:send`; its
`attach` argument also needs `urn:cap:email:attach`, which the endpoint checks at run time.
Three things the kernel derives from the declaration then disagree with what the endpoint does:

- **The manifold over-offers.** `urn:kernel:actions` offers the action with `attach` among its
  inputs to every holder of `urn:cap:email:send`. A caller that uses `attach` without
  `urn:cap:email:attach` is Denied. "Declared = enforced" fails in the over-offer direction.
- **The pre-flight conforms falsely.** `urn:kernel:validate` checks only `spec.requires`
  (`validation_report`, [`kernel.rs:7487`](../../crates/ikigai-core/src/kernel.rs)), so it reports
  `sh:conforms true` for a call that will be Denied.
- **The denial carries nothing structured.** The kernel's floor refusal carries the scopes it
  lacked (`Error::lacking`, ledger #3840). An endpoint's own `Error::Denied("…")` carries an
  empty `lacking` list unless the author remembers `Denial::lacking`, so a host cannot tell the
  caller which grant to ask for.

The workaround is to split the action (send, send-with-attachments), which multiplies actions
for every optional authority. It is compared below.

## Proposal

### Declaring it

Add one field and one builder to `ArgSpec`, the same shape as `ActionSpec.requires`:

```rust
/// The capability scopes this input ADDS to its action's `requires` when it is given.
/// Enforced by the kernel's floor, before dispatch, exactly as the action's own are.
/// Omitted from serialized output when empty, so existing JSON contracts are unchanged.
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub requires: Vec<String>,

/// Declare a capability scope this input adds when it is given (builder). Callable
/// multiple times.
pub fn requires(self, capability: impl Into<String>) -> Self;
```

The email endpoint then reads:

```rust
ActionSpec::new(Verb::Sink)
    .requires("urn:cap:email:send")
    .input(ArgSpec::new("to", "recipient").required())
    .input(ArgSpec::new("attach", "a file to attach").requires("urn:cap:email:attach"))
```

Rules, each checked where the description is first read (`Floor::of`), and refused there as a
malformed description rather than discovered at the first call:

- **Additive, never subtractive.** An input's scopes are added to the action's. Nothing an input
  says can remove a scope the action requires. There is no "this input needs LESS".
- **"Given" means present in the request's arguments** after a named request's `fixed` values are
  applied, and before the kernel injects declared defaults. An input with both a `default` and a
  `requires` is refused: the default makes it given on every call, so its authority would be the
  action's, and the author should say so on the action.
- **A `required` input with `requires` is refused** for the same reason: it is always given.
- **A `list` input** is given once however many items it carries. Its scopes are checked once.
- **Per value, second.** `ValueNote` gains `requires` in a second step, with the same rules,
  "given" meaning "this value is among the input's items". The calendar's `as=text/turtle` in the
  survey below needs it. It is a second enforcement path to test, so it follows the input-level
  form rather than landing with it.
- **Family scopes work unchanged.** An input may declare `urn:cap:fs:read:*`. The floor reads it
  through `cap_satisfies` like any declared scope, a presence test, and the module's own rule still
  decides which path.

`ArgSpecWire` carries the field. In Turtle the input node carries `ik:requires`, the property the
action node already uses. Its vocabulary comment gains "on an input node, a scope added when that
input is given". `ik:requires` has no domain, so no new property is needed.

### How `action_specs()` normalizes it

`action_specs()` ([`describe.rs:2010`](../../crates/ikigai-core/src/describe.rs)) keeps the
input's scopes ON THE INPUT and does not fold them into `ActionSpec.requires`. Folding would make
`attach` mandatory authority for every send, which is the opposite of the point. Both authoring
forms behave the same way. A flat `Description::input(ArgSpec…requires(…))` synthesizes an action
whose input still carries them, and an explicit action takes its inputs whole, as it does today.

Two derived views change:

- `Description::required_scopes(verb)` stays the floor for a call that gives no
  argument-gated input. A new `Description::required_scopes_for(verb, given: &[&str])` adds the
  scopes of each given input, deduplicated, in declaration order. `unsatisfied_scopes` gains the
  same `given` form, so a module's self-check keeps one predicate.
- **The undeclared-verb rows of the floor table include input scopes.** Today an undeclared
  mutating verb requires "every scope declared anywhere" (`Floor::requirements`,
  [`kernel.rs:1425`](../../crates/ikigai-core/src/kernel.rs)). Input scopes join `every`, so a
  verb nobody described never needs less than one somebody did.

### Enforcement: the floor

`Floor::unsatisfied` ([`kernel.rs:1447`](../../crates/ikigai-core/src/kernel.rs)) already
receives the `Request`, whose `args` say which inputs are given. It adds, for each given input
with `requires`, the scopes the capability does not satisfy. The check runs where the action's
check runs: after resolution, before the cache lookup and before dispatch. The `Admission` it
produces is unchanged in shape. `lacking` simply holds more scopes. The `FloorMemo` keeps the
per-verb table of gated inputs beside the defaults it already keeps, so the memo still costs one
read per resolution.

The refusal is the kernel's existing floor sentence (`denial_message`,
[`kernel.rs:4100`](../../crates/ikigai-core/src/kernel.rs)), with the input named, because a
caller who reads only "declared by `urn:email:send`" would look for the scope on the action:

> denied: capability does not grant `urn:cap:email:attach` (declared by `urn:email:send` for the
> input `attach`)

The family form keeps its tail ("; any grant under `…` satisfies it"). The action-level sentence
is unchanged byte for byte, because other repositories' tests match it. `Error::lacking` holds
every scope missing, action scopes first and then input scopes, in declaration order.

**Existence-sensitive actions.** `Floor::conceals` decides from the action's scopes only. A caller
refused for an input's scope already passed the action's floor and can see the action, so the
refusal is Denied, never concealed. Concealing it would tell such a caller that an action it was
just offered does not exist.

### What the manifold shows

A caller holds the action's scopes but not an input's. `urn:kernel:actions` offers the action, as
it does today, and **shows the input as unavailable, naming the scope it lacks**. It does not hide
the input:

```turtle
<urn:ikigai:match:sink:urn:email:send> a ik:ActionMatch ;
    ik:endpoint <urn:email:send> ;
    ik:contract <urn:ikigai:contract:email-send:sink:b3:…> ;
    ik:verb "Sink" ;
    ik:requires <urn:cap:email:send> ;
    ik:unavailableInput <urn:ikigai:contract:email-send:sink:b3:…:input:attach> .

<urn:ikigai:contract:email-send:sink:b3:…:input:attach>
    ik:lacks <urn:cap:email:attach> .
```

`ActionMatch` gains `unavailable: Vec<(String, Vec<String>)>`: input name, scopes lacked. A caller
holding everything sees no `ik:unavailableInput`, so its manifold is byte-identical to today's.

Why shown rather than hidden:

- **The contract IRI must not depend on the caller.** The manifold joins each match to its
  content-addressed contract by `ik:contract`. Hiding the input would mean either a different
  contract per capability, which breaks the join and the catalog's one-node-per-contract rule, or
  a match that disagrees with its own contract. Saying "unavailable" on the match, which is already
  per caller, keeps the contract one thing.
- **It tells the caller what to ask for.** An agent or a person with `send` but not `attach` can
  send now and request `urn:cap:email:attach` later. A hidden input looks like a feature that
  does not exist.
- **It is not a disclosure.** The full contract, `attach` included, is in the catalog and in the
  endpoint's `Meta`. Any caller the action's floor admits can already read it.

A required input is never gated (see the rules above), so an unavailable input never makes an
action uninvokable. Ranking (`missing_optional`) is unchanged: unavailability is not a mismatch of
types.

The text and JSON faces of hosts that render the manifold (a REPL listing, an MCP tool schema)
should show the input with the words "needs `urn:cap:email:attach`", not with color or a struck
style alone. That is a host's rule (see [ACCESSIBILITY.md](../../ACCESSIBILITY.md)), recorded here
so the first host to render it follows it. An MCP door may instead drop the input from the tool's
input schema, since a JSON Schema has no "unavailable" property. It then describes the input in
the tool's description in words, so the information is not lost.

### What `urn:kernel:validate` refuses

`validation_report` gains one loop after the action's scopes: for each input the proposed `args`
give that declares `requires`, each scope the capability does not satisfy is a violation. Unlike
the action's violation, it has a path, the input, because it is about that input. Its sentence:

> the capability does not grant `urn:cap:email:attach`, which the input `attach` requires

with `sh:resultPath` the input's node, and no SHACL constraint component, like the action's
capability violation, since it is about authority rather than shape. The same proposed `args`
without `attach` conform. So validate and the floor agree call by call, and the existing test
`validate_conforms_iff_the_kernel_operation_accepts_the_call`
([`kernel.rs:9203`](../../crates/ikigai-core/src/kernel.rs)) extends to gated inputs without a
new rule.

### Cache fingerprint and attenuation

**Cache.** Nothing new is keyed. A request that gives `attach` and one that does not are
different request identities already, since arguments are part of the identity. Each entry is
keyed by the capability fingerprint it was computed under (`kernel.rs:775`). The floor runs
before every lookup, hits included. So a holder without `attach` authority is refused before any
entry is consulted, and cannot be served one a stronger holder computed. The proposal for
capability-independent answers ([capability-independent-cache.md](capability-independent-cache.md))
requires admission before every shared lookup. Input scopes are part of that admission by
construction, because they are checked in the same `Floor::unsatisfied`. A cached manifold stays
keyed by the capability it was computed for, so a manifold showing `attach` unavailable is never
served to a holder for whom it is available, and the reverse.

**Attenuation never widens.** An input's `requires` is a demand checked against the capability a
request carries. It grants nothing:

- A sub-request issued by an endpoint is checked against that sub-request's own, attenuated
  capability (`Capability::attenuate`,
  [`capability.rs:387`](../../crates/ikigai-core/src/capability.rs)). An endpoint holding
  `urn:cap:email:send` that forwards `attach` to a mailer it calls is refused unless its own
  authority carries `urn:cap:email:attach`. Passing an argument along never passes authority
  along.
- A named request that `fixed`s a gated input to a value makes the input given on every call of
  that name. The caller of the name needs the input's scopes. The name cannot stand in for them.
- The family form remains a presence test, and the module's finer rule still runs inside
  `invoke`. A gated input with `urn:cap:fs:read:*` admits any holder of a read grant, and the
  module decides which path, exactly as for an action scope.
- An exclusion named in an input's `requires` is satisfiable only by root, as for an action
  (`Capability::allows`).

## Endpoints that would use it today

Capabilities enforced at run time, conditional on an argument, and not declared. A survey of the
ikigai-rs repositories at their default branches on 2026-11-27, by reading the code:

The survey read shallow clones of every repository with a code-search hit for a `urn:cap:`
literal or a run-time capability check. Each row was confirmed by reading the code. Two
of the rows exist only on open pull requests, so their links pin those heads.

| Endpoint and action | Trigger | Scope checked at run time | Declared | Where |
|---|---|---|---|---|
| ikigai-cli `crates/ikigai-email`, `urn:email:send` Sink | `attach` given | `urn:cap:email:attach` | only `urn:cap:email:send` ([L588](https://github.com/ikigai-rs/ikigai-cli/blob/f35bf3f31270ef40643ce0f3df89b87d4abb96f5/crates/ikigai-email/src/lib.rs#L588)) | [`lib.rs:305`](https://github.com/ikigai-rs/ikigai-cli/blob/f35bf3f31270ef40643ce0f3df89b87d4abb96f5/crates/ikigai-email/src/lib.rs#L305), open PR ikigai-cli#778 (ledger #3027) |
| ikigai-personal, `urn:personal:calendar` Source | `q` given (a title search is a title read) | `urn:cap:personal:calendar:read:detail` | only `…:calendar:read:freebusy` ([L666](https://github.com/ikigai-rs/ikigai-personal/blob/6e9cd110f40b1290c95b7a226e2f710839d4a24a/src/lib.rs#L666)) | [`lib.rs:546`](https://github.com/ikigai-rs/ikigai-personal/blob/6e9cd110f40b1290c95b7a226e2f710839d4a24a/src/lib.rs#L546) |
| ikigai-personal, `urn:personal:calendar` Source | `as` contains `turtle` (the Turtle face carries titles) | `urn:cap:personal:calendar:read:detail` | as above | [`lib.rs:537`](https://github.com/ikigai-rs/ikigai-personal/blob/6e9cd110f40b1290c95b7a226e2f710839d4a24a/src/lib.rs#L537) |
| ikigai-meeting, the Zoom actions | `teacher` names a teacher whose record is the teacher's own OAuth app (`TeacherRecord::Own`), so the rotated refresh token is written back | `urn:cap:secret:write:<record>` (`gate_secret`, [L1550](https://github.com/ikigai-rs/ikigai-meeting/blob/c49f6a7fca9b2b06e6ad5e98e396d0d7d6072e7c/src/lib.rs#L1550)) | only `CAP_NET` and `CAP_SECRET_READ` ([L683](https://github.com/ikigai-rs/ikigai-meeting/blob/c49f6a7fca9b2b06e6ad5e98e396d0d7d6072e7c/src/lib.rs#L683)) | [`lib.rs:968`](https://github.com/ikigai-rs/ikigai-meeting/blob/c49f6a7fca9b2b06e6ad5e98e396d0d7d6072e7c/src/lib.rs#L968), open PR ikigai-meeting#30 |

The four rows fit the proposal in three different ways:

- **email `attach` and calendar `q`** are gated on presence alone. Input-level `requires` declares
  them exactly.
- **calendar `as=…turtle`** is gated on a VALUE. Input-level `requires` cannot express it without
  gating every `as`. It needs value-level `requires` (`ValueNote`), or a face-level declaration if
  `as` is the kernel's negotiation argument rather than a declared input. This is why value-level
  is the second step below, not "later, if ever".
- **meeting `teacher`** is gated on stored DATA: which teacher's record the value names, and that
  record's kind. No static declaration can say exactly when it applies. The most a declaration
  can do is gate the input with the family `urn:cap:secret:write:*`. That presence test would
  over-refuse callers who name a shared-app teacher, so the endpoint should keep its run-time check
  and declare nothing it cannot enforce exactly. It is a case for the endpoint's own Denied carrying
  `Denial::lacking`, not for this proposal.

Found beside them, but not argument-gated and not this proposal's to fix:

- `urn:personal:calendar` Source accepts EITHER `read:detail` or `read:freebusy`
  ([`lib.rs:516`](https://github.com/ikigai-rs/ikigai-personal/blob/6e9cd110f40b1290c95b7a226e2f710839d4a24a/src/lib.rs#L516)
  to L528). `requires` is a conjunction, so it declares only `freebusy`, and the kernel refuses a
  holder of `detail` alone before the endpoint runs. The code calls this "a known gap" at L660.
  Disjunctive requirements are a separate design.
- `urn:personal:calendars` Source enforces `…:calendar:read:detail` on every call
  ([`lib.rs:1272`](https://github.com/ikigai-rs/ikigai-personal/blob/6e9cd110f40b1290c95b7a226e2f710839d4a24a/src/lib.rs#L1272))
  and declares nothing on its Source action (L1313 to L1316). That is a missing action-level
  declaration, fixable today.
- ikigai-meeting's `may_host`
  ([L1543](https://github.com/ikigai-rs/ikigai-meeting/blob/c49f6a7fca9b2b06e6ad5e98e396d0d7d6072e7c/src/lib.rs#L1543))
  omits the host link unless the caller holds `urn:cap:meeting:host:teacher:{t}`. It is a
  projection of the answer, not a refusal, so it is not over-offered.

Checked and set aside as correctly declared: ikigai-log (`segments.rs` L1761, a family declared
at L1812), ikigai-repo (`lib.rs` L388, a fixed tool per action, each declared), ikigai-ledger
(`grant_for` in `endpoints.rs` L341, a narrow or broader declared grant), and second checks of
declared scopes in ikigai-gonk `backup.rs` L1063, ikigai-credential `resource.rs` L632 and
ikigai-ledger `alias.rs` L473. Not yet read, and the likeliest places for more rows: the
run-time scope builders in ikigai-gonk (`web.rs`, `history.rs`, `github.rs`, `mount.rs`,
`findings_summary.rs`, `layers_view.rs`), ikigai-ledger (`endpoints/across.rs` L470 and several
in `endpoints.rs`) and ikigai-personal `remote.rs` L481.

No repository was found to use the split-action workaround for a conditional scope. The closest
is ikigai-personal adding declared scopes only when configuration enables remote reads
(`REMOTE_READ_REQUIRES`, `lib.rs` L1172), which is a declaration that varies by configuration,
not a split.

## The alternative: split actions

Split `urn:email:send` into `urn:email:send` (no `attach`) and `urn:email:send-with-attachments`
(requires both scopes). This works today and needs no kernel change.

| | Split actions | Input `requires` |
|---|---|---|
| Kernel change | none | a field, a floor loop, a validate loop, a manifold term |
| Declared = enforced | yes, each action declares its whole authority | yes, the floor enforces what the input declares |
| Manifold | a holder without `attach` sees only plain send; nothing says attachments exist or what grant they need | one action; `attach` shown unavailable with the scope named |
| Pre-flight | exact | exact |
| Number of actions | doubles per optional authority: two gated inputs need four actions, or a choice of which combinations to publish | one |
| Contracts and caching | two contracts, two cache partitions, two documentation entries; shared inputs repeated | one contract |
| Callers | must choose the action from the arguments they intend to give; an agent that adds `attach` mid-plan has to switch targets | unchanged |
| Compatibility | the existing action changes meaning (loses `attach`) or a third action appears | the action is unchanged for holders of both scopes; its contract IRI changes once, because the input node gains a triple |
| Composes with `needs`, groups, `list` | duplicated per split | unchanged |

Split actions are honest, and right when the gated form is a different act: different outputs,
different failure classes, a different resend rule. They are the wrong tool when one optional
input needs more authority and nothing else changes, which is the case for email `attach` and
calendar `q` in the survey. For the meeting's data-gated `teacher`, neither tool is exact: a split
would have to be chosen per teacher record, and the run-time check stays. They also
leave one gap the input form closes: a caller who lacks the second scope never learns the gated
form exists.

## Recommendation

Add `ArgSpec::requires`, additive and input-level, enforced by the kernel's floor, shown in the
manifold as unavailable with the missing scope named, and refused by `urn:kernel:validate` at the
input's path. Follow with value-level `requires` on `ValueNote`, which the calendar's `as` needs.
Keep split actions for gated forms that are genuinely different acts. Keep run-time checks, with
`Denial::lacking`, for gates that depend on stored data, which no declaration states exactly.

## Upgrade

Additive for authors. A description with no input `requires` serializes, digests and enforces
exactly as before, and its contract IRI is unchanged. An endpoint that adopts it should keep its
run-time check for one release. The check becomes unreachable once the kernel enforces the
declaration, and it costs nothing, while an older kernel that ignores the field would otherwise
admit the call. Its contract IRI changes once, because the input node gains `ik:requires`. A host
that pins contract IRIs re-reads them, as for any contract change.

## Task list

1. `ikigai-core`: `ArgSpec.requires` and the builder, `ArgSpecWire`, Turtle `ik:requires` on the
   input node, contract digest; refuse `requires` with `default` or `required` at `Floor::of`.
2. `ikigai-core`: `Floor::unsatisfied` adds given inputs' scopes; `denial_message` names the input;
   `Error::lacking` carries them; undeclared verbs include them in `every`.
3. `ikigai-core`: `urn:kernel:validate` violation at the input's path, with the sentence above;
   extend `validate_conforms_iff_the_kernel_operation_accepts_the_call`.
4. `ikigai-core`: `ActionMatch.unavailable`, `ik:unavailableInput` and `ik:lacks` in the manifold,
   and the vocabulary terms with labels and comments.
5. `ikigai-core`: `required_scopes_for` and `unsatisfied_scopes` with `given`; CHANGELOG and an
   upgrade note.
6. `ikigai-core`: `ValueNote::requires`, enforced, validated and shown the same way per value.
7. ikigai-cli email (`attach`) and ikigai-personal calendar (`q`, then `as` after step 6): declare
   the input's scope, keep the run-time check for one release, and add a test that the manifold
   shows the input unavailable to a holder of the action's scopes alone.
8. ikigai-personal `urn:personal:calendars`: declare `…:calendar:read:detail` on its Source action
   (possible today, independent of this proposal).
9. ikigai-meeting: have the `teacher` write-back denial carry `Denial::lacking` with the scope it
   needed, since no declaration can state it exactly.
10. Hosts that render the manifold (cli, MCP door): show an unavailable input in words, naming
    the scope.
11. Finish the survey in the unread files listed above.
