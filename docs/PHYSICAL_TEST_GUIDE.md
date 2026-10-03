# Physical test guide — Limine hybrid ISO → USB

This document explains how to produce the **ZPL kernel lab Limine hybrid BIOS/UEFI ISO** and how to exercise it on **real hardware** (or QEMU as a stand-in). It pairs with `scripts/build-iso.ps1`.

For where `build-iso.ps1` sits in the wider **host vs CI** verification story
and the full scripted-gate table, see `scripts/full-verify.ps1`, which runs
them. Name the gate you skipped when filing regressions from a host
that legitimately skips xorriso — **no fake PASS** on the ISO path without the
toolchain.

For **xorriso resolution on Windows** (PATH, vendored `xorriso.exe`, WSL, empty
`wsl -l -q`) and **`[E_ISO_*]`** codes, see
[`tools/xorriso/README.md`](../tools/xorriso/README.md).

## 1. What you are booting

The ISO is built from:

- **Limine** (pinned release; see `scripts/build-iso.ps1`: tag, zip URL, SHA256).
- **`limine-bridge`**: a small bare-metal ELF staged as `boot/zpl-kernel.elf`. It issues Limine protocol requests (HHDM, memory map, base revision), configures the kernel’s HHDM offset, publishes `ExternalBootFrameV1`, and jumps to `zpl_boot_entry_v1` in `zpl-kernel`.
- **`zpl-kernel`**: linked into the same ELF as the bridge (not the separate `zpl-kernel-bin` QEMU `-kernel` artifact).

**Important:** The **QEMU `-kernel` Multiboot-style path** still uses `zpl-kernel-bin` / `_zpl_qemu_boot`. The **ISO path** uses **Limine → `limine-bridge`**. Serial markers should match the COM1 lines documented in [`MARKERS.md`](./MARKERS.md) once the kernel reaches `kernel_entry`.

**Default ISO profile (Option A — COM1 unblock):** `limine-bridge/Cargo.toml` enables **`engine`** + **`agent_debug_boot`** only (no `vga_crit_mirror` / `vga_visual_hold`). Markers go to **COM1** only; halt uses the plain `halt_once` loop. Use this to capture the full NDJSON gate sequence without legacy VGA MMIO at `HHDM+0xB8000`.

**Optional — VGA mirror + visual hold (metal / HDMI):** Re-add **`vga_crit_mirror`** and **`vga_visual_hold`** on the `zpl-kernel` dependency **after** paging maps **0xB8000** (or use Limine framebuffer). **`vga_crit_mirror`** mirrors `emit_critical_marker` to **0xB8000** (COM1 full line first, then VGA — `boot.rs`). **`vga_visual_hold`** emits **`[ZPL-ALIVE] tick=N`** in the halt path. **Without a PTE for the text buffer, the first VGA write can `#PF` (CR2 ≈ `ffff_8000_000b_8000`); an empty IDT yields #DF / triple fault** — not an infinite “stall”; use `qemu -d int,cpu_reset` to see the chain. For automation on mirror builds, `build-iso.ps1 -SkipQemu` remains valid.

### Hypothesis matrix (evidence-backed)

| ID | Hypothesis | Verdict | Evidence |
|----|------------|---------|----------|
| H1 | Mirror write to `HHDM+0xB8000` unmapped → `#PF` | Confirmed (mirror on) | `qemu-fault.log` + serial: `pre_marker1` + full `[ZPL-BOOT]…` then no `post_marker1`. |
| H2–H5 | scroll / IRQ / self-check / TSS | Inconclusive until mirror off or B maps VGA | — |
| H6 | Empty IDT → cascade → triple fault | Confirmed (QEMU) | `qemu-fault.log` IDT=0 during handling. |

**Metal (Apollo class):** many boxes have **HDMI/VGA but no DB9** — NDJSON from metal needs a **USB–serial** adapter on COM1; HDMI alone cannot show COM1. Visual proof without adapter ⇒ map `0xB8000` or framebuffer (B/D), not Option A alone.

**Suggested order:** Option A (this profile) → minimal fault **IDT** (C) → **map 0xB8000** (B) or framebuffer (D).

**`agent_debug_boot` (NDJSON on COM1):** One-line records (`sessionId` `eadfc1`) at: before/after first `emit_critical_marker`, after `init_early_serial`, after paging self-check, after `interrupts::init`, after HAL self-check, before halt / visual-hold; **`H2`** on VGA scroll (only if `vga_crit_mirror` is on). Log to `c:\Dev\debug-eadfc1.log` via PuTTY / `minicom -C` / QEMU `-serial file:…`.

## 2. Build-host prerequisites

All of the following must be satisfied **on the machine that runs** `build-iso.ps1`:

