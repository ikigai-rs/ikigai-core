use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::repr::{Expiry, Time};
use crate::verb::Verb;

/// Where an input's value comes from — the two channels an endpoint can read a
/// parameter through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputSource {
    /// A by-value argument supplied alongside the request and content-addressed
    /// into its identity (`ArgRef::Inline` / `Content`). The default.
    #[default]
    Argument,
    /// A variable captured from the resource identifier by the resolving grammar
    /// (e.g. a `{var}` in a [`UriTemplate`](crate::UriTemplate)). Its value lives
    /// in the IRI, so it is part of the resource's identity directly.
    Binding,
}

/// A specification of one named input an endpoint accepts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArgSpec {
    /// The argument name.
    pub name: String,
    /// A short human description.
    pub summary: String,
    /// Whether the argument is required.
    pub required: bool,
    /// Whether the value arrives as a by-value argument or a grammar binding.
    #[serde(default)]
    pub source: InputSource,
    /// The RDF class OR datatype this input's value is expected to be, as an IRI: an
    /// `rdfs:Class` for entity-valued inputs (e.g. `https://schema.org/Person`), an XSD
    /// datatype for scalars (e.g. `xsd:dateTime`). Optional; when present it lets selection
    /// match an endpoint to the *types* available in a context — "what can I do with these
    /// entities?" (`select_action`) — the same way `transreptsFrom`/`To` drives transreptor
    /// selection. Omitted from serialized output when absent, so existing JSON contracts are
    /// unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// The value assumed when the argument is omitted. Omitted from serialized output when
    /// absent, so existing JSON contracts are unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// The closed set of accepted values, when the argument is an enumeration (e.g.
    /// `mode=added|removed`). Empty = open-valued. Omitted from serialized output when
    /// empty, so existing JSON contracts are unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<String>,
    /// The inputs this one needs: when this input is given, each named input must be
    /// given too (validate's own `endpoint` needs `verb`). Names of inputs of the same
    /// action. Omitted from serialized output when empty, so existing JSON contracts are
    /// unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// When the value is a LIST in one argument (`proposals=minor,info`, `graph=A B`), the
    /// separator its items are split on: `one_of` and `class` then apply to EACH item, and
    /// this is the one argument a caller may give more than once (the repeats are joined
    /// with the separator's canonical character, so `graph=A graph=B` and `graph="A,B"` are
    /// one call). Ledger #497, #357, #387. Omitted from serialized output when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<ListSeparator>,
}

/// How a list-valued argument's items are separated in its one value (see
/// [`ArgSpec::list`]).
///
/// ```
/// use ikigai_core::ListSeparator;
///
/// assert_eq!(ListSeparator::Comma.split("minor, info,,"), ["minor", "info"]);
/// assert_eq!(ListSeparator::CommaOrWhitespace.split("urn:a, urn:b  urn:c"), ["urn:a", "urn:b", "urn:c"]);
/// assert_eq!(ListSeparator::Newline.split("a\r\nb\n\n"), ["a", "b"]);
/// // Repeats are joined with the canonical character.
/// assert_eq!(ListSeparator::CommaOrWhitespace.join(["urn:a", "urn:b"]), "urn:a,urn:b");
/// assert_eq!(ListSeparator::Whitespace.join(["a", "b"]), "a b");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ListSeparator {
    /// `,` (items trimmed): `proposals=minor,info`.
    Comma,
    /// Runs of whitespace: `graph="A B"`.
    Whitespace,
    /// `,` or whitespace, runs collapsed: `graph=A,B C`.
    CommaOrWhitespace,
    /// Line breaks (items trimmed): the `..` map convention, so a list argument can take a
    /// map stage's joined output.
    Newline,
}

impl ListSeparator {
    /// The separator's word, as the JSON contract and `ik:listSeparator` spell it:
    /// `comma`, `whitespace`, `comma-or-whitespace` or `newline`.
    pub fn word(self) -> &'static str {
        match self {
            ListSeparator::Comma => "comma",
            ListSeparator::Whitespace => "whitespace",
            ListSeparator::CommaOrWhitespace => "comma-or-whitespace",
            ListSeparator::Newline => "newline",
        }
    }

    /// The character repeats are joined with: `,` for [`Comma`](Self::Comma) and
    /// [`CommaOrWhitespace`](Self::CommaOrWhitespace), a space for
    /// [`Whitespace`](Self::Whitespace), `\n` for [`Newline`](Self::Newline).
    pub fn canonical(self) -> char {
        match self {
            ListSeparator::Comma | ListSeparator::CommaOrWhitespace => ',',
            ListSeparator::Whitespace => ' ',
            ListSeparator::Newline => '\n',
        }
    }

    /// The items of `value`, empty items dropped.
    pub fn split(self, value: &str) -> Vec<&str> {
        let items: Vec<&str> = match self {
            ListSeparator::Comma => value.split(',').map(str::trim).collect(),
            ListSeparator::Whitespace => value.split_whitespace().collect(),
            ListSeparator::CommaOrWhitespace => value
                .split(|c: char| c == ',' || c.is_whitespace())
                .collect(),
            ListSeparator::Newline => value.lines().map(str::trim).collect(),
        };
        items.into_iter().filter(|item| !item.is_empty()).collect()
    }

    /// `items` joined with the [`canonical`](Self::canonical) character.
    pub fn join<'a>(self, items: impl IntoIterator<Item = &'a str>) -> String {
        let mut out = String::new();
        for (n, item) in items.into_iter().enumerate() {
            if n > 0 {
                out.push(self.canonical());
            }
            out.push_str(item);
        }
        out
    }
}

impl ArgSpec {
    /// A required by-value argument with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        ArgSpec {
            name: name.into(),
            summary: String::new(),
            required: true,
            source: InputSource::Argument,
            class: None,
            default: None,
            one_of: Vec::new(),
            needs: Vec::new(),
            list: None,
        }
    }

    /// Add a human summary (builder).
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Declare the RDF class (IRI) this input's value is expected to be (builder) — e.g.
    /// `https://schema.org/Person`. Drives type-based selection (`select_action`).
    pub fn class(mut self, class: impl Into<String>) -> Self {
        self.class = Some(class.into());
        self
    }

    /// Declare the value assumed when the argument is omitted (builder). Implies the
    /// argument is optional.
    pub fn default_value(mut self, value: impl Into<String>) -> Self {
        self.default = Some(value.into());
        self.required = false;
        self
    }

    /// Declare the closed set of accepted values (builder) — e.g. `["added", "removed"]`
    /// for a `mode=` argument.
    pub fn one_of(mut self, values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.one_of = values.into_iter().map(Into::into).collect();
        self
    }

    /// Declare inputs this one needs (builder): when it is given, each of `inputs` must be
    /// given too. Callable more than once; the names accumulate.
    ///
    /// ```
    /// use ikigai_core::{ArgSpec, Description, ProposedArgs, Verb};
    ///
    /// let d = Description::new("probe")
    ///     .verb(Verb::Source)
    ///     .input(ArgSpec::new("endpoint").optional().needs(["verb"]))
    ///     .input(ArgSpec::new("verb").optional());
    /// let source = &d.action_specs()[0];
    /// assert!(source.check(&ProposedArgs::parse("endpoint=urn:x&verb=source")).is_empty());
    /// assert!(!source.check(&ProposedArgs::parse("endpoint=urn:x")).is_empty());
    /// // Needing is one-way: `verb` alone needs nothing.
    /// assert!(source.check(&ProposedArgs::parse("verb=source")).is_empty());
    /// ```
    pub fn needs(mut self, inputs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.needs.extend(inputs.into_iter().map(Into::into));
        self
    }

    /// Declare the value a list whose items are separated by `separator` (builder): its
    /// `one_of` and `class` apply to each item, and it is the one kind of argument a caller
    /// may give more than once.
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, ArgSpec, ListSeparator, ProposedArgs, Verb};
    ///
    /// let file = ActionSpec::new(Verb::Source).input(
    ///     ArgSpec::new("proposals")
    ///         .optional()
    ///         .list(ListSeparator::Comma)
    ///         .one_of(["critical", "major", "minor", "info"]),
    /// );
    /// assert!(file.check(&ProposedArgs::parse("proposals=minor,info")).is_empty());
    /// let bad = file.check(&ProposedArgs::parse("proposals=minor,bogus"));
    /// assert_eq!(bad.len(), 1);
    /// assert!(bad[0].message.contains("`bogus`"), "{}", bad[0]);
    /// // Repeated, it is joined: one call.
    /// assert!(file.check(&ProposedArgs::parse("proposals=minor&proposals=info")).is_empty());
    /// ```
    pub fn list(mut self, separator: ListSeparator) -> Self {
        self.list = Some(separator);
        self
    }

    /// Mark the argument optional (builder).
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Declare that this input is captured from the resource identifier by the
    /// resolving grammar rather than passed as a by-value argument (builder).
    pub fn binding(mut self) -> Self {
        self.source = InputSource::Binding;
        self
    }

    /// Every string in this spec that an emitter puts in an IRI position, paired with
    /// the field name to blame. `name` becomes the input node's IRI segment; `class` is
    /// emitted as a resource (`ik:class <…>`).
    fn iri_positions(&self) -> impl Iterator<Item = (&'static str, &str)> {
        std::iter::once(("input name", self.name.as_str()))
            .chain(self.class.as_deref().map(|c| ("input class", c)))
    }
}

