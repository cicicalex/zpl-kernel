# Quickstart

From nothing to a kernel booting in QEMU, in four commands.

## 1. Toolchain

The kernel builds the core library for its own target, so it needs nightly with
`rust-src`:

```bash
rustup toolchain install nightly --component rust-src clippy
rustup default nightly
```

You also need QEMU (`qemu-system-x86_64`) on your PATH.

## 2. Check it compiles

```bash
cargo check --workspace && cargo test --workspace --exclude zpl-kernel-bin --exclude limine-bridge
```

Expect the workspace to build clean and the host tests to pass. If `cargo check`
complains about `-Zbuild-std`, the toolchain is not nightly.

## 3. Build the kernel

```bash
cargo -Zbuild-std=core,compiler_builtins,alloc \
      -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec \
      build -p zpl-kernel-bin \
      --target crates/zpl-kernel/x86_64-zpl-kernel.json --release
```

## 4. Boot it

```bash
qemu-system-x86_64 \
  -kernel target/x86_64-zpl-kernel/release/zpl-kernel-bin \
  -serial file:boot.log -display none -no-reboot -m 256
```

It runs until you stop it — about fifteen seconds is plenty. Then:

```bash
bash scripts/demo-policy-test.sh boot.log
```

which should print three lines, one per rule of the demo policy:

```
Demo policy gate PASS
  rule 1, within budget   : 69 ALLOW
  rule 2, over the limit  : 1 BLOCK
  rule 3, past the quota  : 1665 DEGRADE (quota 256, 1735 requests)
```

Your numbers will differ — they depend on how long you let it run — but all
three lines must be there.

## If something goes wrong

- **`boot.log` is empty.** QEMU did not start the kernel. Check the path to the
  binary, and that the build in step 3 actually finished.
- **The log stops after a few lines.** Read the last marker and look it up in
  `docs/MARKERS.md`; each one says which self-check emitted it.
- **`demo-policy-test.sh` says there were no decisions.** The kernel did not get
  as far as the scheduler. Same as above — the last marker tells you where.
- **Nothing on the screen when booting from an ISO on real hardware.** Expected
  on some machines; `docs/PHYSICAL_TEST_GUIDE.md` covers it.

## Where to read next

- `docs/SYSCALL_ABI.md` — the system calls and their numbers.
- `docs/MARKERS.md` — every line the kernel prints, and what emits it.
- `docs/KNOWN_LIMITATIONS.md` — what does not work. Worth reading early.
- `crates/zpl-kernel/src/zpl_policy.rs` — the demo policy, rules written out at
  the top of the file.