| Requirement | Why |
|-------------|-----|
| **Rust nightly** + `rust-src` (see `rust-toolchain.toml`) | `build-iso` runs `cargo +nightly … -Z build-std …` for `limine-bridge`. |
| **PowerShell** | Script is `*.ps1`. |
| **Network (first run)** | Downloads the pinned Limine binary zip into `artifacts/iso/limine-dist/…` unless `-SkipLimineDownload`. |
| **xorriso** | Limine upstream requires `xorriso -as mkisofs` for hybrid ISOs. Resolved in this order: `tools/xorriso/xorriso.exe`, `PATH`, then **WSL** `xorriso` when **`wsl -l -q` lists at least one distro**. See [`tools/xorriso/README.md`](../tools/xorriso/README.md). |
| **Limine Windows deploy tool** | Ships inside the Limine zip as `limine-tool-windows-x86/limine.exe` (used for `limine bios-install` on the `.iso`). |

**QEMU** (`qemu-system-x86_64`) is optional: only the script’s default QEMU CD-ROM smoke (`-SkipQemu` to omit).

## 3. Build the ISO

From the repo root:

```powershell
cd "c:\Dev\zpl-kernel-lab"
powershell -ExecutionPolicy Bypass -File scripts/build-iso.ps1
```

Common flags:

| Flag | Effect |
|------|--------|
| `-SkipQemu` | Skip the post-build BIOS CD-ROM QEMU smoke (still builds ISO if xorriso succeeds). |
| `-SkipLimineDownload` | Reuse an already-downloaded/extracted tree under `artifacts/iso/limine-dist/<tag>/` (still verifies zip SHA when the zip is present). |

**Successful outputs** (under `artifacts/iso/`, git-ignored except `.gitkeep`):

- `zpl-kernel.iso` — hybrid Limine image (size must be ≤ 20 MiB per script gate).
- `zpl-kernel.iso.sha256` — checksum line: `<lowercase-hex> zpl-kernel.iso`.

## 4. Error codes (build phase)

| Code | Meaning |
|------|---------|
| `[E_ISO_SHA256_MISMATCH]` | Limine zip or integrity check failed versus the pinned digest. |
| `[E_ISO_NO_LIMINE_EXE]` | Windows `limine.exe` not found after extracting the binary release. |
| `[E_ISO_004_NEED_XORRISO]` | No xorriso on PATH/bundle, or **WSL has no Linux distribution** (`wsl -l -q` empty). |
| `[E_ISO_005_WSL_XORRISO]` | WSL distro exists but `xorriso` is not installed inside it. |
| `[E_ISO_MKISOFS_EXIT]` | xorriso returned non-zero. |
| `[E_ISO_006_LIMINE_BIOS_INSTALL]` | `limine bios-install` failed on the ISO file. |
| `[E_ISO_007_TOO_LARGE]` | ISO exceeded 20 MiB. |
| `[E_ISO_QEMU_*]` | QEMU smoke timeout or missing `[ZPL-BOOT]` markers in the serial log. |

## 5. Pre-flight on QEMU (recommended)

Before writing a USB stick, boot the ISO in QEMU (BIOS):

```powershell
qemu-system-x86_64 -cdrom "artifacts\iso\zpl-kernel.iso" -boot d `
  -serial file:artifacts\iso\kernel\manual-qemu.log `
  -display none -m 256 -no-reboot -no-shutdown
```

Compare the serial log to [`MARKERS.md`](./MARKERS.md). The `build-iso.ps1` smoke checks a minimal subset (`kernel_entry`, `serial initialized`, `halt loop entered`) when QEMU is on PATH and `-SkipQemu` is not set.

## 6. Writing to USB (physical machine)

**Data loss warning:** triple-check the destination drive letter or block device. Prefer **dedicated USB keys**; do not aim the ISO at an internal OS disk.

Typical approaches:

- **Windows GUI:** Rufus, “DD image” or ISO mode as appropriate for a hybrid ISO; select the generated `zpl-kernel.iso`.
- **Linux/macOS:** `dd` of the whole ISO to the raw block device, or tooling appropriate for hybrid images on your distro.

After write, **eject safely** before physical boot.

## 7. Firmware setup hints

- **BIOS / CSM:** Enable USB boot and choose the stick; Limine’s BIOS path is exercised by `limine bios-install` on the image.
- **UEFI:** The hybrid image also carries Limine UEFI payload under `EFI/BOOT/` in the staging layout; many boards will show a UEFI USB entry. Secure Boot may need to be disabled if the firmware refuses an unsigned chain (this repo does not ship signed boot artifacts).
- **Boot order:** Prefer **one-shot boot menu** entry for the USB stick to avoid changing permanent order.

## 8. Serial debugging on hardware

If the board exposes **COM1** (or USB–serial adapter routed to the kernel’s expected port), capture at **115200 8N1** to match common Limine/kernel expectations. You should see the same `[ZPL-BOOT] …` prefixes as in QEMU logs when the kernel path is healthy.

If you get a **black screen** only:

- Retry with serial capture.
- Confirm you booted the **ISO from USB**, not the raw `zpl-kernel-bin` ELF meant for `-kernel`.

## 9. Relation to `full-verify.ps1`

