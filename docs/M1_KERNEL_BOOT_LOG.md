# M1 Kernel Boot Log

## Build

```powershell
$env:RUSTUP_OFFLINE='1'
cargo -Zjson-target-spec check -p zpl-kernel -p zpl-kernel-bin
cargo -Zjson-target-spec build -p zpl-kernel-bin --release
```

## ELF Verification

```powershell
& "$env:USERPROFILE\.rustup\toolchains\nightly-x86_64-pc-windows-msvc\lib\rustlib\x86_64-pc-windows-msvc\bin\llvm-objdump.exe" -f "target/x86_64-zpl-kernel/release/zpl-kernel-bin"
& "$env:USERPROFILE\.rustup\toolchains\nightly-x86_64-pc-windows-msvc\lib\rustlib\x86_64-pc-windows-msvc\bin\llvm-readobj.exe" --file-headers "target/x86_64-zpl-kernel/release/zpl-kernel-bin"
```

Expected:
- Format `elf64-x86-64`
- Type `Executable`
- Machine `EM_X86_64`
- Entry non-zero

## QEMU Run

```powershell
.\scripts\qemu-run.ps1
```

Debug mode:

```powershell
.\scripts\qemu-run.ps1 -Debug
```

Serial output is written to:
- `artifacts/kernel/boot-run-YYYYMMDD-HHMMSS.log`

Debug output (if enabled):
- `artifacts/kernel/qemu-debug-YYYYMMDD-HHMMSS.log`

## Acceptance Markers

The serial log must include:
- `[ZPL-BOOT] kernel_entry reached`
- `[ZPL-BOOT] serial initialized`
- `[ZPL-BOOT] halt loop entered`
