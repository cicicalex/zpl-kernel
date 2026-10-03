# Changelog

All notable changes to this repository. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[semantic versioning](https://semver.org/spec/v2.0.0.html) once it has a
released version.

## [Unreleased]

Nothing yet.

## [0.5.0] — first public source release

The first version of this kernel published as source. Earlier versions exist but
were never released, so what follows describes the state of this release rather
than a difference from a published predecessor.

### Added

- **A kernel that boots**, on QEMU via `-kernel` and on real hardware from an ISO
  through the Limine boot protocol.
- **Memory**: a bitmap physical frame allocator, a bump heap, 4 KiB paging with
  map, unmap and TLB invalidation, and a guarded kernel stack that turns an
  overflow into a diagnosable page fault rather than a triple fault.
- **Ring 0 / ring 3 separation** with a TSS, and a system-call layer documented
  in `docs/SYSCALL_ABI.md`.
- **An ELF64 loader**, an in-RAM filesystem, pipes, and shared memory.
- **A scheduler** that picks between tasks on a per-tick score.
- **A policy gate** in front of every state-changing request, which answers
  before the kernel acts rather than after.
- **A hash-linked log** with append, re-walk and tamper detection, exercised by a
  self-check on every boot. It is not yet fed by the gate's live decisions, the
  hash is not cryptographic, nothing is signed, and it lives in RAM — see
  [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §5, which says so at more
  length than this line can.
- **A framebuffer console**, so the kernel is visible on a screen and not only
  on a serial port.
- **A command line**, `zpl-sh`, read from the PS/2 keyboard and from COM1:
  `help version pci mem ps audit run clean/hostile net uptime`. It reports the
  kernel's own state and can put one request through the gate; it does not load
  or run programs from a filesystem.
- **A policy panel** in the bottom seven rows of the screen, which do not
  scroll: the gate's counters, the last decision of each kind with the rule
  behind it, and a summary row carrying the version and a fingerprint of the
  boot. Drawn on both consoles, so it is there whether the kernel was started
  with `-kernel` or from a stick.
- **A demo policy** with three rules — within budget, over the limit, past the
  quota — so a build from this source actually runs and the gate can be watched
  working.
- **Tooling**: `zpl-asm` for the example programs, `zpl-fs` and `zpl-mkfs` for
  disk images, `zpl-test-runner` for booting in QEMU and asserting on the serial
  output, and a set of scripts under `scripts/` for the build and boot gates.

### Not included

- **The private policy engine.** The public build links a demo policy instead.
- **SMP beyond a topology probe**: the kernel reads the CPU topology and then
  runs on one core.
- **Networking beyond one card and one protocol**: there is an e1000 driver
  that sends and receives, and answers an ARP request, on the `-kernel` boot
  path only. There is no IP stack, and the driver is not started on the ISO
  path, where it has never been proven.
- **Loading programs from a filesystem.** `zpl-sh` runs what is built into the
  kernel, not a file you point it at.

`docs/KNOWN_LIMITATIONS.md` is the current list.

### Known weaknesses

Stated here as well as in `SECURITY.md`, because they change what this software
is for:

- the decision log is linked with a 64-bit non-cryptographic, unkeyed mixer. It
  detects accidental corruption. It does not survive deliberate tampering by
  anyone who can write the file;
- the log records the decision, not the request that produced it;
- a kernel built from this source demonstrates the architecture. It is not
  something to run work on;
- on real hardware it is early: on the one laptop tested it boots but does not
  read the keyboard, and on the one UEFI mini PC tested it does not start. It runs
  in QEMU over BIOS and UEFI. `docs/KNOWN_LIMITATIONS.md` has the details.

[Unreleased]: https://example.invalid/compare
[0.5.0]: https://example.invalid/releases
