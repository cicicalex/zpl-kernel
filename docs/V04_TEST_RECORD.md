# ZPL kernel v0.4 — test record

**Date:** 23 September 2026
**Build under test:** the v0.4 build, as it stood on that date.
**Host:** Windows 11 Pro 26200, Intel i7-8700 (6C/12T), 48 GB RAM
**Toolchain:** `rustc 1.100.0-nightly (6bb1652a0 2026-09-22)`, LLVM 23.1.1, MSVC 14.44.35207
**Emulator:** QEMU 11.1.0 · **Bootloader:** Limine v12.2.0 (pinned by SHA in `scripts/build-iso.ps1`)

> **About the commit hashes in this record.** They are from the working
> repository this kernel was developed in, which is not the history that was
> published: the public repository starts from a single commit. `git show` on
> any of them will fail here. They are kept because they are how the record was
> taken, not because you can look them up.

This document records what was measured, how to reproduce it, and what it does **not**
show. It contains no algorithm internals and no decision thresholds.

---

## 1. What v0.4 changes, and why

Up to v0.3 the kernel mirrored its critical markers into the legacy VGA text buffer at
physical `0xB8000`. On 20 September 2026 the kernel was booted from USB on an Acer
Aspire 5738. It reached its halt loop and kept emitting markers on COM1 — but the monitor
stayed black after the bootloader's own output.

The cause is not a fault in the kernel: Limine v12.2.0 hands the kernel a **graphical**
framebuffer, and in that mode the legacy text buffer is not what the display controller
scans out. A kernel that only writes to `0xB8000` is therefore invisible, however healthy
it is.

v0.4 adds a framebuffer console, a polled keyboard, a three-key demo, and an on-screen
summary, so that the machine's state can be read from the screen alone.

Everything is behind two Cargo features, `fb_crit_mirror` and `v04_menu`, which only
`limine-bridge` enables. The `zpl-kernel-bin` target that every automated gate runs
against is unchanged by v0.4.

---

## 2. Determinism method

A boot is summarised by the SHA-256 of its deterministic marker lines:

```bash
grep -a '^\[ZPL-' RUN.log | grep -a -v -e ALIVE -e rdtsc -e PERF -e ZPL-V04 | sha256sum
```

`ALIVE` (the liveness ticker), `rdtsc` and `PERF` (timing) are excluded because they are
time-dependent by construction. `ZPL-V04` is excluded because those lines are new in this
version; excluding them lets a v0.4 boot be compared against a v0.3 boot line for line.

### 2.1 A caveat that must be stated

Two of the 32 lines are **not** stable across binaries, and that is a property of the
markers, not of kernel behaviour:

| line | why it moves |
|---|---|
| `[ZPL-PDPT-VA=…]` | a symbol address — it changes whenever any code changes |
| `[ZPL-SMP … max_logical=…]` | observed as 17 / 18 / 17 across three different binaries on the *same* QEMU invocation |

The first means a criterion of the form *"the hash must stay identical after adding
code"* can never be satisfied while a symbol address is inside the hashed set.

**The second was recorded here as "unexplained", with the note that "the inline-asm
wrapper is correct (it saves and restores `rbx`)". That note was wrong, and the value was
a bug.** Corrected on 26 September 2026 — see §2.2.

Excluding those two lines leaves **30 lines**, which are byte-identical across every run
recorded below, including the May 2026 ISO that was tested on hardware. That is the
measure of "kernel behaviour unchanged"; it is reported **in addition to** the full hash,
never instead of it.

### 2.2 The 17/18 anomaly, explained and fixed — and what it costs the §6 claim

Found 26 September 2026, from a disassembly of the shipped binary. The inline-asm wrapper
in `crates/zpl-kernel/src/smp.rs` was:

```
push rbx ; cpuid ; mov {b:e}, ebx ; pop rbx     with b = lateout(reg)
```

`lateout(reg)` lets the register allocator choose **any** general-purpose register for the
output — including `rbx` itself. For `cpuid(1)` it did:

```
1: 106ad7  push  %rbx          <- save
   106ad8  cpuid               <- ebx = CPUID.01H:EBX
   106ada  mov   %ebx,%ebx     <- the "copy out", a no-op
   106adc  pop   %rbx          <- result destroyed
   106ae5  mov   %ebx,%ebp     <- reads the restored, stale value
```

