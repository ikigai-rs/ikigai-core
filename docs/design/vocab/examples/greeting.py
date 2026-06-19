# The lightweight end of the weight gradient: no manifest file at all.
# A decorated function becomes a resolvable, self-describing, cacheable,
# capability-gated resource. ikigai *infers* the manifest from the signature
# (see greeting.inferred.ttl) — it lands in the same vocabulary as the authored
# personal.module.* files.

from ikigai import endpoint, source


@endpoint                                   # -> urn:personal:greeting  (IRI from the fn name)
def greeting(name: str) -> str:
    # A cross-resource pull: capability-gated and cached, serviced by the kernel.
    # In production this routes through the runtime's `issue` import; in a
    # debugger it routes through an embedded kernel (fixtures or live).
    style = source("urn:personal:prefs:style")          # e.g. "formal"
    return f"Good day, {name}." if style == "formal" else f"Hey {name}!"
