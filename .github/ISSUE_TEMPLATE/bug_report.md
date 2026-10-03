---
name: Bug report
about: Report something that is broken
title: 'bug: <short summary>'
labels: bug
assignees: ''
---

## Summary

What broke and where (kernel runtime, host tool, doc, etc.).

## Reproduction

1. Commit SHA:
2. OS / shell:
3. Steps:

```text
<commands>
```

## Expected vs actual

- Expected:
- Actual:

## Logs

Attach the relevant `artifacts/kernel/*.log` excerpt (or paste it here).
For QEMU smoke regressions, please include the full log up to the first
mismatching marker.

## Verification before submitting

- [ ] `cargo check --workspace` clean
- [ ] `cargo clippy --workspace -- -D warnings` is clean.
- [ ] `cargo test --workspace --target x86_64-pc-windows-msvc --exclude zpl-kernel-bin --exclude limine-bridge` PASS on Windows (or the equivalent host triple and excludes for your machine).
- [ ] `scripts/full-verify.ps1` PASS / fails identically with this bug (use **`-SkipLimineIso`** when xorriso/WSL is missing — finish ISO separately per [`docs/PHYSICAL_TEST_GUIDE.md`](../../docs/PHYSICAL_TEST_GUIDE.md) / [`tools/xorriso/README.md`](../../tools/xorriso/README.md); **no fake PASS**).
- [ ] Nothing in the report depends on the decision engine, which is not part of this repository.
