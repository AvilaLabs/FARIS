# Avila Core study integration

Status: planned. This is not a valid Core case package and contains no receipts.

The finished demo includes a native **Compile study** experience. FARIS is the
domain interface: the user edits the reactor scenario, selects analyses, supplies
assumptions and assessment criteria, and inspects generated dependencies. FARIS
materializes the Core contract and compatible registry/bindings from a reusable
demo study template. Ordinary use requires no hand-authored Core JSON.

The detailed implementation requirements are `K01`–`K12` and `U07`–`U09` in
[the demo roadmap](../../docs/ROADMAP.md).

## Flow and boundaries

```text
scenario + analysis selections + assumptions + assessment criteria
    ↓ FARIS dependency expansion and study materialization
generated contract + registry snapshot + applicable template material
    ↓ genuine Avila Core compilation
compiled study or structured findings
    ↓ separate artifact/executable/data readiness checks
bounded execution / verified reuse → bound evidence → scoped assessments
```

Domain dependency expansion belongs to FARIS. Core checks the declared inputs,
types, units, slots, parameters, purposes, claim models, policies, and requirements.
Successful compilation is not a statement that missing scientific data exists,
an executable is available, or an engineering requirement has passed.

Pin the implemented Core version and semantic profile before selecting the
binding. The current local compiler has `compile_documents` for a contract and
registry, and `compile_documents_with_material` for declared bound material.
Native library integration or a versioned worker/CLI boundary must consume the
real structured outcome and preserve diagnostic identities. No invocation is
implemented in this scaffold.

Conversion from FARIS scenario quantities to Core's authoritative decimal/unit
representations needs an explicit tested policy. Presentation rounding cannot
change criteria or verdicts. Study edits produce a new identity and mark displayed
older results as belonging to their original study.

## Coarse stages and standalone execution

Candidate stages are geometry/material preparation, transport/normalization,
and coupled operating-history/comparison. Use templates and generated packaging
for repeated declarations rather than a contract per component or time step.
The generated workflow must bind actual input/output/executable identities and
receipts before presenting a result as Core-backed.

Core should preserve identifiable inputs, stages, assumptions, and scoped results
without changing their scientific meaning. Measure setup and maintenance effort
alongside reproducibility benefits. The Rust engine and CLI remain independently
usable; importing Core into the simulation kernel is not required.

## Compile attribution

Place **Powered by Avila Core** beside the compile progress and resulting
compiler report, near the upper-right study controls. Use a small existing Core
mark with a restrained pulse/spinner during real work, then a quiet static
attribution after success or rejection. A tooltip identifies the Core version
and semantic profile. Provide reduced motion and a text-only fallback.

Fast compilation displays its result immediately. The mark attributes the
compiler; it is not a certification or engineering approval badge. It appears
when Core is actually invoked and is not added to the present geometry scaffold.