/// How an [`InputGroup`] relates its members.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum GroupKind {
    /// Exactly one member is given (`urn:kernel:cut`'s `thread` or `content`).
    ExactlyOne,
    /// At least one member is given.
    AtLeastOne,
    /// At most one member is given: mutually exclusive, both optional.
    AtMostOne,
}

impl GroupKind {
    /// The kind's word, as the JSON contract and `ik:groupKind` spell it:
    /// `exactly-one`, `at-least-one` or `at-most-one`.
    pub fn word(self) -> &'static str {
        match self {
            GroupKind::ExactlyOne => "exactly-one",
            GroupKind::AtLeastOne => "at-least-one",
            GroupKind::AtMostOne => "at-most-one",
        }
    }
}

/// A relation between an action's inputs that independent [`ArgSpec`]s cannot state
/// (ledger #15): "exactly one of `thread` or `content`", "at least one of these".
///
/// Declared on the ACTION, never on an input, because it is about several inputs at
/// once. Every member is a by-value input of the same action and is **declared
/// optional**: a reader that predates groups (an old engine, an old Python peer) then
/// sees the loose-but-honest contract it saw before, never a false refusal, and a group
/// only ever ADDS a constraint for a reader that knows it. A member is GIVEN when the
/// call carries it with a non-empty value: a Sink's engine always sends `content`, empty
/// when there is no body, so an empty value cannot count as choosing it.
///
/// `members` is a set (the builders sort and deduplicate it), so declaration order is not
/// identity and cannot carry meaning. That is why the member a pipe fills is named
/// explicitly by [`pipe`](Self::pipe) rather than being "the first".
///
/// ```
/// use ikigai_core::{ArgSpec, Description, InputGroup, ProposedArgs, Verb};
///
/// let cut = Description::new("cut")
///     .verb(Verb::Sink)
///     .input(ArgSpec::new("thread").optional())
///     .input(ArgSpec::new("content").optional())
///     .group(InputGroup::exactly_one(["thread", "content"]).pipe("content"));
/// assert!(cut.validate().is_ok());
/// let sink = &cut.action_specs()[0];
/// assert!(sink.check(&ProposedArgs::parse("thread=urn:x")).is_empty());
/// assert!(sink.check(&ProposedArgs::parse("content=urn:x")).is_empty());
/// // None, or both, is a violation: the false green of ledger #15 is gone.
/// assert!(!sink.check(&ProposedArgs::new()).is_empty());
/// assert!(!sink.check(&ProposedArgs::parse("thread=urn:x&content=urn:y")).is_empty());
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct InputGroup {
    /// How the members relate.
    pub kind: GroupKind,
    /// The member input names, a set (sorted and deduplicated by the builders).
    pub members: Vec<String>,
    /// The member a pipe, a positional value or a batch's stdin fills, when none of the
    /// members is named (Source only; on a Sink or Delete the pipe always lands in
    /// `content`, so this is `content` or absent there). A group with no `pipe` leaves the
    /// action named-only for these inputs. Omitted from serialized output when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pipe: Option<String>,
}

impl InputGroup {
    fn of(kind: GroupKind, members: impl IntoIterator<Item = impl Into<String>>) -> Self {
        let mut members: Vec<String> = members.into_iter().map(Into::into).collect();
        members.sort();
        members.dedup();
        InputGroup {
            kind,
            members,
            pipe: None,
        }
    }

    /// Exactly one of `members` is given.
    pub fn exactly_one(members: impl IntoIterator<Item = impl Into<String>>) -> Self {
        InputGroup::of(GroupKind::ExactlyOne, members)
    }

    /// At least one of `members` is given.
    pub fn at_least_one(members: impl IntoIterator<Item = impl Into<String>>) -> Self {
        InputGroup::of(GroupKind::AtLeastOne, members)
    }

    /// At most one of `members` is given.
    pub fn at_most_one(members: impl IntoIterator<Item = impl Into<String>>) -> Self {
        InputGroup::of(GroupKind::AtMostOne, members)
    }

    /// Name the member a pipe fills when no member is named (builder).
    pub fn pipe(mut self, member: impl Into<String>) -> Self {
        self.pipe = Some(member.into());
        self
    }

    /// The group node's segment under its contract IRI: `group:{kind}:{m1},{m2}`, each
    /// member a NAME ([`encode_name`](crate::encode_name), with `,` encoded as `%2C` too so
    /// the list is unambiguous), the members sorted. Stable, so a catalog holding the group
    /// diffs, and the node a validation report's `sh:resultPath` names.
    pub fn node_segment(&self) -> String {
        let mut members: Vec<String> = self
            .members
            .iter()
            .map(|m| crate::encode_name(m).replace(',', "%2C"))
            .collect();
        members.sort();
        members.dedup();
        format!("group:{}:{}", self.kind.word(), members.join(","))
    }

    /// Whether `name` is a member.
    pub fn has(&self, name: &str) -> bool {
        self.members.iter().any(|m| m == name)
    }
}

/// One verb's contract on an endpoint: the inputs it reads, the outputs it can produce,
/// and the capability scopes invoking it requires. The **action** — an (endpoint, verb)
/// pair — is the unit of selection: a calendar endpoint's `Source` (read, under a read
/// capability) and `Sink` (write, different arguments, write capability) are different
/// actions with different contracts.
///
/// Most endpoints never construct one: a single-verb endpoint's flat
/// [`Description`] fields ARE its action spec, and [`Description::action_specs`]
/// synthesizes the per-verb view. That flat form is the common case and is permanent
/// sugar, not a deprecated one.
///
/// Declare explicit `ActionSpec`s when a verb's contract genuinely differs from the flat
/// one: different capability scopes (a calendar's `Sink` demands write authority its
/// `Source` does not), different arguments, or a different summary. If every verb reads the
/// same inputs under the same authority, the flat fields already say so.
///
/// ⚠ The two forms are **exclusive per verb, not additive**. [`Description::action_specs`]
/// takes an explicit spec whole for the verbs that have one, so its `inputs`, `outputs` and
/// `requires` REPLACE the flat fields for that verb rather than adding to them: an explicit
/// spec whose `requires` is empty declares that the verb needs no authority, and the flat
/// `requires` sitting beside it is dead for that verb. Restate on each explicit spec
/// whatever it still needs — and state no more, since the kernel enforces what is declared.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionSpec {
    /// The verb this contract applies to.
    pub verb: Verb,
    /// A short human summary of what this verb does on this endpoint (optional).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    /// The named inputs this verb reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<ArgSpec>,
    /// The representation types this verb can produce.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    /// The capability scopes invoking this verb requires (IRIs like
    /// `urn:cap:personal:calendar:write`) — enforced by the kernel before dispatch, and
    /// REPLACING the description's flat `requires` for this verb.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Relations between this verb's inputs (ledger #15): see [`InputGroup`]. Omitted from
    /// serialized output when empty, so existing JSON contracts are unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<InputGroup>,
}

