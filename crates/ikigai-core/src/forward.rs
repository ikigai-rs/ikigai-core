//! **A forwarding endpoint is described by its TARGET** (ledger #2513, #2065, #2080).
//!
//! A view ([`ResolveAt::Root`](crate::ResolveAt::Root)) and a named request forward every
//! request they get to another name, from the root, under the caller's own capability. Its
//! own [`describe`](crate::Endpoint::describe) cannot say what it forwards to — `describe()`
//! takes no invocation, and the target lives in the root the view is part of — so before
//! this, `Meta` of a view showed a generic forwarder with no inputs and no outputs, the
//! catalog never listed one, and the manifold never offered one. Worse, an engine routing
//! by that empty contract sent `in=hello` to a view as a positional VALUE, which the fail-open
//! rule then handed whole to `in`: `IN=HELLO`, a wrong answer rather than a refusal.
//!
//! So a forwarding endpoint says what it forwards to ([`Endpoint::forwards`]), and the
//! kernel follows it wherever it describes anything, through ONE helper applied at two
//! places: [`probe`](crate::select) (which every `entries → Meta → describe` walk uses —
//! the catalog, `urn:kernel:actions`, `urn:kernel:validate`, [`Kernel::describe`]) and the
//! `Meta` arm of the issue path. The projection is the target's contract:
//!
//! - **id**: the bound name (the rule's logical name, or its template);
//! - **verbs**: the bound verb with `Exists`, else the target's; `Meta` always;
//! - **inputs**: the target's for that verb, the fixed ones LISTED and marked
//!   [`fixed`](crate::ArgSpec::fixed) (never omitted: an engine must name them, so a caller
//!   who gives one reaches the refusal), a template's captures as required bindings, and
//!   the binding's declared inputs in place of the target's of the same name;
//! - **outputs, failures, requires, existence sensitivity**: the target's for that verb —
//!   declared = enforced holds transitively, since the forward is checked at the target;
//! - a target that resolves to nothing: the forwarder's own contract, saying so in its
//!   summary, never an error that empties the catalog.
//!
//! Computed live from the probe, so it is never staler than the description memo already
//! allows, and that memo is cut by `urn:kernel:bindings` (a target rebind, a dynamic
//! space's swap). The capability floor is NOT projected: the forwarder declares nothing
//! and grants nothing, and the target's floor refuses at the target, as it always did.
//!
//! [`Endpoint::forwards`]: crate::Endpoint::forwards
//! [`Kernel::describe`]: crate::Kernel::describe

use std::sync::Arc;

use async_trait::async_trait;

use crate::alias::{FixedArgument, FixedValue};
use crate::describe::{ArgSpec, Description};
use crate::endpoint::{Endpoint, Invocation};
use crate::error::Result;
use crate::iri::Iri;
use crate::repr::Representation;
use crate::topology::Topology;
use crate::verb::Verb;

/// How many forwards a description follows before it stops: a view onto a view onto … is
/// described through at most this many, and a cycle stops here, described as unresolved.
pub(crate) const MAX_FORWARDS: usize = 8;

/// What a forwarding endpoint forwards to (ledger #2513): what
/// [`Endpoint::forwards`](crate::Endpoint::forwards) answers, and what the kernel reads to
/// describe the endpoint by its target's contract.
///
/// `#[non_exhaustive]`: build one with [`new`](Self::new) and the builders.
///
/// ```
/// use ikigai_core::{FixedArgument, Forward, Iri, Verb};
///
/// let forward = Forward::new("urn:example:count", Iri::parse("urn:example:sparql").unwrap())
///     .verb(Verb::Source)
///     .fixing(FixedArgument::value("graph", "urn:example:graph"));
/// assert_eq!(forward.target.as_str(), "urn:example:sparql");
/// assert_eq!(forward.fixed[0].value.to_string(), "urn:example:graph");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Forward {
    /// The name the forwarder is bound under: an exact name, or a URI template. The
    /// projected description's id.
    pub name: String,
    /// The name it forwards to.
    pub target: Iri,
    /// The one verb it forwards beside `Exists`, or `None` for every verb.
    pub verb: Option<Verb>,
    /// The arguments it fixes, in the order it applies them. A capture names a variable
    /// of [`name`](Self::name)'s template.
    pub fixed: Vec<FixedArgument>,
    /// The variables its template name captures, in order: required bindings of the
    /// projected contract.
    pub captures: Vec<String>,
    /// The inputs it declares itself: the caller's to give, defaults filled.
    pub inputs: Vec<ArgSpec>,
}

impl Forward {
    /// A forward of every verb from `name` to `target`, fixing nothing.
    pub fn new(name: impl Into<String>, target: Iri) -> Self {
        Forward {
            name: name.into(),
            target,
            verb: None,
            fixed: Vec::new(),
            captures: Vec::new(),
            inputs: Vec::new(),
        }
    }

    /// Bind the one verb forwarded (builder).
    pub fn verb(mut self, verb: Verb) -> Self {
        self.verb = Some(verb);
        self
    }

