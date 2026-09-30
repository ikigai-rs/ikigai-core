# Space declarations: the arrangement as data, coming in

**Status:** arc 1 built, core 0.1.83 (ledger [#633](http://localhost:1060/l/default/item/633),
with [#608](http://localhost:1060/l/default/item/608) ahead of it). The design and Brian's five
decisions of 2026-09-29 are in the hub's brief `ikigai-core-space-declarations.md`; this note says
what core now does and does not do.

## The idea

Core 0.1.78 made the arrangement a resource going OUT: every space reports a `Topology`, and
`urn:kernel:topology` renders it as Turtle over the `ik:` vocabulary
([`spaces-as-named-graphs.md`](spaces-as-named-graphs.md) §6). A **declaration** is the same graph
coming IN: a resource whose representation is that Turtle, from which a live `Arc<dyn Space>` is
built. Endpoints stay code. A declaration arranges endpoints the host has registered, by name, and
never mints one. The arrangement around them becomes data: the order of doors, the patterns,
fallbacks, mounts, aliases, limiters, confinements, levels, seals and namespaces.

| step | what | where |
|---|---|---|
| read | `Topology::from_turtle(&str)`, the exact inverse of `Topology::to_turtle` | `declare` feature (brings `oxttl`/`oxrdf`) |
| provide | `Registry`: endpoint name → `Arc<dyn Endpoint>`, plus the host's sealed prefixes | always on |
| build | `build(&Topology, &Registry) -> Result<Arc<dyn Space>, DeclarationError>`, pure | always on |

The host reads the declaration resource through its kernel (a file, a store graph), so the file is
golden-threaded like any other resource; core only ever sees the parsed tree. `build` needs no
parser, so a host that constructs a `Topology` another way (or a surface that parses its own
syntax straight to the tree) pays nothing for RDF.

## What a declaration may do

Every kind a declaration can state is rebuilt with its public constructor, keeping its identity
(a named node stays named, an anonymous one stays anonymous):

| kind | rebuilt as |
|---|---|
| `ik:EndpointSpace` | its `ik:doors` in order, each an `Exact` or a `UriTemplate` by its `ik:matchKind`, bound to the registered endpoint its `ik:endpointName` names |
| a door's `ik:confinedTo` | the registered endpoint wrapped in `Confine`, named by the corridor's IRI, confined to the corridor built from the declaration |
| `ik:Fallback` | its `ik:layers`, in order |
| `ik:Mount` | its `ik:prefix` over its declared `ik:space` — a LOCAL mount |
| `ik:Alias` | its rules (`ik:rewrites`) and its hop bound (`ik:maxHops`). The rules' order is precedence (the first that matches a name applies), and it is SORTED, not authored: `AliasTable` re-sorts on every insertion, longest `from` first, exact before prefix at equal length, then by `from`, stably. So `{alias}:rule:{n}` numbers a rule's place in that sorted order, and a declaration written in any other order builds, then renders its rules sorted |
| `ik:Limit` | its `ik:family` as a prefix, an exact name, or a template, by its `ik:matchKind` |
| `ik:Level` | its name, `ik:seals` and `ik:namespace`, over its `ik:space` |

A name met twice is ONE space (one `Arc`), as a shared space is in code: a name is a claim, same
name, same doors.

## What a declaration may not do, and why

Never skip. A builder that drops what it does not understand builds a kernel that answers less
than its declaration says, and nothing would notice. So everything else is REFUSED, with an error
that names the node's IRI exactly as the Turtle does (skolems included) and says why:

| refused | why |
|---|---|
| `ik:OpaqueSpace` | a remote peer or a hand-written resolver: nothing to rebuild. Under an `ik:Mount` the error names the prefix and points at [#630](http://localhost:1060/l/default/item/630) (declared remote mounts) |
| `ik:Rewrite` | its rule is a closure. The table-driven form is `ik:Alias` |
| `ik:Chain` as the root | a declaration describes a space; the chain is per request. `urn:kernel:topology` answers a chain, so a host reading one back declares its root layer: `Kernel::root_topology()` is that layer, without unwrapping the chain by hand |
| `ik:Confine` where a space belongs | a confinement lives at a door: declare it as the door's `ik:confinedTo` |
| a door or family whose `ik:matchKind` is `custom` | a grammar written outside core is described by its pattern and not defined by it (browse's `RootRow` adds a binding its pattern does not show). Rebuilding it from the text would silently change what it answers |
| a `prefix` door, a template that does not parse | a door is an exact name or a template |
| an endpoint name the registry does not hold | a declaration binds only what the host registered: it arranges, it never mints |
| two different endpoints under one name | refused by `Registry::register`. Names are not unique (`FnEndpoint::new("x", …)` is always `x`), and a registry that kept the first or the last would bind a door to whichever one it happened to keep |
| two different arrangements under one name | same name, same doors: a declaration that breaks the claim is refused rather than built twice as two things. The renderer shares the check: `to_turtle` writes a named node once, so a clash would render as the first arrangement alone; `Topology::try_to_turtle` refuses it instead, and `urn:kernel:topology` answers `Error::Conflict` |
| a seal the host, core or another level owns | `Kernel::check_sealing` runs on the built space, against the host's prefixes (`Registry::sealing`), so a declaration cannot seal its way into a name the host sealed, nor bind a level's door under one |
| a declaration past a bound | deeper than `MAX_DECLARATION_DEPTH`, or more than `MAX_DECLARATION_NODES` nodes or `MAX_DECLARATION_TEXT` bytes of text once expanded: `DeclarationError::TooLarge`, naming the bound, its limit and the node. See [Bounds](#bounds) |

`Topology::from_turtle` is as strict about the document: every triple must belong to the one
arrangement under the one root. A blank node, an unknown kind or property, a property missing or
stated twice, an ill-formed list cell, a cycle, a leaf whose flat `ik:pattern`s disagree with its
`ik:doors`, or an `ik:maxHops` below one is refused, naming the node. A comment or a label on a
node is refused too, today: to_turtle would drop it, and a surface that cannot carry something
must say so rather than lose it. Admitting annotations is an additive relaxation if a surface
wants them.

## Bounds

A declaration is operator input (`ikigai --arrangement` hands its Turtle straight to
`from_turtle`), so core reads and builds it within three bounds, each a public constant and each
REFUSED whole with `DeclarationError::TooLarge` — never a crash, never a truncation (ledger
[#643](http://localhost:1060/l/default/item/643)):

| bound | limit | why |
|---|---|---|
| `MAX_DECLARATION_DEPTH` | 48 | every walk over a declaration recurses once per level, and a stack overflow ABORTS the process |
| `MAX_DECLARATION_NODES` | 65,536 | a named node is expanded at every place it is used (`Topology` is a value tree), so a few KB of Turtle whose named nodes each reference the next several times grows exponentially |
| `MAX_DECLARATION_TEXT` | 16 MiB | the node bound alone still lets one large literal in a node used thousands of times expand to gigabytes |

Depth counts the root as 1 and each enclosed space — a layer, a mounted, aliased or level's space, a
door's confined corridor — as one deeper. Nodes are spaces, doors and alias rules; text is every IRI
a node claims and every pattern, endpoint name, prefix, family, rule, seal and namespace. Both are
counted at every occurrence. `from_turtle` checks depth before each descent and counts as it
produces nodes, so an exponential document is refused after the bound, not after the expansion.
`build` accepts any tree, so it measures the tree first, without recursing, with the same
accounting: every tree `from_turtle` returns passes it, and every space `build` returns nests no
deeper than the bound.

The depth was chosen by measurement for a **1 MiB** stack in a **debug** build (wasm32's default,
and a small worker thread). Measured 2026-09-30 on the two costliest shapes (a chain of fallbacks,
a chain of confined doors): rendering overflows 1 MiB first, past 218 levels; parsing past 269;
building past 323; cloning, comparing, dropping, `topology()`, the seal check and a request
resolving through every level past 450. Getting there took slimming the two recursive walks
declarations own: reading a node and descending in one function had cost about 19 KB a level
(24 KB through a confined door) in the parser and about 10 KB in the builder, which put parsing's
overflow near 41 levels. Each now keeps what a node states about itself in a frame that is gone
before the walk descends. `tests/declare_bounds.rs` runs the whole pipeline at the bound on a
1 MiB thread.

## The round trip is the acceptance test

`tests/declare.rs`: for a kernel K covering every rebuildable kind, and for a tic-tac-toe-shaped
space (fourteen doors, eight exact aliases onto a `cells:{list}` template, over a fallback), render
K's topology, read it back, build K′ from the same registry, and require

- a fixpoint: `topology(K′) == topology(K)`, as a tree and as Turtle; and
- the same answers for a sample of names drawn from the arrangement itself (every exact door, an
  instance of every template, every alias source, names under every limiter, mount and seal, and
  names that miss): the same endpoint name, bindings, `answered_by`, canonical name and level
  path, and from a kernel over each, the same bytes or the same refusal.

The round trip found one defect in the rendering it inverts: a named space reached twice (one
confinement at two doors, a shared `Arc` under two mounts) was rendered twice, and its anonymous
descendants were skolemized twice under new numbers, so one list cell of the named node stated two
different members. `to_turtle` now renders a named node once, where it is first met.

## Surfaces

Turtle is the canonical form and the one core reads. Every other surface reaches core AS Turtle
(or as a `Topology` it builds itself):

| surface | status |
|---|---|
| Turtle | here |
| s-expressions | next, in `ikigai-sexpr`, which already transrepts s-expressions to Turtle (decision 1) |
| JSON-LD | free, through the existing JSON-LD transreptors and the regenerated context |
| YAML | deferred, [#629](http://localhost:1060/l/default/item/629) |

## Deferred, by decision

| what | ledger |
|---|---|
| hot reload: a watched declaration, a swappable root, and who may rebind a running kernel (`urn:cap:kernel:arrange`) | [#628](http://localhost:1060/l/default/item/628) |
| YAML as a surface | [#629](http://localhost:1060/l/default/item/629) |
| declared remote mounts, behind a host-granted capability | [#630](http://localhost:1060/l/default/item/630) |
| a factory registry: a module instantiated with configuration from the file | [#631](http://localhost:1060/l/default/item/631) |
| corridor templates: temporal and personal spaces, instantiated per request from values | [#632](http://localhost:1060/l/default/item/632) |

Also not here, and not core's: the cli registry and the start-time option that builds the root
from a declaration resource, the s-expression grammar, the tutorial's proving case, and the
picture of a space ([#635](http://localhost:1060/l/default/item/635), which reads the doors'
endpoint names this arc added).

## In the browser

`declare` is runtime-free and builds for WASI as it stands. On `wasm32-unknown-unknown`, `oxrdf`'s
`rand` needs getrandom's JS backend: core enables getrandom's `wasm_js` feature under `declare` on
that target, and since getrandom 0.3.4 the feature alone selects the backend, so a browser host
needs no rustflags (ikigai-diagram and the tutorial verified it in a real browser). Core's floor is
0.3.4 for that reason; before it, the host also had to set `--cfg getrandom_backend="wasm_js"`, and
a lock pinned to 0.3.0–0.3.3 still would. With `declare` off, nothing changes.
