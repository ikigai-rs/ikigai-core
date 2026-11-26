# A closed integrity and confidentiality lattice, carried as open label IRIs

**Status:** design only (ledger #3640). It extends `docs/design/provenance-labels.md` (ledger
#2389), which is also design only. Nothing in the kernel changes. The join, the encoding and the
policy check below are pinned by a model in `crates/ikigai-core/tests/label_lattice.rs`. One
test there runs the encoding through today's kernel to show that its existing join already
computes the lattice join.

**Anchor types:** the sketched `Label` and `Provenance::labels` of `provenance-labels.md`, and
`ArgSpec::refuses` / `ActionSpec::refuses` from the same note.
**Prior art:** Denning's lattice model of secure information flow (1976). Biba integrity and
Bell–LaPadula confidentiality are the two classic dimensions, and their product is the usual
two-dimension label. CaMeL (see `provenance-labels.md`) tracks sources and readers, a similar
pair.

---

## The need

`provenance-labels.md` gives a representation an **open set of label IRIs**. Labels join by
union, only accumulate, and are refused per IRI at the floor. Some consumers want something
closed instead: a fixed lattice with two dimensions and a fixed join.

- **Integrity** is a chain such as `untrusted < endorsed < trusted`. A value derived from two
  inputs is only as trustworthy as the less trustworthy one, so integrity joins by **minimum**.
- **Confidentiality** is a chain such as `public < internal < secret`. A value derived from two
  inputs is as sensitive as the more sensitive one, so confidentiality joins by **maximum**.

A point is a pair (integrity, confidentiality). The flow order is `a ⊑ b` when `a` has at least
`b`'s integrity and at most `b`'s confidentiality. The join of two points is their least upper
bound in that order. The bottom of the order, (trusted, public), is what a constant carries.

Such a consumer wants three things. It wants to read an answer's point, not a bag of IRIs. It
wants a policy that names a point ("admit at most (endorsed, internal)"). And it wants a
guarantee that the join is exactly min and max, with no IRI it did not plan for changing the
result.

## Decision: carry the lattice inside the open set, encoded so that union is the join

The kernel keeps one carrier, the open label set, and one join, set union. A lattice point is
written as the set of **threshold IRIs** it satisfies:

| IRI | Present when |
|---|---|
| `urn:label:integrity:below:<level>` | integrity is strictly below `<level>` (one IRI per level above the lowest) |
| `urn:label:confidentiality:above:<level>` | confidentiality is strictly above `<level>` (one IRI per level below the highest) |

With the three-level chains above, the IRIs are `integrity:below:endorsed`,
`integrity:below:trusted`, `confidentiality:above:public` and `confidentiality:above:internal`.
For example, (untrusted, internal) is `{integrity:below:endorsed, integrity:below:trusted,
confidentiality:above:public}`.

A lower integrity satisfies **more** "below" thresholds, so its set contains a higher level's
set. The union of two encodings is therefore the encoding of the lower one: union computes the
minimum. Confidentiality works the same way in the other direction, so union computes the
maximum. Together, **`encode(a) ∪ encode(b) = encode(a ⊔ b)`** for every pair of points. This
is Birkhoff's representation of a distributive lattice as down-sets, and a product of chains is
distributive. The test file checks it over all 81 pairs of the 3×3 lattice.

The consequences:

- **No second join in the kernel.** Every join point that `provenance-labels.md` lists (a
  sub-request answer read, the incoming pipe, a by-reference argument, a cache hit, the issuer's
  set traveling with a sub-request, a peer across a wire) is a union. None of them needs to know
  that a lattice exists. The last test in the file runs two labeled sources under a composite
  through today's kernel. It uses dependency threads as a stand-in carrier, and the composite's
  answer decodes to the join.
- **Default behavior is unchanged.** The bottom point (trusted, public) encodes as the **empty
  set**. An unlabeled representation, which is every representation today, already reads as the
  bottom. A consumer that defines no lattice sees nothing new.
- **Open labels sit beside it.** Threshold IRIs and open labels such as `urn:label:origin:web`
  share one set. A lattice reading ignores IRIs outside its two families. An open-label policy
  ignores the threshold IRIs it does not name.
- **Stray IRIs only narrow.** An endpoint may add labels (adding is narrowing) but never remove
  one. If it adds a threshold IRI that no `encode` would produce alone, for example
  `integrity:below:endorsed` without `integrity:below:trusted`, the set is not a clean
  encoding. The reading is therefore defined to be robust: integrity is the **lowest** level
  any present "below" IRI implies, and confidentiality is the **highest** level any present
  "above" IRI implies. A stray IRI can lower integrity or raise confidentiality, never the
  reverse. The test file checks this for every point and every single stray IRI.

### Why not a typed point beside the set

The alternative is a typed field, `Provenance { labels, point: Option<Point> }`, with its own
min/max join. It reads more directly, but it costs more than it gives:

- Every join point in `provenance-labels.md` would need a second join. The cache entry, the
  wire codec and the Turtle face would each need a second field.
- The levels belong to the consumer, not the kernel. A typed field would make the kernel generic
  over a lattice it does not define, or fix one set of levels for every host.
- Two carriers can disagree, for example after a peer sends one and not the other. One set
  cannot.

A typed **view** is still worth having, but outside the kernel: a small helper that a policy or
a face uses to read a point from the set (`decode` in the test file). It is a reader, not a
carrier.

### Wider lattices

The same encoding covers any finite distributive lattice. A confidentiality dimension made of
**compartments** (a set of categories, as in Bell–LaPadula's categories) is a powerset. There,
one IRI per compartment, joined by union, is already the join. A non-distributive lattice has
no such encoding. No consumer has asked for one, and this note does not cover it.

## How a gatekeeper policy names a lattice point

A policy names the **highest point it admits**, its ceiling. A ceiling is spelled as at most two
refused IRIs, using the `refuses` mechanism that `provenance-labels.md` already proposes:

| Ceiling | Refused IRIs |
|---|---|
| integrity at least `<i>` | `urn:label:integrity:below:<i>` (none if `<i>` is the lowest level) |
| confidentiality at most `<c>` | `urn:label:confidentiality:above:<c>` (none if `<c>` is the highest level) |

A request is admitted when none of the refused IRIs is present. The test file checks that this
admits exactly the points that flow to the ceiling, for every ceiling and every point. So a
point-shaped policy needs no new floor check: it is sugar over the per-IRI refusal.

```rust
// sketch, not built: a ceiling written as a point, lowered to `refuses`
ActionSpec::new(Verb::Sink)
    .input(ArgSpec::new("to").at_most(lattice.point("endorsed", "secret")))
// is the same contract as
    .input(ArgSpec::new("to").refuses("urn:label:integrity:below:endorsed"))
```

The mail Sink from `provenance_labels.rs` is this case: `to` needs at least `endorsed`
integrity, at any confidentiality, which is one refused IRI.

A refusal still names the argument, the IRI and the thread the IRI came from, as
`provenance-labels.md` decides. With a lattice declared, the message can also say the point it
read and the ceiling it was held to, in words: "`to` is untrusted, and this action needs at
least endorsed". That follows ACCESSIBILITY.md: the first words say the kind of failure, and
levels are named, never given as numbers.

A host's gatekeeper adds ceilings to contracts it did not write, as it adds refusals. Adding a
ceiling only adds refused IRIs, so the host can tighten a contract and never loosen it.

## Endorsement and declassification

The host endorsement table in `provenance-labels.md` removes one named label at a declared
sanitizer. For a lattice, the entry names a **move**, and the kernel removes a closed family of
IRIs so that the result is still a clean encoding:

- **Endorse to `<i>`** (raise integrity): remove `integrity:below:<j>` for every `<j>` at or
  below `<i>`.
- **Declassify to `<c>`** (lower confidentiality): remove `confidentiality:above:<j>` for every
  `<j>` at or above `<c>`.

These are the only operations that move a point up in integrity or down in confidentiality. They
stay a host's act, as in `provenance-labels.md`, and an endpoint cannot reach them. Every other
label, open or threshold, passes through. An endorsement never declassifies, and a
declassification never endorses. Each one names its dimension.

## Declaring the lattice

The levels are data, and a host declares them in the vocabulary as a SKOS ordered collection per
dimension, in the same style as `ik:LayerKindOrder`. From that collection, the threshold IRIs are
derived mechanically, and a reader orders levels by `skos:memberList` instead of hard-coding
names. The kernel reads none of this. A policy, a face or a validator does.

Declaring the lattice also gives `urn:kernel:validate` a way to check a ceiling: a ceiling that
names a level the declared lattice lacks is a contract error at declaration time, not a request
refused later.

## What it costs

- **The kernel:** nothing beyond the first increment of `provenance-labels.md`. The carrier, the
  union join and the per-IRI refusal are all that the lattice needs.
- **Vocabulary:** two threshold IRI families, and a way to declare a dimension's ordered levels.
- **A helper:** `decode` and `refusals_for` as a small library or module, outside the kernel's
  resolution path. Policy authors write points, and the helper lowers them to IRIs.
- **The cache key:** unchanged, as `provenance-labels.md` says: labels are a function of the
  inputs, which already identify the entry.

## Open questions

- **Number of levels.** A dimension with `n` levels costs `n − 1` IRIs in the worst case on
  every labeled answer. Three or four levels per dimension is cheap. A consumer that wants many
  levels may want a compact wire form that sends the point and expands it on arrival. Since the
  expansion is deterministic, that is a codec choice, not a semantic one.
- **Mixed lattices across a wire.** Two hosts that declare different levels under the same IRI
  family would misread each other. The IRIs should probably carry the lattice's own IRI (for
  example `urn:label:<lattice>:integrity:below:<level>`), so that a peer's labels from an
  unknown lattice are carried as open labels and never misread.
- **The formal side.** "Union of encodings equals encoding of the join" is a lemma that the
  monotonicity statement in `provenance-labels.md` can cite once labels are built.

## Proposed follow-up items

1. **Threshold IRIs carry their lattice's name.** Settle the IRI spelling
   (`urn:label:<lattice>:integrity:below:<level>`, or something else) so that two lattices
   never collide across a wire, and amend the test model to match.
2. **Vocabulary: declare a label dimension's ordered levels.** Add `ik:LabelDimension` and an
   ordered SKOS collection per dimension in ikigai-vocab, in the `ik:LayerKindOrder` style, with
   a test that derives the threshold IRIs from the declaration.
3. **A lattice helper outside the resolution path.** A small module offering `Lattice`, `Point`,
   `decode`, `refusals_for`, endorse-to and declassify-to, built on the open label set once the
   first increment of `provenance-labels.md` lands. The model in `tests/label_lattice.rs` is its
   specification.
4. **`ArgSpec::at_most(point)` lowers to `refuses`.** Once `refuses` exists, add the point-shaped
   spelling as sugar, and require `urn:kernel:actions` and `urn:kernel:validate` to report the
   lowered IRIs, so that what is offered matches what is admitted.
5. **Lattice-aware refusal text.** When a lattice is declared, a refusal names the point read and
   the ceiling in words, following ACCESSIBILITY.md, with a test on the message.
6. **Lattice moves in the endorsement table.** Extend the host endorsement table with "endorse
   to" and "declassify to" entries that remove a closed family of threshold IRIs, with tests
   that the result is still a clean encoding and that other labels pass through.
