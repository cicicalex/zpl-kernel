# xorriso (Limine hybrid ISO)

Limine’s hybrid BIOS/UEFI ISO flow requires **xorriso** (see Limine's own upstream documentation).

`scripts/build-iso.ps1` resolves xorriso in this order:

1. `tools/xorriso/xorriso.exe` in this repo (drop a Windows build here), or  
2. `xorriso` / `xorriso.exe` on `PATH`, or  
3. `xorriso` inside **WSL**, but only if `wsl -l -q` prints **at least one distro name**; then e.g. `sudo apt install xorriso` in that distro.

If none of the above exist, or WSL is installed but **`wsl -l -q` is empty**, the script fails with **`[E_ISO_004_NEED_XORRISO]`** (see also **`[E_ISO_005_WSL_XORRISO]`** when a distro exists but the `xorriso` package is missing).

`scripts/full-verify.ps1` runs `build-iso.ps1` unless you pass **`-SkipLimineIso`**.

Options to obtain xorriso on Windows:

- Install a WSL Ubuntu/Debian and `sudo apt install xorriso` (simplest).  
- Use an MSYS2 / Cygwin environment that provides `xorriso` and ensure it is on `PATH` when you run PowerShell.  
- Place a vendor `xorriso.exe` under `tools/xorriso/` (project does not ship the binary).

## Related docs

- [`docs/PHYSICAL_TEST_GUIDE.md`](../../docs/PHYSICAL_TEST_GUIDE.md) — hybrid ISO,
  USB imaging, optional QEMU CD smoke, `[E_ISO_*]` codes (**no fake PASS**
  without the toolchain).
- [`docs/PHYSICAL_TEST_GUIDE.md`](../../docs/PHYSICAL_TEST_GUIDE.md) — **Script /
  automation gates** table (`build-iso.ps1`, `full-verify.ps1`).
- [`docs/MARKERS.md`](../../docs/MARKERS.md) — COM1 marker catalogue; see
  **ISO vs QEMU `-kernel` (Limine)** for the ISO boot path vs `zpl-kernel-bin`.

Bump **Limine** ZIP tag/SHA inside `scripts/build-iso.ps1` when upgrading the bootloader blob.
