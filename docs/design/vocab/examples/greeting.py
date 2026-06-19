# The lightweight end of the weight gradient: no manifest file at all.
# A decorated function becomes a resolvable, self-describing, cacheable,
# capability-gated resource. ikigai *infers* the manifest from the signature
# (see greeting.inferred.ttl) — it lands in the same vocabulary as the authored
# personal.module.* files.
#
# NOTE: this file is a COMPONENT OF the personal module (same authority, same
# signer) — not a stray third-party file that "drops in" and binds into
# urn:personal:. Binding into a space is an act of host authority, not a claim a
# module makes by naming a prefix; see ../README.md "Binding: who may publish
# into a space".

from ikigai import endpoint, source


@endpoint                                   # -> urn:personal:greeting  (IRI from the fn name)
def greeting(name: str) -> str:
    # A cross-resource pull: capability-gated and cached, serviced by the kernel.
    # In production this routes through the runtime's `issue` import; in a
    # debugger it routes through an embedded kernel (fixtures or live).
    style = source("urn:personal:prefs:style")          # e.g. "formal"
    return f"Good day, {name}." if style == "formal" else f"Hey {name}!"
