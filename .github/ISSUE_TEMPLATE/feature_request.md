---
name: Feature request
about: Suggest a new capability for the kernel or its tooling
title: 'feat: <short summary>'
labels: enhancement
assignees: ''
---

## Goal

What capability are you asking for?

## Motivation

Why does the project need this? What concrete scenario is unblocked?

## Proposed scope

- Files / modules touched:
- Acceptance criteria:
- Out of scope:

## Risks

- Scope: confirm this can be built inside this repository, without the
  decision engine, which is not part of it.
- Determinism: does this change the boot self-check or QEMU smoke
  output? If yes, propose how the verification fortress is updated.

## Verification expectations (before merge)

Contributors should satisfy the same gates the project runs. Quick
reference:

- [ ] `cargo check --workspace` clean
- [ ] `cargo clippy --workspace -- -D warnings` is clean.
- [ ] `cargo test --workspace --target x86_64-pc-windows-msvc --exclude zpl-kernel-bin --exclude limine-bridge` PASS on Windows (or the equivalent host triple and excludes for your machine).
- [ ] `scripts/full-verify.ps1` PASS when your change affects the
  verification fortress (use **`-SkipLimineIso`** when xorriso/WSL is
  missing — finish ISO separately per [`docs/PHYSICAL_TEST_GUIDE.md`](../../docs/PHYSICAL_TEST_GUIDE.md) / [`tools/xorriso/README.md`](../../tools/xorriso/README.md); **no fake PASS**).
