param(
  [int]$TimeoutMinutes = 10
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

$started = Get-Date
Write-Output "==> build-all start: $started"

function Assert-Timeout {
  $elapsed = (Get-Date) - $started
  if ($elapsed.TotalMinutes -gt $TimeoutMinutes) {
    throw "[E_BUILD_ALL_TIMEOUT] build-all exceeded $TimeoutMinutes minutes"
  }
}

function Run-Step([string]$name, [scriptblock]$block) {
  Assert-Timeout
  Write-Output ""
  Write-Output "==> $name"
  & $block
  if ($LASTEXITCODE -ne 0) {
    throw "[E_BUILD_ALL_STEP] $name failed with exit $LASTEXITCODE"
  }
}

Run-Step "cargo clean" {
  cargo clean
}

Run-Step "cargo check --workspace" {
  cargo check --workspace
}

Run-Step "cargo test --workspace (exclude bare-metal-only crates)" {
  cargo test --workspace --target x86_64-pc-windows-msvc --exclude zpl-kernel-bin --exclude limine-bridge
}

Run-Step "cargo build -p zpl-kernel-bin --release (custom target)" {
  cargo "-Zbuild-std=core,compiler_builtins,alloc" "-Zbuild-std-features=compiler-builtins-mem" "-Zjson-target-spec" build -p zpl-kernel-bin --target "crates/zpl-kernel/x86_64-zpl-kernel.json" --release
}

# The bare-metal target again, with warnings fatal.
#
# `cargo clippy --workspace` does NOT cover this target -- it builds for the
# host -- so a warning that only exists in the kernel build passes every other
# gate silently. One did: an unused static shipped in a commit and was only
# noticed when the bare-metal output was read by hand. The build above stays as
# it is so a failure here is clearly about warnings and nothing else.
#
# The linker script has to be repeated here. A RUSTFLAGS environment variable
# REPLACES `target.<triple>.rustflags` from `.cargo/config.toml`, it does not
# add to it -- so setting just "-D warnings" silently dropped
# `-Tcrates/zpl-kernel/linker.ld`, rust-lld fell back to looking for `_start`,
# and this step relinked the artifact without the kernel's memory layout. The
# entry-address check below is what makes that impossible to miss again.
$linkerScript = "-C link-arg=-Tcrates/zpl-kernel/linker.ld"
Run-Step "bare-metal build with warnings denied (clippy does not cover this target)" {
  $prev = $env:RUSTFLAGS
  $env:RUSTFLAGS = "-D warnings $linkerScript"
  try {
    cargo "-Zbuild-std=core,compiler_builtins,alloc" "-Zbuild-std-features=compiler-builtins-mem" "-Zjson-target-spec" build -p zpl-kernel-bin --target "crates/zpl-kernel/x86_64-zpl-kernel.json" --release
  } finally {
    $env:RUSTFLAGS = $prev
  }
}

$artifact = Join-Path $root "target/x86_64-zpl-kernel/release/zpl-kernel-bin"
if (-not (Test-Path $artifact)) {
  throw "[E_BUILD_ALL_ARTIFACT] Missing artifact: $artifact"
}

$llvmDir = Join-Path $env:USERPROFILE ".rustup/toolchains/nightly-x86_64-pc-windows-msvc/lib/rustlib/x86_64-pc-windows-msvc/bin"
$objdump = Join-Path $llvmDir "llvm-objdump.exe"

Run-Step "ELF gate (llvm-objdump -f)" {
  $header = & $objdump -f $artifact
  $header
  # `ENTRY(_zpl_qemu_boot)` in crates/zpl-kernel/linker.ld puts the entry in the
  # kernel's load region. A link that lost the script reports `start address
  # 0x0000000000000000`, which boots nothing -- and used to pass this gate.
  $match = $header | Select-String -Pattern 'start address:?\s+(0x[0-9a-fA-F]+)' | Select-Object -First 1
  if (-not $match) { throw "[E_BUILD_ALL_ELF_001] llvm-objdump printed no start address." }
  $start = $match.Matches[0].Groups[1].Value
  if ($start -eq "0x0000000000000000") {
    throw "[E_BUILD_ALL_ELF_002] Entry address is zero: the linker script was not applied."
  }
  "entry: $start"
}

Run-Step "QEMU smoke (PowerShell fallback)" {
  $before = Get-ChildItem -Path "artifacts/kernel" -Filter "boot-run-*.log" -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime |
    Select-Object -Last 1
  $qemuProc = Start-Process -FilePath "powershell" -ArgumentList @(
    "-ExecutionPolicy", "Bypass", "-File", "scripts/qemu-run.ps1"
  ) -NoNewWindow -PassThru
  if (-not $qemuProc.WaitForExit(20000)) {
    Stop-Process -Id $qemuProc.Id -Force
  }
  Get-Process | Where-Object { $_.ProcessName -like "qemu-system-x86_64*" } | ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
  Start-Sleep -Milliseconds 1000

  $latest = Get-ChildItem -Path "artifacts/kernel" -Filter "boot-run-*.log" |
    Sort-Object LastWriteTime |
    Select-Object -Last 1
  if (-not $latest) { throw "[E_BUILD_ALL_QEMU_001] Missing boot-run log." }
  if ($before -and $latest.FullName -eq $before.FullName -and $latest.LastWriteTime -le $before.LastWriteTime) {
    throw "[E_BUILD_ALL_QEMU_002] No new boot-run log."
  }

  $log = Get-Content -Path $latest.FullName -Raw
  if ([string]::IsNullOrEmpty($log)) {
    throw "[E_BUILD_ALL_QEMU_000] Empty boot-run log: $($latest.FullName)"
  }
  if (-not $log.Contains("[ZPL-BOOT] kernel_entry reached")) { throw "[E_BUILD_ALL_QEMU_003] Missing kernel_entry marker." }
  if (-not $log.Contains("[ZPL-BOOT] serial initialized")) { throw "[E_BUILD_ALL_QEMU_004] Missing serial marker." }
  if (-not $log.Contains("[ZPL-BOOT] halt loop entered")) { throw "[E_BUILD_ALL_QEMU_005] Missing halt marker." }
  # Shape, not value: the old text was a fixed string in the kernel, so this
  # check could never fail. The marker now reports what the machine offers.
  if (-not ($log -match "\[ZPL-FRAME\] init base=0x[0-9a-f]+ cap=[0-9]+")) { throw "[E_BUILD_ALL_QEMU_006] Missing frame_alloc init marker." }
  if (-not $log.Contains("[ZPL-FRAME] burst alloc=1000 free=1000 leak=0")) { throw "[E_BUILD_ALL_QEMU_007] Missing frame_alloc burst marker." }
  if (-not $log.Contains("[ZPL-FRAME] stress=1000000 leak=0")) { throw "[E_BUILD_ALL_QEMU_008] Missing frame_alloc stress marker." }
  if (-not $log.Contains("[ZPL-HEAP] selfcheck box+vec100 OK")) { throw "[E_BUILD_ALL_QEMU_021] Missing heap selfcheck marker." }
  if (-not $log.Contains("[ZPL-AUDIT] selfcheck append=5 verify=ok tamper_detected OK")) { throw "[E_BUILD_ALL_QEMU_022] Missing audit chain selfcheck marker." }
  if (-not $log.Contains("[ZPL-SMP max_leaf=")) { throw "[E_BUILD_ALL_QEMU_023] Missing SMP probe marker." }
  if (-not $log.Contains("[ZPL-PERF phase=0 name=kernel_entry")) { throw "[E_BUILD_ALL_QEMU_024] Missing rdtsc phase profiler marker." }
  if (-not $log.Contains("[ZPL-PAGING] init pdpt[1]+pd[0] ok")) { throw "[E_BUILD_ALL_QEMU_009] Missing paging init marker." }
  if (-not $log.Contains("[ZPL-PAGING] map_4k virt=0x40000000 ok")) { throw "[E_BUILD_ALL_QEMU_010] Missing paging map marker." }
  if (-not $log.Contains("[ZPL-PAGING] write_read_match val=0xcafebabedeadbeef")) { throw "[E_BUILD_ALL_QEMU_011] Missing paging verify marker." }
  if (-not $log.Contains("[ZPL-PAGING] unmap virt=0x40000000 ok")) { throw "[E_BUILD_ALL_QEMU_012] Missing paging unmap marker." }
  if (-not $log.Contains("[ZPL-PAGING] page_fault virt=0x40000000 reread ok")) { throw "[E_BUILD_ALL_QEMU_013] Missing paging pf marker." }
  if (-not $log.Contains("[ZPL-RAMFS] selfcheck create+write+close+reopen+read OK")) { throw "[E_BUILD_ALL_QEMU_014] Missing ramfs selfcheck marker." }
  if (-not $log.Contains("[ZPL-PROC] selfcheck spawn=3 wait=3 exit_codes_sum=43 live=0 OK")) { throw "[E_BUILD_ALL_QEMU_015] Missing process table selfcheck marker." }
  if (-not $log.Contains("[ZPL-IPC] selfcheck pipe+send+recv ipc-roundtrip OK")) { throw "[E_BUILD_ALL_QEMU_016] Missing ipc selfcheck marker." }
  if (-not $log.Contains("[ZPL-SHM] selfcheck allow_clean+block_hostile OK")) { throw "[E_BUILD_ALL_QEMU_017] Missing shm selfcheck marker." }
  if (-not $log.Contains("[ZPL-MATRIX] selfcheck 8ops naive==simd 64x64 OK")) { throw "[E_BUILD_ALL_QEMU_018] Missing matrix selfcheck marker." }
  if (-not $log.Contains("[ZPL-HAL] selfcheck block_rw+net_tx_rx OK")) { throw "[E_BUILD_ALL_QEMU_019] Missing hal selfcheck marker." }
  if (-not $log.Contains("[ZPL-VNET] probe")) { throw "[E_BUILD_ALL_QEMU_025] Missing virtio-net probe marker." }
  if (-not $log.Contains("[ZPL-RUNQ] selfcheck N=5 ticks=10 winner=task0_wins=10 OK")) { throw "[E_BUILD_ALL_QEMU_020] Missing runqueue selfcheck marker." }
  $global:LASTEXITCODE = 0
}

Write-Output ""
Write-Output "==> build-all PASS"
Write-Output "Elapsed: $(((Get-Date) - $started).ToString())"