impl ActionSpec {
    /// An action spec for the given verb.
    pub fn new(verb: Verb) -> Self {
        ActionSpec {
            verb,
            summary: String::new(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            requires: Vec::new(),
            groups: Vec::new(),
        }
    }

    /// Set the summary (builder).
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Declare an input (builder).
    pub fn input(mut self, input: ArgSpec) -> Self {
        self.inputs.push(input);
        self
    }

    /// Declare an output media type (builder).
    pub fn output(mut self, media_type: impl Into<String>) -> Self {
        self.outputs.push(media_type.into());
        self
    }

    /// Declare a required capability scope (builder).
    pub fn requires(mut self, capability: impl Into<String>) -> Self {
        self.requires.push(capability.into());
        self
    }

    /// Declare a relation between this verb's inputs (builder): see [`InputGroup`].
    pub fn group(mut self, group: InputGroup) -> Self {
        self.groups.push(group);
        self
    }

    /// The pipe members of every [`ExactlyOne`](GroupKind::ExactlyOne) or
    /// [`AtLeastOne`](GroupKind::AtLeastOne) group none of whose members is in `named`:
    /// the slots a piped, positional or stdin value can fill beside the required inputs.
    ///
    /// For the engine's routing rule (a Source's piped value fills the one REQUIRED
    /// argument left unnamed): a group member is declared optional, so without this a
    /// piped-or-named intake moved into a group would leave every pipeline. The engine's
    /// "required set" is the unnamed required inputs plus these.
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, ArgSpec, InputGroup, Verb};
    ///
    /// let lift = ActionSpec::new(Verb::Source)
    ///     .input(ArgSpec::new("content").optional())
    ///     .input(ArgSpec::new("in").optional())
    ///     .group(InputGroup::exactly_one(["content", "in"]).pipe("content"));
    /// assert_eq!(lift.pipe_slots(&[]), ["content"]);
    /// // Named `in=`: the group is satisfied, and no slot is left for a value.
    /// assert!(lift.pipe_slots(&["in"]).is_empty());
    /// assert_eq!(lift.excluded_by_groups(&["in"]), ["content"]);
    /// ```
    pub fn pipe_slots(&self, named: &[&str]) -> Vec<&str> {
        let mut slots: Vec<&str> = Vec::new();
        for group in &self.groups {
            if !matches!(group.kind, GroupKind::ExactlyOne | GroupKind::AtLeastOne) {
                continue;
            }
            if group.members.iter().any(|m| named.contains(&m.as_str())) {
                continue;
            }
            if let Some(pipe) = group.pipe.as_deref() {
                if !slots.contains(&pipe) {
                    slots.push(pipe);
                }
            }
        }
        slots
    }

    /// The members of every [`ExactlyOne`](GroupKind::ExactlyOne) or
    /// [`AtMostOne`](GroupKind::AtMostOne) group that already has a member in `named`,
    /// other than the named ones: inputs a value must NOT be routed into, because giving
    /// one would break the group. The engine drops these from its "remaining arguments" so
    /// a positional value after `in=` is refused as "every declared argument is already
    /// set" rather than routed into `content` and then refused as "got both".
    pub fn excluded_by_groups(&self, named: &[&str]) -> Vec<&str> {
        let mut excluded: Vec<&str> = Vec::new();
        for group in &self.groups {
            if !matches!(group.kind, GroupKind::ExactlyOne | GroupKind::AtMostOne) {
                continue;
            }
            if !group.members.iter().any(|m| named.contains(&m.as_str())) {
                continue;
            }
            for member in &group.members {
                if !named.contains(&member.as_str()) && !excluded.contains(&member.as_str()) {
                    excluded.push(member);
                }
            }
        }
        excluded
    }

    /// Every string in this spec that an emitter puts in an IRI position, paired with
    /// the field name to blame. Shared by [`Description::validate`] so the predicate can
    /// never fall behind what the RDF projection actually emits.
    fn iri_positions(&self) -> impl Iterator<Item = (&'static str, &str)> {
        self.inputs
            .iter()
            .flat_map(ArgSpec::iri_positions)
            .chain(self.requires.iter().map(|c| ("requires", c.as_str())))
    }
}

/// A structured, RDF-agnostic self-description of an endpoint.
///
/// `ikigai-core` keeps this free of any RDF dependency; `ikigai-vocab` projects
/// it to RDF (e.g. Turtle) so a `Meta` request can return a machine-readable
/// description of an endpoint.
///
/// **On the wire it is JSON, or another SELF-DESCRIBING format.** The serde derive
/// here, and on [`ArgSpec`](crate::ArgSpec) and [`ActionSpec`] inside it,
/// omits empty and default fields (`skip_serializing_if`) so the JSON Meta face stays
/// small and a field added later never changes an existing contract's JSON. A format
/// that is not self-describing (postcard, bincode) cannot read that back: it has no
/// field names, so an omitted field reads as the end of the buffer
/// (`DeserializeUnexpectedEnd` from postcard, even for `Description::new("x")`). A
/// binary protocol that carries a description carries it as JSON text inside its own
/// frame, which is what `ikigai-module` does. [`Request`](crate::Request) is the
/// opposite case on purpose: it skips nothing, because the IPC wire is postcard.
///
/// ```
/// use ikigai_core::{ArgSpec, Description, Verb};
///
/// let d = Description::new("wc").verb(Verb::Source).input(ArgSpec::new("in"));
/// let json = serde_json::to_value(&d).unwrap();
/// // The empty and default fields are omitted, not written as empty values…
/// for absent in ["requires", "actions", "kind"] {
///     assert!(json.get(absent).is_none(), "{absent} is omitted");
/// }
/// assert!(json["inputs"][0].get("one_of").is_none());
/// // …and a self-describing reader fills them back in.
/// let back: Description = serde_json::from_value(json).unwrap();
/// assert_eq!(back, d);
/// ```
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Description {
    /// The endpoint identifier.
    ///
    /// **What is enforced**: nothing at construction — [`Description::new`] takes any
    /// string. The one hard requirement is that the id must be safe in an RDF `IRIREF`
    /// ([`is_iri_safe`](crate::is_iri_safe)), because it is projected into IRI positions
    /// (`urn:ikigai:endpoint:{id}`, and the action and input nodes hung off it). Emitters
    /// percent-encode rather than trust it, and [`validate`](Self::validate) reports it —
    /// call that if you would rather fail than be encoded. A second requirement has no
    /// encoded fallback: no code point outside RFC 3987's `ucschar` (DEL, a C1 control,
    /// private use, a noncharacter), which emitters spell as it is and an IRI parser
    /// refuses; only [`validate`](Self::validate) catches it (ledger #1351).
    ///
    /// **What is convention**: a short noun in `kebab-case` (`camel-case`, `tag-suggest`).
    /// Not enforced, and not universal — 27 ids in the ecosystem are full IRIs
    /// (`urn:cms:graph`, `urn:secret`), which nest oddly as
    /// `<urn:ikigai:endpoint:urn:cms:graph>` and are load-bearing anyway: the MCP
    /// projection derives tool names from this field, so an id is a published name.
    pub id: String,
    /// A short human title.
    pub title: String,
    /// A longer human summary.
    pub summary: String,
    /// The verbs this endpoint answers.
    pub verbs: Vec<Verb>,
    /// The named inputs it accepts.
    pub inputs: Vec<ArgSpec>,
    /// The representation types it can produce (canonical media-type strings).
    pub outputs: Vec<String>,
    /// What kind of endpoint this is. Defaults to [`EndpointKind::Endpoint`]; omitted from
    /// serialized output for plain endpoints, so existing JSON contracts are unchanged.
    #[serde(default, skip_serializing_if = "EndpointKind::is_endpoint")]
    pub kind: EndpointKind,
    /// The capability scopes invoking this endpoint requires (IRIs such as
    /// `urn:cap:net:*`, `urn:cap:fs:read`) — what *authority* it demands. **Enforced**:
    /// the kernel refuses, before dispatch, a caller whose
    /// [`Capability`](crate::Capability) does not satisfy them, on the same predicate
    /// selection and `urn:kernel:validate` use — and the same declaration lets a host
    /// project a capability-scoped catalog and a caller pre-check feasibility. Applies to
    /// every verb this flat form stands for (each declared verb without its own
    /// [`ActionSpec`]); declared with no verb at all, it applies to EVERY verb. A verb the
    /// endpoint does not declare is not a way around it: see
    /// [`Kernel`](crate::Kernel)'s capability floor. Empty for endpoints needing no special
    /// authority; omitted from serialized output when empty, so existing JSON contracts are
    /// unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Per-verb contracts, for endpoints whose verbs genuinely differ (a calendar's Source
    /// vs Sink). An explicit action WINS over the flat fields for its verb; verbs with no
    /// explicit action get one synthesized from the flat fields
    /// ([`action_specs`](Self::action_specs)). Empty for the common single-verb endpoint;
    /// omitted from serialized output when empty, so existing JSON contracts are unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionSpec>,
    /// Relations between the flat inputs (ledger #15), for every verb this flat form
    /// stands for: see [`InputGroup`]. Omitted from serialized output when empty, so
    /// existing JSON contracts are unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<InputGroup>,
    /// How long a kernel may keep what it derives from this description before it calls
    /// `describe()` again — see [`ContractCaching`]. Defaults to
    /// [`UntilRebind`](ContractCaching::UntilRebind), a static contract. Set it with
    /// [`contract_max_age`](Self::contract_max_age) or
    /// [`contract_volatile`](Self::contract_volatile).
    ///
    /// Kernel-local, like a representation's golden threads: **never serialized** (a JSON
    /// contract is unchanged by it, and a description read from JSON carries the default),
    /// and **not part of a description's equality** (two doors with one contract are one
    /// contract in the catalog, whatever each may be cached for).
    #[serde(skip)]
    pub contract_caching: ContractCaching,
}