So `max_logical_cpus` and `apic_id` were slices of whatever the caller left in `rbx` — in
the kernel, the low half of a pointer. The observed `apic_id=128 max_logical=17` is
`0x8011…`, which is a kernel address, not a CPU identifier. That also explains what the
table above called unexplained: the value tracked the **binary**, because a pointer does,
and it was stable within a binary for the same reason.

`cpuid(0)` happened to get a different register (`mov %ebx,%esi`), so **`max_leaf` was
always correct.** The bug was confined to the two fields read out of leaf 1's EBX.

**Fixed** by using `core::arch::x86_64::__cpuid`, which gets the constraints right. The
fixed build disassembles to `mov %rbx,%r10 ; cpuid ; xchg %rbx,%r10` — allocator-safe — and
QEMU now reports:

```
[ZPL-SMP max_leaf=13 max_logical=0 apic_id=0 OK]
```

`apic_id=0` is the expected BSP value. `build-all` PASS, 3 m 35 s, all workspace tests
green including four new host tests in `smp.rs`.

**What this costs §6.** The `[ZPL-SMP …]` line passes the hashed-set filter (it starts with
`[ZPL-`, contains none of `ALIVE`/`rdtsc`/`PERF`/`ZPL-V04`), so it **is** folded into the
on-screen fingerprint. The fingerprint that matched between QEMU and the Acer therefore
contained two fields that came from the binary, not from the hardware. Those two fields
contributed **no** hardware evidence to the match.

Three things that are **not** damaged, stated as precisely as the damage:

- **The 30-line hash excludes the SMP line by name**, and always did. The "kernel behaviour
  unchanged" measure was never affected.
- **`max_leaf=13`, on the same line, is genuine** and had to match on the Acer for the
  fingerprints to be equal — so that one hardware value is corroborated, not undermined.
- **`[ZPL-VNET] probe bus=0 no virtio-net-pci`** is a real PCI enumeration and is in the
  hashed set too.

The honest form of the §6 claim is therefore: **the software path is deterministic and runs
to completion on real hardware, producing byte-identical output.** It is not: "every value
the kernel read from the hardware was confirmed". Two of them were not read from the
hardware at all.

**The v0.4 artefacts are deliberately not rebuilt.** The ISO, logs and photographs stay
exactly what was tested. The fix changes what future builds print, so the fingerprint of a
build after this commit will differ from `e44a58ef45488dc4` — by design, and only in those
two fields.

### 2.3 The version string was `v0.3.0-dev`, and changing it moves the 30-line hash

Added 30 September 2026, as a **proposal**; revert this commit and the number below goes
back to what it was.

The kernel announced itself as `v0.3.0-dev` while the CHANGELOG draft, the tested ISO, the
artefacts in `artifacts/v04/` and this record all say v0.4. One of the two had to move. The
build marker moved, to `v0.4.0`, because the disk says v0.4 everywhere else and because
`-dev` inside a citable record is a contradiction.

**What that costs.** The BUILD-ID line passes `line_counts` — it starts with `[ZPL-` and
carries none of `ALIVE`/`rdtsc`/`PERF`/`ZPL-V04` — and it is **not** one of the two lines
§2.1 excludes. It is therefore inside the 30-line set, which is the measure of "kernel
behaviour unchanged" and which has been `f750944dc831e9e0` for every build from the May
2026 ISO through v0.4.

Recomputed by substituting the line in `artifacts/v04/v04f-det1.stable`, which reproduces
`f750944dc831e9e0` exactly before the substitution:

| | 30-line hash (first 16) |
|---|---|
| every build, May 2026 → v0.4 | `f750944dc831e9e0` |
| any build after this commit | `607e1f1b0bace322` |

**The v0.4 artefacts are not rebuilt**, here as in §2.2. The ISO, the logs and the
photographs stay exactly what was tested, and `f750944dc831e9e0` stays the correct value
for them.

**Two ways to keep the continuity, if it is wanted** — neither taken here, because both
are method changes, and a test record is the wrong place to change the method:

1. **Record `607e1f1b0bace322` as the new baseline** and say in one line why it moved. The
   chain of equal hashes breaks once, on a version bump, for a reason written down.
2. **Exclude the BUILD-ID line from the hashed set**, the way `[ZPL-PDPT-VA=…]` already is.
   A build identity is not behaviour; keeping it in means the measure fails on every
   version bump, which makes it useless exactly when a release needs it. The set becomes
   29 lines and the old value can no longer be compared either.

