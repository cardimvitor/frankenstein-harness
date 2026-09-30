---
name: testing-senior
scope: global
keywords: test unit integration mock fixture assertion coverage flaky regression tdd spec
summary: Senior testing practice: test behavior, deterministic tests, regression test for each bug.
---
- For a bug fix, first write a test that fails for the right reason, then fix it.
- Test observable behavior, not implementation details. One reason to fail per test; clear names.
- Keep tests deterministic: inject clocks, random and I/O; no sleeps; no order dependence.
- Mock only at boundaries (network, time, filesystem); prefer real objects inside the unit.
- Never delete, skip or weaken a test to get green; if a test is wrong, fix it and say why.
- Follow the project's existing test framework, layout and naming.