// Every field but `contract_caching`, which is cache policy, not contract (see the field).
// Destructured exhaustively, so a field added later is a compile error here rather than a
// field silently left out of equality.
impl PartialEq for Description {
    fn eq(&self, other: &Self) -> bool {
        let Description {
            id,
            title,
            summary,
            verbs,
            inputs,
            outputs,
            kind,
            requires,
            actions,
            groups,
            contract_caching: _,
        } = self;
        *id == other.id
            && *title == other.title
            && *summary == other.summary
            && *verbs == other.verbs
            && *inputs == other.inputs
            && *outputs == other.outputs
            && *kind == other.kind
            && *requires == other.requires
            && *actions == other.actions
            && *groups == other.groups
    }
}

impl Eq for Description {}

/// How long a kernel may keep what it derives from a [`Description`] before it calls
/// `describe()` again (ledger #1579).
///
/// What it governs is everything the kernel builds from a description and caches: the
/// `Meta` answer in every face (the contract the engine routes named arguments by), the
/// capability floor it enforces before dispatch, and the catalog, action manifold,
/// `urn:kernel:validate` and `urn:kernel:explain` answers built from it. A face built from
/// several descriptions (the catalog, the manifold) is kept no longer than the tightest of
/// them.
///
/// The default, [`UntilRebind`](Self::UntilRebind), is a static contract: kept until the
/// binding set changes, which is a cut of [`BINDINGS_THREAD`](crate::BINDINGS_THREAD).
/// That is right for an endpoint whose contract is its code. It is wrong for one whose
/// contract can change while it stays bound, such as a mount whose peer narrows what it
/// serves: nothing on the mounting side cuts the binding thread when the peer changes, so
/// the old contract would be served for the life of the process. Such an endpoint states a
/// bound. The binding thread still invalidates a bounded answer too.
///
/// ```
/// use std::time::Duration;
/// use ikigai_core::{ContractCaching, Description, Expiry, Time, Verb};
///
/// // A static contract: kept until the bindings change.
/// let fixed = Description::new("upper").verb(Verb::Source);
/// assert_eq!(fixed.contract_caching, ContractCaching::UntilRebind);
/// assert_eq!(fixed.contract_caching.expiry_at(None), Expiry::Never);
///
/// // A mounted contract the peer may change: kept at most five seconds.
/// let mounted = Description::new("upper")
///     .verb(Verb::Source)
///     .contract_max_age(Duration::from_secs(5));
/// let now = Time::from_millis(1_000);
/// assert_eq!(
///     mounted.contract_caching.expiry_at(Some(now)),
///     Expiry::At(Time::from_millis(6_000))
/// );
/// // A deadline needs a clock: a kernel without one keeps nothing.
/// assert_eq!(mounted.contract_caching.expiry_at(None), Expiry::Always);
///
/// // Cache policy is not contract: equality and the JSON wire form ignore it.
/// assert_eq!(fixed, mounted);
/// assert_eq!(
///     serde_json::to_value(&fixed).unwrap(),
///     serde_json::to_value(&mounted).unwrap()
/// );
///
/// // A face built from both is kept no longer than the tighter of the two.
/// assert_eq!(
///     fixed.contract_caching.most_restrictive(mounted.contract_caching),
///     ContractCaching::MaxAge(Duration::from_secs(5))
/// );
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ContractCaching {
    /// Kept until the binding set changes: a static contract. The default.
    #[default]
    UntilRebind,
    /// Kept at most this long, on the kernel's [`Clock`](crate::Clock). A kernel with no
    /// clock cannot honor a deadline, so it keeps nothing (the rule an
    /// [`Expiry::At`] answer follows). `MaxAge(Duration::ZERO)` keeps nothing either.
    MaxAge(Duration),
    /// Never kept: every read derives it from a fresh `describe()`.
    Volatile,
}

impl ContractCaching {
    /// The tighter of two bounds: [`Volatile`](Self::Volatile) over any
    /// [`MaxAge`](Self::MaxAge), the shorter of two ages, and any bound over
    /// [`UntilRebind`](Self::UntilRebind) — how long a face derived from both descriptions
    /// may be kept.
    pub fn most_restrictive(self, other: ContractCaching) -> ContractCaching {
        use ContractCaching::*;
        match (self, other) {
            (Volatile, _) | (_, Volatile) => Volatile,
            (MaxAge(a), MaxAge(b)) => MaxAge(a.min(b)),
            (MaxAge(age), UntilRebind) | (UntilRebind, MaxAge(age)) => MaxAge(age),
            (UntilRebind, UntilRebind) => UntilRebind,
        }
    }

    /// The [`Expiry`] a representation derived from the description takes when it is
    /// computed at `now` (the kernel's clock reading; `None` for a kernel with no clock):
    /// [`Expiry::Never`] until a rebind, `now + max_age` for a bound, [`Expiry::Always`]
    /// for a volatile contract or a bound with no clock to measure it.
    pub fn expiry_at(self, now: Option<Time>) -> Expiry {
        match self {
            ContractCaching::UntilRebind => Expiry::Never,
            ContractCaching::MaxAge(age) => match now {
                Some(now) => {
                    Expiry::At(now.plus_millis(u64::try_from(age.as_millis()).unwrap_or(u64::MAX)))
                }
                None => Expiry::Always,
            },
            ContractCaching::Volatile => Expiry::Always,
        }
    }
}

/// What *kind* of endpoint this is — a first-class type that projects to an RDF class
/// (`ik:Endpoint`, `ik:Transreptor`, …) and lets the kernel select endpoints by role.
///
/// The default, [`Endpoint`](EndpointKind::Endpoint), is a plain endpoint (today's
/// behavior). [`Transreptor`](EndpointKind::Transreptor) marks an endpoint that converts a
/// representation between media types and carries the `from`/`to` types it handles, so a host
/// can find "a transreptor from A to B." More kinds may follow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EndpointKind {
    /// A plain endpoint (`ik:Endpoint`).
    #[default]
    Endpoint,
    /// A transreptor (`ik:Transreptor ⊏ ik:Endpoint`) — converts a representation from one
    /// media type to another, carrying the conversions it supports.
    Transreptor(Transreption),
}

impl EndpointKind {
    /// Whether this is the default plain [`Endpoint`](EndpointKind::Endpoint) kind.
    pub fn is_endpoint(&self) -> bool {
        matches!(self, EndpointKind::Endpoint)
    }
}

/// The media-type conversions a transreptor supports: it can accept any of `from` and
/// produce any of `to`. Used both to type the endpoint in RDF (`ik:transreptsFrom`/`To`,
/// `ik:lossless`) and to select a transreptor for a needed `from → to` conversion.
///
/// Built by [`Description::transreptor`] (and [`Description::lossy`]); the struct is
/// `#[non_exhaustive]` so a later field is never a flag day for a consumer that
/// authored it by literal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Transreption {
    /// The media types it can read.
    pub from: Vec<String>,
    /// The media types it can produce.
    pub to: Vec<String>,
    /// Whether the conversion is a **transreption** in the formal sense — an injective
    /// function on representations that changes form and loses nothing, so the output
    /// represents the SAME resource — or a **projection**: a summary, an extraction, an
    /// embedding, a classification, an arbitrary transform, whose output is a different
    /// resource that has lost information. **Default `true`**: the definition is the
    /// default, and a transreptor that says nothing claims to be one. A projection
    /// declares itself with [`Description::lossy`].
    ///
    /// **This is a declaration, not a checked property** — like `requires` before 0.1.49.
    /// Core cannot decide injectivity from a description; it can only carry the claim and
    /// plan by it: selection routes through lossless edges only unless the caller
    /// consents (`TransreptionPolicy`), and every plan reports which steps are lossy.
    /// Conformance can round-trip a declared-lossless transreptor where a reverse edge
    /// exists; a declaration that is wrong is a module defect the planner cannot see.
    #[serde(default = "lossless_default")]
    pub lossless: bool,
}