A third possibility is to leave the marker at `v0.3.0-dev` and move the CHANGELOG to
`0.3.0` instead. That keeps `f750944dc831e9e0` intact, and it contradicts the ISO name, the
hardware record and three draft files.

---

## 3. Results

### 3.1 Automated gates (`zpl-kernel-bin`, unaffected by v0.4)

Run with nothing else running on the machine:

| gate | command | result |
|---|---|---|
| build + boot | `scripts\build-all.ps1 -TimeoutMinutes 20` | **PASS**, exit 0, 4 min 28 s |
| hostile workload | `scripts\attack-test.ps1` | **PASS** |
| audit replay | `scripts\replay-test.ps1` | **PASS** |
| unit tests | `cargo test --workspace --quiet` | **29 passed / 0 failed** |
| v0.4 unit tests | `cargo test -p zpl-kernel --features v04_menu` | **15 passed / 0 failed** |

`build-all` contains 23 marker assertions; none fired. The hostile-workload gate reports
`[ZPL-SCHED tid=user ain=15 action=BLOCK]`, and the replay gate rejects a tampered chain
with a non-zero exit and `[E_REPLAY_011_SELF_HASH] record 3 self_hash mismatch`.

**The 29 tests do not cover v0.4.** `v04_menu` is not a default feature, so the ten new
unit tests and the lints on the new modules require:

```bash
cargo clippy -p zpl-kernel --features v04_menu --all-targets -- -D warnings   # exit 0
cargo test  -p zpl-kernel --features v04_menu                                  # 15 passed / 0 failed
```

Anyone validating this work must run those two commands as well; `cargo test --workspace`
alone does not compile the v0.4 code.

**This gap caught a real defect.** On the second verification round
`cargo test -p zpl-kernel --features v04_menu` exited 101: the unit test asserting FNV-1a
against published vectors failed on `"foobar"`. The reference constant written into the
test was wrong — the kernel's own value, `0x85944171f73967e8`, is the correct one, as
confirmed by an independent Python implementation. The test was fixed; the implementation
was not changed, because it was already right. The defect survived the first round only
because that round ran `clippy` on the new code but not the tests. The commit message of
`47a9d23` claims "6 new unit tests pass"; at that commit there were ten new tests and one
of them failed. That claim is corrected here rather than by rewriting history.

### 3.2 Boot reproducibility

Each configuration was booted twice, 60 s per boot, with a PNG screen capture at 45 s.

| configuration | lines | SHA-256 (first 16) | 30-line hash (first 16) |
|---|---|---|---|
| May 2026 ISO staging (v0.3) | 32 | `de265cc8803b6a81` | `f750944dc831e9e0` |
| commit `c276de0`, pre-v0.4 | 32 | `e5127da9e69b4204` | `f750944dc831e9e0` |
| v0.4 after console (4a–4c) | 32 | `254bb41908900a84` | `f750944dc831e9e0` |
| v0.4 after menu (4d–4e) | 32 | `c5df739832ef989e` | `f750944dc831e9e0` |
| v0.4 after summary (4f) | 32 | `5de1b24519186040` | `f750944dc831e9e0` |

Within each configuration the two boots were byte-identical. Across configurations the
full hash differs only in the two lines named in §2.1; the 30-line hash is unchanged
from the May build through to v0.4.

A second round booted the finished **ISO via BIOS** (`-cdrom … -boot d`, the same path the
hardware test uses) twice more: 32 lines, `5de1b24519186040…` — byte-identical to each
other *and* to the UEFI boot of the same binary, including the two layout-sensitive lines.
The same binary started over two different boot paths produces the same markers.

All gates were re-run in that round and all passed again.

### 3.3 On-screen output

Captures are in `docs/v04-ecran-*.png`, taken from a BIOS boot of the v0.4 ISO.

- `v04-ecran-selfcheck.png` — 20 self-check lines, green for those ending in `OK`, plus
  the status block.
- `v04-ecran-allow.png` — after key `1`: `[ZPL-V04 menu=1 ain=99 action=ALLOW]`, green.
- `v04-ecran-block.png` — after key `2`: `[ZPL-V04 menu=2 ain=17 action=BLOCK]` in red,
  with the reason given in words only.
- `v04-ecran-audit.png` — after key `3`: audit chain length and `verify=ok records=5`.

Some markers (`[ZPL-SMP]`, `[ZPL-STACKGUARD]`, `[ZPL-AUDIT]`, `[ZPL-PERF]`) do not appear
on screen. They are emitted directly rather than through `emit_critical_marker`, so the
pre-existing VGA mirror did not show them either. This is unchanged behaviour, not a
regression.