`scripts/full-verify.ps1` invokes `build-iso.ps1` **after** `e2e-test.ps1` unless you pass **`-SkipLimineIso`**. On hosts without xorriso/WSL distro, use:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/full-verify.ps1 -SkipLimineIso
```

until the toolchain in §2 is satisfied.

## 10. Honest blocker note

Build agents or laptops **without xorriso and without a registered WSL Linux distro** will hit **`[E_ISO_004_NEED_XORRISO]`**. That is an **environment** requirement, not a kernel source defect, until the tools in §2 are installed.

You can still **prepare hardware and firmware** (below) while xorriso is
missing—the ISO file simply will not exist until the build host is
ready.

## 11. Reference platform: CLS Narrow Box (Intel N3450)

This section is a **concrete checklist** for a common lab shape: **CLS
Narrow Box** class mini-PC, **Intel Celeron N3450**, **4 GB RAM**, **32
GB eMMC**, firmware offering **UEFI** with **CSM** (legacy BIOS
compatibility). Your exact BIOS vendor string may differ; treat menu
names as **hints** and locate the equivalent options on your board.

### 11.1 Before you touch the machine

1. Obtain a **dedicated USB flash drive** (8 GB+). You will destroy its
   contents when imaging.
2. On the **build machine** (where `zpl-kernel.iso` will eventually be
   produced), keep `zpl-kernel.iso.sha256` alongside the ISO for manual
   verification after copy steps.
3. Optional but recommended: **USB–serial TTL** adapter if the board
   exposes a UART header—serial is easier than HDMI-only bring-up.

### 11.2 Firmware: recommended starting posture

Power on with **Delete** / **F2** / **Esc** (varies) to enter firmware
setup. Then:

| Goal | Typical menu location | Suggested setting |
|------|----------------------|-------------------|
| Boot the Limine hybrid ISO from USB | Boot / Boot priority | Enable **USB boot**; disable fast boot if it hides USB |
| Legacy path for hybrid ISO | CSM / Boot mode | **Enable CSM** or set **UEFI + Legacy** if the stick fails pure UEFI first |
| Secure Boot | Security / Boot | **Disable** Secure Boot while validating unsigned Limine payloads |
| Serial after boot | Super I/O / ACPI | Enable **COM1** if present; note **IRQ/address** if non-default |

**Honest note:** some boxes hide CSM unless CSM support is enabled
globally; others only offer UEFI. If **only UEFI** is available, try the
UEFI USB boot entry first—the hybrid ISO includes an EFI payload in the
Limine layout. If boot fails, capture the screen or serial garbage and
iterate (do not claim PASS without a `[ZPL-BOOT]` line).

### 11.3 Writing the ISO from Windows (Rufus)

These steps are **theoretical** until `zpl-kernel.iso` exists (xorriso
gate on the build host):

1. Install **Rufus** from the official site (`https://rufus.ie/` —
   verify HTTPS and publisher before run).
2. Insert the USB key; **select the correct device** in Rufus (disk
   size is your sanity check).
3. **Boot selection:** pick `zpl-kernel.iso` from `artifacts\iso\`.
4. Image mode: for hybrid ISOs, Rufus may offer **ISO** or **DD**—if the
   first attempt does not show Limine, retry with **DD** mode on a
   disposable stick (DD overwrites partition layout aggressively).
5. Start; wait until complete; **eject** safely.

### 11.4 Writing the ISO with Ventoy (alternative)

**Ventoy** (`https://www.ventoy.net/`) formats a stick once, then you
copy ISO files. For a **single experimental** ISO this can be heavier
than Rufus; use it if you already maintain a Ventoy stick for multiple
images.

1. Install Ventoy to the USB drive from the official Windows installer.
2. Copy `zpl-kernel.iso` into the exFAT/FAT partition Ventoy exposes.
3. Boot the stick, pick the ISO from Ventoy’s menu.

If Limine does not chain correctly under Ventoy, fall back to Rufus DD
for a controlled baseline.

### 11.5 Physical boot order

1. Insert USB; power on; open **one-shot boot menu** (often **F11** /
   **F12**).
2. Choose the **USB mass storage** entry (UEFI or legacy as available).
3. Limine menu should appear; select the default entry that loads
   `boot/zpl-kernel.elf` per your `limine.cfg` staging.
4. Watch serial (preferred) or HDMI for `[ZPL-BOOT]` markers per
   [`MARKERS.md`](./MARKERS.md).

### 11.6 eMMC vs USB (gotcha)

Internal **32 GB eMMC** is easy to confuse with a USB disk in imaging
tools. **Never** point Rufus/dd at `PhysicalDrive` entries you have not
identified by size + removal. When in doubt, unplug all other USB
storage and use only the target stick.

### 11.7 What “PASS” means on hardware

**PASS verified** on this platform means: **serial or captured log**
shows the same **ordered markers** you already assert in QEMU smoke
(e.g. `kernel_entry reached`, serial init, halt loop), with no
unexpected reboot loop for at least one full boot. Anything less is
**not** a PASS—record the partial logs instead, with the last
line that reached the screen.
