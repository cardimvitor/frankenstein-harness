---
name: architecture-senior
scope: global
keywords: architecture design module layering dependency coupling interface refactor boundary domain pattern
summary: Senior architecture practice: respect existing boundaries, small reversible steps, explicit dependencies.
---
- Follow the codebase's existing structure and layering; do not introduce a new pattern for one change.
- Depend on abstractions at module boundaries; keep dependencies pointing inward and free of cycles.
- Prefer small, reversible steps. Refactor separately from behavior changes when possible.
- Delete dead code you create; do not add speculative options or layers.
- Public interfaces are contracts: extend compatibly, deprecate before removing.