### 3.4 The on-screen hash is reproducible from the log

The bottom line of the screen reads
`ZPL kernel v0.4 - N self-check-uri OK, M decizii, hash <16 hex digits>`.
The hash is FNV-1a/64 — a published algorithm, deliberately not the audit chain's mixer —
folded over exactly the lines the determinism filter keeps.

| run | hash read from the screenshot | recomputed from the serial log | match |
|---|---|---|---|
| QEMU UEFI, key `2` pressed | `31758f643268b4ed` | 33 lines → `31758f643268b4ed` | yes |
| BIOS boot from ISO, keys `1`,`2`,`3` | `e44a58ef45488dc4` | 34 lines → `e44a58ef45488dc4` | yes |

The counters matched too (10 self-checks OK in both runs; 3 and 4 decisions). The
recomputation is an independent Python implementation reading the raw serial log.

Note that pressing key `2` adds a **deterministic** line to the log, emitted by the shared
memory gate rather than by the demo code, so a key press changes the determinism hash.
The automatic boot sequence, with no keys pressed, stays at 32 lines.

### 3.5 ISO

| artifact | size | SHA-256 |
|---|---|---|
| `artifacts/v04/zpl-kernel-v04.iso` | 18 335 744 B | `76F2BE4693174FAE517C53647FDD44524F5F0E45B264B1600BE2B70BB47E7548` |

Built with `xorriso 1.5.6` under WSL Ubuntu 26.04.1, using the exact `xorriso -as mkisofs`
invocation from `scripts/build-iso.ps1`, followed by `limine.exe bios-install`. The
script itself was **not** run, because it deletes the previously hardware-tested ISO; that
file was verified unchanged afterwards.

---

## 4. What this does not show

- **§3 is emulator-only.** Every measurement in §3 comes from QEMU. The hardware run in
  §6 passed and matched the emulator's fingerprint, but it is a single run on a single
  machine, with photographs rather than a captured serial log as evidence.
- **The decision input is still synthetic.** The workloads that produce ALLOW and BLOCK
  are driven by a hand-written input value, not by anything measured from the task's
  actual behaviour. This test shows that the kernel *displays* its decisions correctly;
  it does **not** show that those decisions track real workload behaviour.
- **No comparison against a baseline scheduler** was performed. Nothing here establishes
  that the decision model outperforms a simpler rule on the same inputs.

---

## 5. Reproducing this

```bash
# 1. gates
scripts\build-all.ps1 -TimeoutMinutes 20
scripts\attack-test.ps1
scripts\replay-test.ps1
cargo test --workspace --quiet
cargo clippy -p zpl-kernel --features v04_menu --all-targets -- -D warnings
cargo test  -p zpl-kernel --features v04_menu

# 2. bootloader-side binary
cargo +nightly -Zbuild-std=core,compiler_builtins,alloc \
  -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec \
  build -p limine-bridge --target limine-bridge/x86_64-zpl-limine.json --release

# 3. boot it (QEMU must be on PATH for the session only)
qemu-system-x86_64 -m 256M -cdrom artifacts/v04/zpl-kernel-v04.iso -boot d \
  -display none -serial file:run.log -no-reboot -monitor tcp:127.0.0.1:4455,server,nowait
# screenshots and key presses go through the monitor socket:
#   screendump <abs-path>.png -f png
#   sendkey 1 | sendkey 2 | sendkey 3

# 4. determinism hash
grep -a '^\[ZPL-' run.log | grep -a -v -e ALIVE -e rdtsc -e PERF -e ZPL-V04 | sha256sum
```

## 6. Hardware test — performed 26 September 2026: **PASS**

The results in §3 are all from QEMU. This is the test that decided whether v0.4 actually
fixes the black screen seen on 20 September 2026. **It does.**

**Target:** Acer Aspire 5738, booted from USB, BIOS (not UEFI), F12 → USB HDD.
**Image:** `artifacts/v04/zpl-kernel-v04.iso`, SHA-256 verified on the host before writing;
written with Rufus 4.15, MBR, BIOS target, **DD (raw image) mode**. All external hard
drives were disconnected first and this was confirmed with `Get-Disk` before writing.

### 6.1 Result

> The screen strings quoted in this section are in Romanian because that is what
> the v0.4 build printed. They are quoted, not described, so that the table says
> what was on the screen rather than what it meant. The kernel prints English
> from v0.5.0 onward; these lines are a record of an older build and are left as
> they were.