    /// Fix one argument (builder).
    pub fn fixing(mut self, argument: FixedArgument) -> Self {
        self.fixed.push(argument);
        self
    }

    /// Name a variable the bound name's template captures (builder).
    pub fn capturing(mut self, var: impl Into<String>) -> Self {
        self.captures.push(var.into());
        self
    }

    /// Declare one input of the name itself (builder).
    pub fn input(mut self, input: ArgSpec) -> Self {
        self.inputs.push(input);
        self
    }
}

/// The contract `forward` presents: `target`'s, projected as the module docs say, or
/// `own` (the forwarder's) when the target resolved to nothing.
pub(crate) fn project(
    forward: &Forward,
    own: Description,
    target: Option<&Description>,
) -> Description {
    let Some(target) = target else {
        let mut own = own;
        own.summary.push_str(&format!(
            " Its target <{}> resolves to nothing here, so this is the forwarder's own \
             contract.",
            forward.target.as_str()
        ));
        return own;
    };
    let fixed_names: Vec<&str> = forward.fixed.iter().map(|a| a.name.as_str()).collect();
    let mut words: Vec<String> = Vec::new();
    for argument in &forward.fixed {
        words.push(format!("{}={}", argument.name, argument.value));
    }
    let verb = forward.verb.map_or(String::new(), |v| format!("{v:?} "));
    let mut summary = format!(
        "{} = {verb}{}{}{}",
        forward.name,
        forward.target.as_str(),
        if words.is_empty() { "" } else { " " },
        words.join(" ")
    );
    if !target.summary.is_empty() {
        summary.push_str(". ");
        summary.push_str(&target.summary);
    }
    let title = if target.title.is_empty() {
        forward.name.clone()
    } else {
        target.title.clone()
    };
    let mut description = Description::new(forward.name.clone())
        .title(title)
        .summary(summary);
    let specs = target.action_specs();
    let verbs: Vec<Verb> = match forward.verb {
        Some(bound) => vec![bound, Verb::Exists],
        None => specs
            .iter()
            .map(|s| s.verb)
            .filter(|v| *v != Verb::Meta)
            .collect(),
    };
    for verb in verbs {
        // An undeclared `Exists` runs what a reader runs (the floor's table), so it is
        // described by the target's `Source`.
        let Some(mut spec) = specs.iter().find(|s| s.verb == verb).cloned().or_else(|| {
            (verb == Verb::Exists)
                .then(|| specs.iter().find(|s| s.verb == Verb::Source).cloned())
                .flatten()
        }) else {
            continue;
        };
        spec.verb = verb;
        for argument in &forward.fixed {
            // A capture whose name is its own variable is stated once: as the binding.
            if matches!(&argument.value, FixedValue::Capture(var) if *var == argument.name) {
                continue;
            }
            let spelled = argument.value.to_string();
            match spec.inputs.iter_mut().find(|i| i.name == argument.name) {
                Some(input) => {
                    input.fixed = Some(spelled);
                    input.required = false;
                    // A default would invite a caller to think it applies.
                    input.default = None;
                }
                None => spec
                    .inputs
                    .push(ArgSpec::new(argument.name.clone()).fixed_to(spelled)),
            }
        }
        for var in &forward.captures {
            spec.inputs.retain(|i| i.name != *var);
            spec.inputs.push(
                ArgSpec::new(var.clone())
                    .binding()
                    .summary(format!("captured by the name {}", forward.name)),
            );
        }
        for input in &forward.inputs {
            spec.inputs.retain(|i| i.name != input.name);
            spec.inputs.push(input.clone());
        }
        // A relation between inputs one of which the binding fixes no longer holds for
        // the caller, who gives none of the fixed ones: it is left out, never misstated.
        spec.groups
            .retain(|g| !g.members.iter().any(|m| fixed_names.contains(&m.as_str())));
        description = description.verb(verb).action(spec);
    }
    description = description.verb(Verb::Meta);
    description.outputs = target.outputs.clone();
    description.existence_sensitive = target.existence_sensitive;
    description.contract_caching = own
        .contract_caching
        .most_restrictive(target.contract_caching);
    description
}

/// A forwarder, described by its projection: what a walk that describes it hands on in
/// place of the forwarder itself. Everything else is the forwarder's.
pub(crate) struct Projected {
    pub(crate) inner: Arc<dyn Endpoint>,
    pub(crate) description: Description,
}

#[async_trait]
impl Endpoint for Projected {
    async fn invoke(&self, inv: &Invocation<'_>) -> Result<Representation> {
        self.inner.invoke(inv).await
    }

    async fn invoke_body(&self, inv: &Invocation<'_>) -> Result<Representation<crate::Body>> {
        self.inner.invoke_body(inv).await
    }

    fn name(&self) -> &str {
        self.inner.name()
    }

    fn describe(&self) -> Description {
        self.description.clone()
    }

    fn forwards(&self) -> Option<Forward> {
        self.inner.forwards()
    }

    fn confinement(&self) -> Option<Topology> {
        self.inner.confinement()
    }
}
