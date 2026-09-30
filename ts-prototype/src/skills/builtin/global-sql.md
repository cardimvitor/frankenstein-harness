---
name: sql-senior
scope: global
keywords: sql query index join migration schema database transaction orm entity table select insert update
summary: Senior SQL practice: parameterized queries, indexes for access paths, safe migrations.
---
- Always parameterize; never concatenate user input into SQL.
- Select only needed columns; avoid SELECT * in application code. Add an index for each frequent filter/join/order path and check with the query plan.
- Watch N+1 patterns from ORMs; prefer a join or a batched query.
- Migrations: small, reversible when possible, backwards compatible with the running app (add nullable/default, backfill, then enforce). Never edit an applied migration.
- Use the right isolation and keep transactions short; do not hold them across network calls.
- Handle NULL semantics and time zones deliberately (store UTC).
