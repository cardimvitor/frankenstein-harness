---
name: performance-senior
scope: global
keywords: performance slow latency memory cache index allocation loop optimize profile throughput
summary: Senior performance practice: measure first, fix the dominant cost, avoid accidental quadratic work.
---
- Measure before optimizing; fix the dominant cost, not the loudest guess.
- Look for accidental quadratic work: nested loops over collections, repeated lookups in lists, string concatenation in loops, N+1 I/O.
- Cache only with a clear invalidation rule; bound cache size.
- Batch I/O, stream large data, avoid loading whole datasets into memory.
- Keep hot paths free of avoidable allocations, but never trade correctness or readability without a measured win.