| check | what the photographs show | verdict |
|---|---|---|
| screen comes up | full boot sequence from `[ZPL-BOOT] kernel_entry reached` through to the menu, drawn on the framebuffer. The 20 September black screen is gone. | **PASS** |
| self-checks | the `… OK` lines are green; summary reads `10 self-check-uri OK` | **PASS** |
| tick counter | increments continuously (55 → 345 → 389 → 529 → 761 across the photographs) | **PASS** |
| key `1` | green `action=ALLOW] program curat: rulat` | **PASS** |
| key `2` | red `action=BLOCK] intrare puternic dezechilibrata: scris refuzat` | **PASS** |
| key `3` | `verify=ok records=5] lant de audit intact` | **PASS** |

**Correction, added after the run: the keys were pressed on a USB keyboard**, not on the
laptop's built-in one, which does not work. The kernel only reads the PS/2 controller, so
the key presses arrived through the BIOS's USB Legacy Support emulation of a PS/2
keyboard. That path exists in legacy BIOS mode and may not exist when a machine is booted
through UEFI, where USB keyboards are usually handled by the firmware's own USB stack and
are not presented on the legacy ports.

So what §6.1 demonstrates is narrower than "the keyboard works": it is that key input
reaches the kernel **on this machine, booted in legacy BIOS mode, with USB Legacy Support
enabled**. Whether a USB keyboard reaches it on a UEFI boot is untested, and on the
evidence here should be assumed not to until someone tries it. A machine with a genuine
PS/2 port is also untested, though that is the case the driver was written for.

**The fingerprint matches the emulator exactly.** After keys 1, 2, 3 the hardware screen
shows `e44a58ef45488dc4` — the same value §3.4 records for the BIOS boot from ISO under
QEMU. The same binary, on real silicon and in emulation, folds the same marker set to the
same hash. Fifteen of the sixteen characters are legible in the clearest photograph; the
sixteenth sits under a speck of dirt on the panel and was read directly off the screen.

> **What that match does and does not prove — corrected 26 September 2026, see §2.2.**
> The hashed set includes the `[ZPL-SMP …]` line, and two of its three fields
> (`max_logical`, `apic_id`) were a CPUID bug reading a stale register rather than the
> hardware. Those two fields came from the binary, so they are identical on any machine
> and contributed nothing to the match.
>
> What the match does prove: **the software path is deterministic and runs to completion on
> real hardware, producing byte-identical output.** On the hardware side, `max_leaf=13` on
> that same line is genuine and had to agree for the hashes to be equal, and the
> `[ZPL-VNET]` PCI probe is a real enumeration. What it does not prove is the stronger
> reading — that every value the kernel reported from the hardware was confirmed.
>
> The 30-line hash excludes the SMP line by name and was never affected.

Before any key was pressed the summary read `b30dcae5caea6309` with 2 decisions, which is
consistent: the hash is cumulative over markers seen so far.

`[ZPL-VNET] probe bus=0 no virtio-net-pci` appears on hardware and is expected — that
device only exists under QEMU.

Evidence: `artifacts/v04/fier-2026-09-26/` (a written result plus four photographs). The
photographs were read directly when writing this section, not taken on trust.

### 6.2 Open defect, not fixed

**The last character of the summary line flickers on the Acer.** It is legible but blinks,
which is why the sixteenth hash character had to be confirmed by eye rather than from a
photograph. **The cause has not been verified.** The place to start is `fbcon.rs`
(`newline` / `PENDING_ROW_CLEAR`, around lines 407–424): the bottom row is blanked lazily
and rewritten on every tick, and the final character is drawn last, so it spends the
largest fraction of each cycle blank. That is a hypothesis, not a diagnosis — it has not
been measured. It does not affect correctness of the output and was deliberately left
unfixed here.

### 6.3 Procedure, for repeating the test

**Before opening the imaging tool, disconnect every other external drive.** The tool lists
all removable and external disks together; picking the wrong one destroys that disk. On the
machine used here five external hard drives (1.8-7.4 TB) were attached, any of which would
be listed alongside the 64 GB stick.

1. Verify the image first:
   `certutil -hashfile artifacts\v04\zpl-kernel-v04.iso SHA256`
   must print `76F2BE4693174FAE517C53647FDD44524F5F0E45B264B1600BE2B70BB47E7548`.