/// The `serde` default for [`Transreption::lossless`]: a contract that predates the
/// field (or omits it) is read as lossless, exactly as an authored one that says nothing.
fn lossless_default() -> bool {
    true
}

impl Default for Transreption {
    fn default() -> Self {
        Transreption {
            from: Vec::new(),
            to: Vec::new(),
            lossless: true,
        }
    }
}

impl Transreption {
    /// Whether the conversion is declared lossless (see [`lossless`](Self::lossless)).
    pub fn is_lossless(&self) -> bool {
        self.lossless
    }
}

impl Description {
    /// A description for the endpoint with the given identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Description {
            id: id.into(),
            ..Default::default()
        }
    }

    /// Set the title (builder).
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the summary (builder).
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Declare a supported verb (builder).
    pub fn verb(mut self, verb: Verb) -> Self {
        self.verbs.push(verb);
        self
    }

    /// Declare an input (builder).
    pub fn input(mut self, input: ArgSpec) -> Self {
        self.inputs.push(input);
        self
    }

    /// Declare an output media type (builder).
    pub fn output(mut self, media_type: impl Into<String>) -> Self {
        self.outputs.push(media_type.into());
        self
    }

    /// Declare a capability scope this endpoint requires to be invoked (builder), e.g.
    /// `urn:cap:net:*`. Callable multiple times. Enforced by the kernel before dispatch
    /// (see the `requires` field), and it drives capability-scoped
    /// catalog projection and feasibility pre-checks from the same declaration.
    pub fn requires(mut self, capability: impl Into<String>) -> Self {
        self.requires.push(capability.into());
        self
    }

    /// Set the endpoint kind (builder).
    pub fn kind(mut self, kind: EndpointKind) -> Self {
        self.kind = kind;
        self
    }

    /// Say that what a kernel derives from this description may be kept at most
    /// `max_age` (builder): [`ContractCaching::MaxAge`]. For an endpoint whose contract can
    /// change while it stays bound, such as a mount, which sets its own contract memo's
    /// lifetime here so a mounted name is never described staler than the memo.
    pub fn contract_max_age(mut self, max_age: Duration) -> Self {
        self.contract_caching = ContractCaching::MaxAge(max_age);
        self
    }

    /// Say that nothing a kernel derives from this description may be kept (builder):
    /// [`ContractCaching::Volatile`]. Every `Meta` read and every dispatch then calls
    /// `describe()`, so an endpoint whose `describe()` is not cheap should prefer
    /// [`contract_max_age`](Self::contract_max_age).
    pub fn contract_volatile(mut self) -> Self {
        self.contract_caching = ContractCaching::Volatile;
        self
    }

    /// Declare a relation between the flat inputs (builder): see [`InputGroup`]. Like
    /// the flat `inputs`, it applies to every verb with no explicit [`ActionSpec`].
    pub fn group(mut self, group: InputGroup) -> Self {
        self.groups.push(group);
        self
    }

    /// Declare a per-verb contract (builder). The verb is added to
    /// [`verbs`](Self::verbs) if not already declared.
    pub fn action(mut self, action: ActionSpec) -> Self {
        if !self.verbs.contains(&action.verb) {
            self.verbs.push(action.verb);
        }
        self.actions.push(action);
        self
    }

    /// The normalized per-verb view: one [`ActionSpec`] per declared verb (Meta excluded —
    /// every endpoint answers Meta with this description; it is not a selectable action).
    /// An explicitly declared action wins for its verb; any other verb gets a spec
    /// synthesized from the flat `inputs`/`outputs`/`requires` fields — so the two
    /// authoring forms normalize to the same view, and catalog consumers never know which
    /// form authored an endpoint.
    pub fn action_specs(&self) -> Vec<ActionSpec> {
        self.verbs
            .iter()
            .filter(|v| **v != Verb::Meta)
            .map(|verb| {
                self.actions
                    .iter()
                    .find(|a| a.verb == *verb)
                    .cloned()
                    .unwrap_or_else(|| ActionSpec {
                        verb: *verb,
                        summary: String::new(),
                        inputs: self.inputs.clone(),
                        outputs: self.outputs.clone(),
                        requires: self.requires.clone(),
                        groups: self.groups.clone(),
                    })
            })
            .collect()
    }

    /// The capability scopes a request with `verb` must hold to pass the kernel's
    /// declared-capability floor — the floor the kernel itself enforces before
    /// dispatch, read from this description: the verb's own `requires` when it is
    /// declared; for an undeclared `Exists`, what `Source` requires (or, with no
    /// `Source` either, everything declared); for any other undeclared verb, every
    /// scope declared anywhere; for `Meta`, nothing (the kernel answers it from the
    /// description and never enters the endpoint). Deduplicated, in declaration
    /// order. The same table, row for row, the kernel's floor applies (ledger #750).
    ///
    /// ```
    /// use ikigai_core::{ActionSpec, Description, Verb};
    ///
    /// let calendar = Description::new("calendar")
    ///     .action(ActionSpec::new(Verb::Source).requires("urn:cap:personal:calendar:read"))
    ///     .action(ActionSpec::new(Verb::Sink).requires("urn:cap:personal:calendar:write"));
    /// assert_eq!(calendar.required_scopes(Verb::Sink), ["urn:cap:personal:calendar:write"]);
    /// // An undeclared Exists asks what a reader holds; Meta is never gated.
    /// assert_eq!(calendar.required_scopes(Verb::Exists), ["urn:cap:personal:calendar:read"]);
    /// assert!(calendar.required_scopes(Verb::Meta).is_empty());
    /// // An undeclared mutating verb needs everything the endpoint declares.
    /// assert_eq!(calendar.required_scopes(Verb::Delete).len(), 2);
    /// ```
    pub fn required_scopes(&self, verb: Verb) -> Vec<String> {
        crate::kernel::Floor::of(self).requirements(verb).to_vec()
    }

    /// The scopes in [`required_scopes`](Self::required_scopes) that `capability` does
    /// NOT satisfy — empty exactly when the kernel's floor would admit the request.
    /// Satisfaction is the one predicate the floor, `urn:kernel:actions` and
    /// `urn:kernel:validate` share: a plain scope must be held exactly, and a `…:*`
    /// family scope is satisfied by holding ANY grant under its prefix. So a module
    /// asserting "the grant list I publish satisfies what I declare" asks it here,
    /// rather than keeping a private copy of the predicate (ledger #14).
    ///
    /// ```
    /// use ikigai_core::{Capability, Description, Verb};
    ///
    /// let fetch = Description::new("fetch")
    ///     .verb(Verb::Source)
    ///     .requires("urn:cap:net:*")
    ///     .requires("urn:cap:fetch:read");
    /// let holder = Capability::scoped(["urn:cap:net:example.com"]);
    /// // The family scope is satisfied by a grant under it; the plain one is missing.
    /// assert_eq!(fetch.unsatisfied_scopes(Verb::Source, &holder), ["urn:cap:fetch:read"]);
    /// assert!(fetch.unsatisfied_scopes(Verb::Source, &Capability::root()).is_empty());
    /// ```
    pub fn unsatisfied_scopes(&self, verb: Verb, capability: &crate::Capability) -> Vec<String> {
        crate::kernel::Floor::of(self)
            .requirements(verb)
            .iter()
            .filter(|scope| !crate::select::cap_satisfies(capability, scope))
            .cloned()
            .collect()
    }

    /// Mark this endpoint a **transreptor** that converts representations from any of `from`
    /// to any of `to` (media-type strings) — builder. Sets [`EndpointKind::Transreptor`].
    /// The conversion is declared **lossless** (a transreption in the formal sense: the
    /// output represents the same resource in another form); a projection follows this
    /// with [`lossy`](Self::lossy).
    ///
    /// Three properties of this declaration that a module must design around (ledger
    /// #325), none of which anything checks:
    ///
    /// - **It is a cross product, not a list of pairs.** Selection treats the endpoint
    ///   as handling every `(f, t)` with `f` in `from` and `t` in `to`. Declaring two
    ///   multi-element sides claims every combination, so a type-preserving transform
    ///   (`T → T+gzip` for each `T`) cannot be declared for several types without
    ///   claiming conversions it cannot do. Keep one side a singleton when the pairs are
    ///   not all real (`ikigai-compress`: payload types → `application/gzip`, and the
    ///   reverse on its own endpoint), or bind one endpoint per pair.
    /// - **There is no wildcard.** A transform defined for every media type enumerates
    ///   the ones it expects to be asked about, and that list can go stale.
    /// - **The pivot assumes the pair decides the transform.** When no direct hop
    ///   exists, [`select_transreptor`](crate::select_transreptor) plans
    ///   `from → text/turtle → to`. A transform whose output type depends on the payload
    ///   rather than the pair (decompression) can be planned into that pivot and label a
    ///   payload that is not Turtle as Turtle.
    ///
    /// ```
    /// use ikigai_core::{select_transreptor, Description, EndpointSpace, Exact, FnEndpoint};
    /// use ikigai_core::{ArgSpec, ReprType, Representation};
    ///
    /// let both = Description::new("two-by-two")
    ///     .transreptor(["text/a", "text/b"], ["text/x", "text/y"])
    ///     .input(ArgSpec::new("content"));
    /// let space = EndpointSpace::new().bind(
    ///     Exact::new("urn:demo:two-by-two"),
    ///     FnEndpoint::new("two-by-two", |_| {
    ///         Ok(Representation::new(ReprType::new("text/x"), Vec::new()))
    ///     })
    ///     .with_description(both),
    /// );
    /// // Declared as two lists, planned as four pairs: `text/b → text/x` is offered
    /// // whether or not the endpoint can do it.
    /// for (from, to) in [("text/a", "text/x"), ("text/a", "text/y"), ("text/b", "text/x"), ("text/b", "text/y")] {
    ///     let plan = select_transreptor(&space, from, to).unwrap();
    ///     assert_eq!(plan[0].endpoint, "urn:demo:two-by-two");
    /// }
    /// ```
    pub fn transreptor(
        mut self,
        from: impl IntoIterator<Item = impl Into<String>>,
        to: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.kind = EndpointKind::Transreptor(Transreption {
            from: from.into_iter().map(Into::into).collect(),
            to: to.into_iter().map(Into::into).collect(),
            lossless: true,
        });
        self
    }

    /// Declare the transreption **lossy** — a *projection* (a summary, an extraction, an
    /// embedding, a classification, an arbitrary transform): its output is a different
    /// resource that has lost information, not the same resource in another form.
    /// Builder; follows [`transreptor`](Self::transreptor). Selection will not route
    /// through this endpoint unless the caller consents, and a plan that does reports
    /// the step as lossy (see [`Transreption::lossless`]).
    ///
    /// The word is the paper's negation: Definition 8 makes a transreption injective,
    /// and "lossy" is the one-word way to say this edge is not.
    ///
    /// # Panics
    ///
    /// If the endpoint is not a transreptor — a projection is declared by its
    /// conversions first (`transreptor(from, to).lossy()`); there is nothing to mark
    /// lossy on a plain endpoint, and a silent no-op here would leave a projection
    /// declared lossless, the very defect the flag exists to name.
    ///
    /// ```
    /// use ikigai_core::Description;
    ///
    /// let lift = Description::new("summarize")
    ///     .transreptor(["text/plain"], ["text/x-summary"])
    ///     .lossy();
    /// assert!(!lift.transreption().unwrap().is_lossless());
    ///
    /// // Saying nothing claims the definition.
    /// let gzip = Description::new("gzip").transreptor(["text/plain"], ["application/gzip"]);
    /// assert!(gzip.transreption().unwrap().is_lossless());
    /// ```
    pub fn lossy(mut self) -> Self {
        match &mut self.kind {
            EndpointKind::Transreptor(t) => t.lossless = false,
            EndpointKind::Endpoint => panic!(
                "`Description::lossy` on `{}`, which is not a transreptor: declare the \
                 conversions first (`.transreptor(from, to).lossy()`)",
                self.id
            ),
        }
        self
    }

    /// The transreption this endpoint supports, if it is a transreptor — for the vocabulary
    /// projection and for transreptor selection.
    pub fn transreption(&self) -> Option<&Transreption> {
        match &self.kind {
            EndpointKind::Transreptor(t) => Some(t),
            EndpointKind::Endpoint => None,
        }
    }

    /// Check that every identifier this description projects into an RDF IRI position is
    /// safe to write there — the [`id`](Self::id), each input `name` and `class`, and each
    /// `requires` scope, across the flat fields and every explicit [`ActionSpec`].
    ///
    /// **Opt-in, and STRICT.** [`Description::new`] validates nothing and never will: it has
    /// hundreds of call sites and a fallible constructor would be a flag day. Nor does this
    /// stand between a bad name and a broken graph: the emitters never write an unparseable
    /// document, whatever a name holds. A NAME (the id, an input name) is percent-encoded by
    /// [`encode_name`](crate::encode_name), and an authored `class` or `requires` IRI is
    /// escaped ([`escape_iri_fragment`](crate::escape_iri_fragment)) and degraded to a
    /// literal when it is still not an IRI. So this is for a host that would rather refuse
    /// an endpoint at bind time than serve it under an encoded name.
    ///
    /// Refused, each as [`InvalidArgument`](crate::Error::InvalidArgument) whose `name` is
    /// the field (`id`, `input name`, `input class`, `requires`):
    ///
    /// - any field holding what a Turtle `IRIREF` cannot carry (`<>"{}|^` `` ` `` `\`, a
    ///   space, a control character): an id that would close the IRI early is an injection
    ///   attempt or a typo, never a name anyone meant;
    /// - a NAME holding a code point outside RFC 3987's `ucschar` (DEL, a C1 control, private
    ///   use, a noncharacter), named by its `U+XXXX`: the emitters encode it, and here it is
    ///   refused as an authoring mistake (ledger #1351, #1361);
    /// - per contract (each explicit action, each verb the flat form stands for), a
    ///   relation the contract cannot honor (`group`, `needs`): an [`InputGroup`] with fewer
    ///   than two members, a member that is not a declared by-value input, is declared
    ///   required or has a default, or a `pipe` that is not a member (or, on a Sink or
    ///   Delete, is not `content`); a `needs` naming an undeclared input or itself, or on a
    ///   defaulted input.
    ///
    /// `[` and `]` are admitted: an id may be a full IRI with an IPv6 host, and a name's
    /// brackets are encoded (`http://[::1]/x` is `urn:ikigai:endpoint:http://%5B::1%5D/x`).
    /// So are `#` and `?`, which a name's encoding keeps inside the path (`a#b` is
    /// `urn:ikigai:endpoint:a%23b`, ledger #1390).
    ///
    /// ```
    /// use ikigai_core::{ArgSpec, Description, Error};
    ///
    /// assert!(Description::new("tag-suggest").input(ArgSpec::new("book")).validate().is_ok());
    ///
    /// // An id that would close the IRI early and inject triples into every Meta response.
    /// let err = Description::new("evil> ; a <urn:x> . <urn:y").validate().unwrap_err();
    /// assert!(matches!(&err, Error::InvalidArgument { name, .. } if name == "id"), "{err}");
    ///
    /// // Nested fields are checked too — an input name is an IRI segment as well.
    /// let err = Description::new("ok").input(ArgSpec::new("a b")).validate().unwrap_err();
    /// assert!(matches!(&err, Error::InvalidArgument { name, .. } if name == "input name"));
    ///
    /// // A name no IRI can carry raw: refused, naming the field and the code point.
    /// let err = Description::new("ok").input(ArgSpec::new("a\u{85}b")).validate().unwrap_err();
    /// assert!(matches!(&err, Error::InvalidArgument { name, .. } if name == "input name"));
    /// assert!(err.to_string().contains("U+0085"), "{err}");
    ///
    /// // Brackets are encoded in a name, so an id that is a full IRI with an IPv6 host is fine.
    /// assert!(Description::new("http://[::1]/x").input(ArgSpec::new("a[0]")).validate().is_ok());
    ///
    /// // Ordinary Unicode is a ucschar, and still a name.
    /// assert!(Description::new("naïve").input(ArgSpec::new("書名")).validate().is_ok());
    /// ```
    pub fn validate(&self) -> crate::Result<()> {
        let flat = self
            .inputs
            .iter()
            .flat_map(ArgSpec::iri_positions)
            .chain(self.requires.iter().map(|c| ("requires", c.as_str())));
        let positions = std::iter::once(("id", self.id.as_str()))
            .chain(flat)
            .chain(self.actions.iter().flat_map(ActionSpec::iri_positions));
        for (field, value) in positions {
            if !crate::is_iri_safe(value) {
                return Err(crate::Error::InvalidArgument {
                    name: field.to_string(),
                    detail: format!(
                        "endpoint `{}`: {field} `{}` is not safe in an IRI position \
                         (an RDF IRIREF cannot contain <>\"{{}}|^`\\, space or a control \
                         character)",
                        self.id.escape_debug(),
                        value.escape_debug()
                    ),
                });
            }
        }
        // A NAME holding a code point the IRI grammar refuses is encoded by every emitter
        // (`encode_name`, ledger #1361), so the documents parse; the strict check refuses it
        // anyway, as an authoring mistake (ledger #1351). An authored `class` or `requires`
        // IRI is not a name: the emitters parse it and degrade one that is not an IRI to a
        // literal.
        let names = std::iter::once(("id", self.id.as_str())).chain(
            self.inputs
                .iter()
                .chain(self.actions.iter().flat_map(|a| a.inputs.iter()))
                .map(|input| ("input name", input.name.as_str())),
        );
        for (field, value) in names {
            if let Some(c) = value.chars().find(|&c| crate::iri::is_non_ucschar(c)) {
                return Err(crate::Error::InvalidArgument {
                    name: field.to_string(),
                    detail: format!(
                        "endpoint `{}`: {field} `{}` contains U+{:04X}, which no IRI can carry \
                         (DEL, a C1 control, private use or a noncharacter); the emitters \
                         would percent-encode it, and a strict check refuses it",
                        self.id.escape_debug(),
                        value.escape_debug(),
                        u32::from(c)
                    ),
                });
            }
        }
        // The relations between inputs, per contract: each explicit action and each
        // verb the flat form stands for (or the flat form alone, when it declares no verb).
        let specs = self.action_specs();
        if specs.is_empty() {
            let flat = ActionSpec {
                inputs: self.inputs.clone(),
                groups: self.groups.clone(),
                ..ActionSpec::new(Verb::Source)
            };
            validate_relations(&self.id, None, &flat)?;
        }
        for spec in &specs {
            validate_relations(&self.id, Some(spec.verb), spec)?;
        }
        Ok(())
    }
}

