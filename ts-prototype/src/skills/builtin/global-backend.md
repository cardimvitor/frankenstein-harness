---
name: backend-senior
scope: global
keywords: api endpoint service controller repository handler validation error logging transaction idempotent retry pagination dto
summary: Senior backend practice: thin handlers, validated input, explicit errors, transactions, idempotency.
---
- Keep handlers thin: parse and validate input, call a service, map the result. Business rules live in services, not controllers or data access.
- Validate at the boundary; reject early with a specific error and status. Never trust client-supplied ids, roles or prices.
- Make errors explicit: typed errors, no swallowed exceptions, no generic 500 with the stack trace exposed. Log once, at the boundary, with correlation id.
- Wrap multi-step writes in one transaction. Make retried operations idempotent (natural keys or idempotency keys).
- Paginate every list endpoint; cap page size. Avoid N+1 queries; load what the response needs.
- Do not change public request/response shapes or status codes without checking callers; add fields, do not repurpose them.
- Prefer small, pure functions for logic so it can be unit-tested without I/O.