2. Write it to the stick as a **raw image** (in Rufus: partition scheme **MBR**, target
   system **BIOS (or UEFI-CSM)**, and when prompted about the ISOHybrid image choose
   **DD / image mode**, not ISO mode). File-copy mode will not boot.
3. On the laptop: F12 at power-on, choose **USB HDD**.

### 6.4 What a pass looks like

- Roughly twenty marker lines appear top-left; the ones ending in `OK` are green.
- A menu line offers keys `1`, `2`, `3`.
- The bottom three rows carry a tick counter that keeps incrementing, the last decision,
  and a summary line ending in a 16-character hash.
- Key `1` adds a green `action=ALLOW` line; key `2` adds a **red** `action=BLOCK` line with
  the reason in words; key `3` adds the audit chain length and `verify=ok`.

### 6.5 What each failure mode would mean

| symptom | reading |
|---|---|
| bootloader menu, then black, no reboot | the v0.3 behaviour — the framebuffer console did not take effect |
| nothing at all, or an immediate reboot | the stick was not written as a raw image, or the firmware booted it as UEFI |
| text appears but the tick counter is frozen | the kernel reached the halt path and stopped making progress |
| text appears, keys do nothing | the framebuffer console works but the PS/2 path does not on this hardware |

Photograph the screen after boot and after each of the three keys. A photograph of a frozen
or blank screen is a result and should be recorded as one.

Full chronological record, including two procedural mistakes made during this session and
how they were corrected, is kept with the run records.


---

## 8. The 30-line hash after v0.5.0 — and why it is now a 44-line hash

Added 30 September 2026, when the version marker moved to `v0.5.0` (§2.3 is the
same question one version earlier).

### 8.1 The method, reproduced before anything was computed with it

The filter in §2 was applied to `artifacts/v04/v04f-det1.stable`, unchanged, and
then with only the version string substituted. Both historical values came back
exactly:

| set | first 16 of sha256 | matches the record |
|---|---|---|
| the artifact as stored (`version=v0.3.0-dev`) | `f750944dc831e9e0` | yes |
| same, with `version=v0.4.0` substituted | `607e1f1b0bace322` | yes |

That is the control. Line endings matter: LF, with a trailing newline. With CRLF
the same bytes hash to `482990254d126015`, and without the final newline to
`e8506a0bb5c2d6a1` — neither of which is this measure.

### 8.2 The like-for-like successor

By the same substitution, `version=v0.5.0` gives **`4e032517c2f5b6f3`**. That is
the number that continues the series of §2.3, and it is of limited use, because
of what follows.

### 8.3 The set is no longer 30 lines, and the reason is not the version

Applying the §2 filter to a boot of today's public build gives **485 lines**, not
30. The difference is almost entirely `[ZPL-SCHED …]`: 439 of those 485. The May
build did not put scheduler output through this filter at all.

Worse, the count is not fixed. Three boots of the *same binary* gave 491, 488 and
485 lines, and therefore three different hashes. The scheduler tick is driven by
the PIT, so how many of its lines a boot emits depends on how long the machine
ran before it halted — which makes any hash over them a measure of duration, not
of behaviour.

### 8.4 Two more lines that are time-dependent by construction — and they are ours

With the scheduler lines removed as well, three boots give 46 lines each, and
*still* three different hashes. Exactly two lines differ, and both are new this
month:

```
[ZPL-E1000] link … frame_at=874088
[ZPL-E1000] rx len=64 polls=1 … cycles=874088 …
```

They carry cycle counts on purpose: those counts are the evidence for what the
emulated card's timer does. But that puts them
in the same category as `rdtsc` and `PERF`, which §2 already excludes by name for
exactly this reason. They are excluded here on the same grounds, and not because
excluding them makes a number come out.

### 8.5 The baseline

Filter: the §2 filter, plus `[ZPL-SMP …]` and `[ZPL-PDPT-VA=…]` as in §2.1, plus
`[ZPL-SCHED …]`, plus any line carrying `cycles=` or `frame_at=`.

| | |
|---|---|
| lines | 44 |
| first 16 of sha256 | **`7db07c0c02d1aaa2`** |
| runs it was identical across | 3, same binary |

The chain of equal hashes from May 2026 does **not** continue into this value,
and it is not meant to: the set it is taken over is a different set, for the
reasons above. What §6 claimed — that the software path is deterministic and runs
to completion producing byte-identical output — is what §8.5 still measures, over
the lines that are byte-identical by construction rather than by luck.
