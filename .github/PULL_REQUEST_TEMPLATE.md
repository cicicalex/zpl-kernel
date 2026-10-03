# Pull Request

## Linked issue

Closes / advances: #<N>.

## Summary

<1-3 sentences>

## Verification

GitHub issue templates (`.github/ISSUE_TEMPLATE/*.md`) checklist the same
host surfaces as this section. 
- [ ] `cargo check --workspace` clean
- [ ] `cargo clippy --workspace -- -D warnings` clean
- [ ] `cargo test --workspace --target x86_64-pc-windows-msvc --exclude zpl-kernel-bin --exclude limine-bridge` PASS  
  (includes **`zpl-kernel`** host `phys_hhdm` tests; matches `scripts/full-verify.ps1`.)
- [ ] `cargo build -p zpl-kernel-bin --release` clean (custom JSON target; see `QUICKSTART.md`)
- [ ] `scripts/full-verify.ps1` PASS (Windows host; use **`-SkipLimineIso`** when xorriso/WSL is missing — finish ISO separately per [`docs/PHYSICAL_TEST_GUIDE.md`](../docs/PHYSICAL_TEST_GUIDE.md) / [`tools/xorriso/README.md`](../tools/xorriso/README.md); **no fake PASS**)
- [ ] If this PR touches kernel runtime: `scripts/elf-test.ps1`, `scripts/ring3-test.ps1`, `scripts/attack-test.ps1`, and `scripts/panic-test.ps1` all PASS
- [ ] Determinism unaffected (or determinism markers updated alongside)

## Scope

- [ ] This change stays inside this repository: it does not copy in, or depend
      on, the decision engine, which is not part of it.

## Notes for reviewers

<anything you want eyes on>
