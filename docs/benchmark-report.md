# Benchmark Report

## Harness

- Tool: pb-bench
- Cases: small_3x3, mid_8x8, large_64x64
- Metrics: mean latency, p95 latency

## Source

- JSON: artifacts/bench/bench_all.json

## Result table

| Case | Repeats | Mean (ms) | P95 (ms) |
|------|---------|-----------|----------|
| small_3x3 | 5 | 0,413 | 0,471 |
| mid_8x8 | 5 | 1,916 | 2,480 |
| large_64x64 | 5 | 8,639 | 11,408 |