/// The relation rules of one contract ([`Description::validate`]): each refused as
/// [`InvalidArgument`](crate::Error::InvalidArgument) naming `group` or `needs`.
fn validate_relations(id: &str, verb: Option<Verb>, spec: &ActionSpec) -> crate::Result<()> {
    let refuse = |field: &str, detail: String| {
        Err(crate::Error::InvalidArgument {
            name: field.to_string(),
            detail: format!("endpoint `{}`: {detail}", id.escape_debug()),
        })
    };
    let input = |name: &str| spec.inputs.iter().find(|i| i.name == name);
    for group in &spec.groups {
        let what = format!(
            "{} group of {}",
            group.kind.word(),
            group.members.join(", ")
        );
        if group.members.len() < 2 {
            return refuse("group", format!("the {what} needs at least two members"));
        }
        for member in &group.members {
            let Some(arg) = input(member) else {
                return refuse(
                    "group",
                    format!("the {what} names `{member}`, which is not a declared input"),
                );
            };
            if arg.source == InputSource::Binding {
                return refuse(
                    "group",
                    format!("the {what} names `{member}`, a binding (always present)"),
                );
            }
            if arg.required {
                return refuse(
                    "group",
                    format!(
                        "the {what} names `{member}`, which is declared required: a member is \
                         declared optional, so a reader that predates groups never refuses a \
                         valid call"
                    ),
                );
            }
            if arg.default.is_some() {
                return refuse(
                    "group",
                    format!(
                        "the {what} names `{member}`, which has a default: a default would \
                         always give it"
                    ),
                );
            }
        }
        if let Some(pipe) = &group.pipe {
            if !group.has(pipe) {
                return refuse(
                    "group",
                    format!("the {what} pipes into `{pipe}`, which is not a member"),
                );
            }
            if matches!(verb, Some(Verb::Sink | Verb::Delete)) && pipe != "content" {
                return refuse(
                    "group",
                    format!(
                        "the {what} pipes into `{pipe}`, but a {verb:?}'s pipe always lands in \
                         `content`",
                        verb = verb.expect("matched")
                    ),
                );
            }
        }
    }
    for arg in &spec.inputs {
        for needed in &arg.needs {
            if input(needed).is_none() {
                return refuse(
                    "needs",
                    format!(
                        "input `{}` needs `{needed}`, which is not a declared input",
                        arg.name
                    ),
                );
            }
            if needed == &arg.name {
                return refuse("needs", format!("input `{}` needs itself", arg.name));
            }
        }
        if !arg.needs.is_empty() && arg.default.is_some() {
            return refuse(
                "needs",
                format!(
                    "input `{}` has a default and needs another input: the kernel's default \
                     would make the dependency unconditional",
                    arg.name
                ),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_specs_normalize_both_authoring_forms() {
        // flat single-verb: the flat fields ARE the (synthesized) action
        let flat = Description::new("to-upper")
            .verb(Verb::Source)
            .verb(Verb::Meta) // Meta is universal, never a selectable action
            .input(ArgSpec::new("in"))
            .output("text/plain")
            .requires("cap:demo");
        let specs = flat.action_specs();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].verb, Verb::Source);
        assert_eq!(specs[0].inputs[0].name, "in");
        assert_eq!(specs[0].requires, vec!["cap:demo".to_string()]);

        // explicit action WINS for its verb; other declared verbs synthesize
        let mixed = Description::new("calendar")
            .verb(Verb::Source)
            .input(ArgSpec::new("calendar").optional()) // flat = Source's shape
            .action(
                ActionSpec::new(Verb::Sink)
                    .requires("urn:cap:personal:calendar:write")
                    .input(ArgSpec::new("start")),
            );
        assert!(
            mixed.verbs.contains(&Verb::Sink),
            ".action() declares its verb"
        );
        let specs = mixed.action_specs();
        assert_eq!(specs.len(), 2);
        let sink = specs.iter().find(|a| a.verb == Verb::Sink).unwrap();
        assert_eq!(sink.inputs[0].name, "start", "explicit action wins");
        let source = specs.iter().find(|a| a.verb == Verb::Source).unwrap();
        assert_eq!(source.inputs[0].name, "calendar", "synthesized from flat");
    }

    #[test]
    fn builds_a_description() {
        let d = Description::new("to-upper")
            .title("Upper-case")
            .verb(Verb::Source)
            .input(ArgSpec::new("in").summary("the string").optional())
            .output("text/plain");
        assert_eq!(d.id, "to-upper");
        assert_eq!(d.verbs, vec![Verb::Source]);
        assert_eq!(d.inputs[0].name, "in");
        assert!(!d.inputs[0].required);
    }

    #[test]
    fn inputs_default_to_arguments_and_can_be_bindings() {
        let arg = ArgSpec::new("in");
        assert_eq!(arg.source, InputSource::Argument);
        let bound = ArgSpec::new("message").binding();
        assert_eq!(bound.source, InputSource::Binding);
    }

    #[test]
    fn serde_round_trip() {
        let d = Description::new("x")
            .verb(Verb::Meta)
            .input(ArgSpec::new("message").binding());
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Description>(&json).unwrap(), d);
    }

    #[test]
    fn kind_defaults_to_endpoint_and_is_omitted_from_json() {
        let d = Description::new("to-upper").output("text/plain");
        assert_eq!(d.kind, EndpointKind::Endpoint);
        assert!(d.transreption().is_none());
        // A plain endpoint's JSON carries no `kind` key — the engine's contract is unchanged.
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains("kind"), "{json}");
        // …and JSON without a `kind` still deserializes (back-compat).
        let d2: Description = serde_json::from_str(&json).unwrap();
        assert_eq!(d2.kind, EndpointKind::Endpoint);
    }

    #[test]
    fn transreptor_builder_records_its_conversions() {
        let d = Description::new("rdf-transrept")
            .verb(Verb::Source)
            .transreptor(
                ["text/turtle", "application/rdf+xml"],
                ["text/turtle", "text/html"],
            );
        let t = d.transreption().expect("is a transreptor");
        assert_eq!(t.from, vec!["text/turtle", "application/rdf+xml"]);
        assert_eq!(t.to, vec!["text/turtle", "text/html"]);
        assert!(!d.kind.is_endpoint());
        // Saying nothing claims the definition: a transreptor is lossless by default.
        assert!(t.is_lossless());
        // The transreptor kind round-trips through serde, and the JSON face carries the
        // declaration explicitly (a client reads a claim, not an absence).
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("transreptor"), "{json}");
        assert!(json.contains("\"lossless\":true"), "{json}");
        assert_eq!(serde_json::from_str::<Description>(&json).unwrap(), d);
    }

    #[test]
    fn a_projection_declares_itself_lossy() {
        let d = Description::new("lift")
            .transreptor(["text/markdown"], ["text/turtle"])
            .lossy();
        let t = d.transreption().expect("still a transreptor");
        assert!(!t.is_lossless());
        assert_eq!(t.from, vec!["text/markdown"]);
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"lossless\":false"), "{json}");
        assert_eq!(serde_json::from_str::<Description>(&json).unwrap(), d);
    }

    #[test]
    fn a_contract_without_the_flag_reads_as_lossless() {
        // A JSON contract from a kernel that predates the field (or a hand-written one
        // that omits it) is read exactly as an authored declaration that says nothing.
        let with =
            serde_json::to_string(&Description::new("rdf").transreptor(["a/b"], ["c/d"])).unwrap();
        let without = with.replace(",\"lossless\":true", "");
        assert_ne!(with, without, "the field was present to strip: {with}");
        let d: Description = serde_json::from_str(&without).unwrap();
        assert!(d.transreption().unwrap().is_lossless());
        assert!(Transreption::default().lossless);
    }

    #[test]
    #[should_panic(expected = "not a transreptor")]
    fn lossy_on_a_plain_endpoint_is_refused_not_ignored() {
        let _ = Description::new("plain").lossy();
    }

    #[test]
    fn input_class_is_optional_and_omitted_when_absent() {
        // No class → no `class` key in the input's JSON (existing contract unchanged).
        let plain = ArgSpec::new("content");
        assert_eq!(plain.class, None);
        let json = serde_json::to_string(&plain).unwrap();
        assert!(!json.contains("class"), "{json}");

        // With a class → recorded and round-trips.
        let typed = ArgSpec::new("who").class("https://schema.org/Person");
        assert_eq!(typed.class.as_deref(), Some("https://schema.org/Person"));
        assert_eq!(
            serde_json::from_str::<ArgSpec>(&serde_json::to_string(&typed).unwrap()).unwrap(),
            typed
        );
    }

    #[test]
    fn requires_is_empty_by_default_and_omitted_from_json() {
        let d = Description::new("to-upper").output("text/plain");
        assert!(d.requires.is_empty());
        // A no-authority endpoint carries no `requires` key — JSON contract unchanged.
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains("requires"), "{json}");

        // Declared capability scopes accumulate and round-trip.
        let gated = Description::new("httpGet")
            .verb(Verb::Source)
            .requires("cap:net")
            .requires("cap:fs:read");
        assert_eq!(gated.requires, vec!["cap:net", "cap:fs:read"]);
        assert_eq!(
            serde_json::from_str::<Description>(&serde_json::to_string(&gated).unwrap()).unwrap(),
            gated
        );
    }

    #[test]
    fn a_plain_descriptions_json_is_byte_identical_to_before_these_fields() {
        // Belt-and-suspenders: an endpoint using neither new field serializes with exactly
        // the pre-existing keys — so the engine's machine contract is untouched.
        let d = Description::new("to-upper")
            .title("Upper-case")
            .verb(Verb::Source)
            .input(ArgSpec::new("in"))
            .output("text/plain");
        let json = serde_json::to_string(&d).unwrap();
        assert!(
            !json.contains("class") && !json.contains("requires"),
            "{json}"
        );
    }

    /// Ledger #15: the relations a contract states must be ones it can honor.
    #[test]
    fn validate_refuses_a_relation_the_contract_cannot_honor() {
        let both = || {
            Description::new("lift")
                .verb(Verb::Source)
                .input(ArgSpec::new("content").optional())
                .input(ArgSpec::new("in").optional())
        };
        both()
            .group(InputGroup::exactly_one(["content", "in"]).pipe("content"))
            .validate()
            .expect("a piped-or-named intake validates");
        let refused: Vec<(&str, Description)> = vec![
            ("group", both().group(InputGroup::exactly_one(["content"]))),
            (
                "group",
                both().group(InputGroup::exactly_one(["content", "nope"])),
            ),
            (
                "group",
                Description::new("x")
                    .verb(Verb::Source)
                    .input(ArgSpec::new("a"))
                    .input(ArgSpec::new("b").optional())
                    .group(InputGroup::at_least_one(["a", "b"])),
            ),
            (
                "group",
                Description::new("x")
                    .verb(Verb::Source)
                    .input(ArgSpec::new("a").default_value("1"))
                    .input(ArgSpec::new("b").optional())
                    .group(InputGroup::at_most_one(["a", "b"])),
            ),
            (
                "group",
                Description::new("x")
                    .verb(Verb::Source)
                    .input(ArgSpec::new("a").binding())
                    .input(ArgSpec::new("b").optional())
                    .group(InputGroup::exactly_one(["a", "b"])),
            ),
            (
                "group",
                both().group(InputGroup::exactly_one(["content", "in"]).pipe("other")),
            ),
            (
                "group",
                Description::new("x").action(
                    ActionSpec::new(Verb::Sink)
                        .input(ArgSpec::new("content").optional())
                        .input(ArgSpec::new("in").optional())
                        .group(InputGroup::exactly_one(["content", "in"]).pipe("in")),
                ),
            ),
            (
                "needs",
                Description::new("x")
                    .verb(Verb::Source)
                    .input(ArgSpec::new("a").optional().needs(["missing"])),
            ),
            (
                "needs",
                Description::new("x")
                    .verb(Verb::Source)
                    .input(ArgSpec::new("a").optional().needs(["a"])),
            ),
            (
                "needs",
                Description::new("x")
                    .verb(Verb::Source)
                    .input(ArgSpec::new("a").default_value("1").needs(["b"]))
                    .input(ArgSpec::new("b").optional()),
            ),
        ];
        for (field, d) in refused {
            let err = d.validate().expect_err("must be refused");
            assert!(
                matches!(&err, crate::Error::InvalidArgument { name, .. } if name == field),
                "expected `{field}`: {err:?}"
            );
        }
    }

    #[test]
    fn groups_and_needs_are_omitted_from_json_when_absent_and_round_trip() {
        let plain = Description::new("x")
            .verb(Verb::Source)
            .input(ArgSpec::new("a"));
        let json = serde_json::to_string(&plain).unwrap();
        assert!(
            !json.contains("groups") && !json.contains("needs"),
            "{json}"
        );
        let grouped = Description::new("cut")
            .verb(Verb::Sink)
            .input(ArgSpec::new("thread").optional().needs(["content"]))
            .input(ArgSpec::new("content").optional())
            .group(InputGroup::exactly_one(["thread", "content"]).pipe("content"));
        let json = serde_json::to_value(&grouped).unwrap();
        assert_eq!(
            json["groups"],
            serde_json::json!([
                {"kind": "exactly-one", "members": ["content", "thread"], "pipe": "content"}
            ])
        );
        assert_eq!(json["inputs"][0]["needs"], serde_json::json!(["content"]));
        assert_eq!(
            serde_json::from_value::<Description>(json).unwrap(),
            grouped
        );
        // The flat groups ride into every synthesized action.
        assert_eq!(grouped.action_specs()[0].groups, grouped.groups);
    }

    #[test]
    fn validate_reaches_every_field_an_emitter_puts_in_an_iri_position() {
        // The predicate must cover the NESTED positions too — an explicit ActionSpec's
        // inputs get their own IRI nodes, and a spec that only checked the flat fields
        // would pass a description whose Turtle still needed encoding.
        let base = || {
            Description::new("cal").action(
                ActionSpec::new(Verb::Sink)
                    .requires("urn:cap:cal:write")
                    .input(
                        ArgSpec::new("start").class("http://www.w3.org/2001/XMLSchema#dateTime"),
                    ),
            )
        };
        base().validate().expect("a real endpoint validates");

        let cases: [(&str, Description); 6] = [
            ("id", Description::new("ca>l")),
            (
                "input name",
                Description::new("cal").input(ArgSpec::new("a b")),
            ),
            (
                "input class",
                Description::new("cal").input(ArgSpec::new("a").class("urn:x>y")),
            ),
            ("requires", Description::new("cal").requires("urn:cap:a b")),
            (
                "input name",
                Description::new("cal")
                    .action(ActionSpec::new(Verb::Sink).input(ArgSpec::new("a>b"))),
            ),
            (
                "requires",
                Description::new("cal").action(ActionSpec::new(Verb::Sink).requires("urn:cap:a>b")),
            ),
        ];
        for (field, d) in cases {
            // One variant for every refusal (ledger #1361): the IRIREF one was
            // `Error::Endpoint` before, the code-point one `InvalidArgument`.
            let err = d.validate().expect_err("must be refused");
            assert!(
                matches!(&err, crate::Error::InvalidArgument { name, .. } if name == field),
                "expected `{field}` to be blamed: {err:?}"
            );
        }
    }
}
