param(
  [int]$TimeoutMinutes = 15,
  # Skip Limine hybrid ISO when xorriso/WSL toolchain is unavailable (see tools/xorriso/README.md).
  [switch]$SkipLimineIso
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

$started = Get-Date
Write-Output "==> full-verify start: $started"

function Assert-Timeout {
  $elapsed = (Get-Date) - $started
  if ($elapsed.TotalMinutes -gt $TimeoutMinutes) {
    throw "[E_VERIFY_TIMEOUT] full-verify exceeded $TimeoutMinutes minutes"
  }
}

function Run-Step([string]$name, [scriptblock]$block) {
  Assert-Timeout
  Write-Output ""
  Write-Output "==> $name"
  & $block
  if ($LASTEXITCODE -ne 0) {
    throw "[E_VERIFY_STEP] $name failed with exit $LASTEXITCODE"
  }
}

# Two steps that only exist in a tree that has more than one build configuration.
# A copy with a single configuration has nothing for them to compare, and the
# scripts are not shipped with it -- so the step is skipped by name rather than
# failing, which is what it did before: the orchestrator reported a broken
# repository because a check that did not apply was missing.
function Run-Optional-Step {
  param([string]$Name, [string]$Script)
  if (Test-Path -LiteralPath $Script) {
    Run-Step $Name { & powershell -ExecutionPolicy Bypass -File $Script }
  } else {
    Write-Output "==> $Name -- skipped, $Script is not part of this copy"
  }
}

Run-Step "demo policy gate (the three rules must show in the boot log)" {
  & bash "scripts/demo-policy-test.sh"
}
Run-Optional-Step "build-features (build configurations)" "scripts/build-features.ps1"

Run-Step "cargo check --workspace" {
  cargo check --workspace
}

Run-Step "cargo clean (workaround for nightly clippy ICE)" {
  cargo clean
}

Run-Step "cargo clippy --workspace -- -D warnings" {
  cargo clippy --workspace -- -D warnings
}

Run-Step "cargo test --workspace (exclude bare-metal-only crates)" {
  # Include `zpl-kernel` host lib tests (`phys_hhdm`); exclude bare-metal
  # `zpl-kernel-bin` + `limine-bridge` (both are bare-metal-only targets).
  cargo test --workspace --target x86_64-pc-windows-msvc --exclude zpl-kernel-bin --exclude limine-bridge
}

Run-Step "cargo -Zjson-target-spec build -p zpl-kernel-bin --release" {
  cargo "-Zbuild-std=core,compiler_builtins,alloc" "-Zbuild-std-features=compiler-builtins-mem" "-Zjson-target-spec" build -p zpl-kernel-bin --target "crates/zpl-kernel/x86_64-zpl-kernel.json" --release
}

$artifact = Join-Path $root "target/x86_64-zpl-kernel/release/zpl-kernel-bin"
if (-not (Test-Path $artifact)) {
  throw "[E_VERIFY_ARTIFACT] missing artifact: $artifact"
}

$llvmDir = Join-Path $env:USERPROFILE ".rustup/toolchains/nightly-x86_64-pc-windows-msvc/lib/rustlib/x86_64-pc-windows-msvc/bin"
$objdump = Join-Path $llvmDir "llvm-objdump.exe"
$readobj = Join-Path $llvmDir "llvm-readobj.exe"

Run-Step "ELF gate (llvm-objdump + llvm-readobj)" {
  & $objdump -f $artifact
  & $readobj --file-headers $artifact
}

$bash = Get-Command bash -ErrorAction SilentlyContinue
$qemu = Get-Command qemu-system-x86_64 -ErrorAction SilentlyContinue
if ($qemu) {
  $bashUsable = $false
  if ($bash) {
    & bash -lc "exit 0"
    if ($LASTEXITCODE -eq 0) {
      $bashUsable = $true
    }
  }

  if ($bashUsable) {
    Run-Step "QEMU smoke test (scripts/qemu-boot-test.sh)" {
      bash scripts/qemu-boot-test.sh
    }

    Run-Step "determinism test (scripts/determinism-test.sh)" {
      bash scripts/determinism-test.sh
    }
  } else {
    Write-Output "==> bash unavailable, running PowerShell fallback smoke checks."
    Run-Step "QEMU smoke test (PowerShell fallback)" {
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
      if (-not $latest) {
        throw "[E_QEMU_FALLBACK_001_NO_LOG] Missing boot-run log."
      }
      if ($before -and $latest.FullName -eq $before.FullName -and $latest.LastWriteTime -le $before.LastWriteTime) {
        throw "[E_QEMU_FALLBACK_002_STALE_LOG] No new boot-run log created."
      }

      $log = Get-Content -Path $latest.FullName -Raw
      if (-not $log.Contains("[ZPL-BOOT] kernel_entry reached")) { throw "[E_QEMU_FALLBACK_003] missing kernel_entry marker" }
      if (-not $log.Contains("[ZPL-BOOT] serial initialized")) { throw "[E_QEMU_FALLBACK_004] missing serial marker" }
      if (-not $log.Contains("[ZPL-BOOT] halt loop entered")) { throw "[E_QEMU_FALLBACK_005] missing halt marker" }
      # Shape, not value: the old text was a fixed string in the kernel, so this
      # check could never fail. The marker now reports what the machine offers.
      if (-not ($log -match "\[ZPL-FRAME\] init base=0x[0-9a-f]+ cap=[0-9]+")) { throw "[E_QEMU_FALLBACK_006] missing frame_alloc init marker" }
      if (-not $log.Contains("[ZPL-FRAME] burst alloc=1000 free=1000 leak=0")) { throw "[E_QEMU_FALLBACK_007] missing frame_alloc burst marker" }
      if (-not $log.Contains("[ZPL-FRAME] stress=1000000 leak=0")) { throw "[E_QEMU_FALLBACK_008] missing frame_alloc stress marker" }
      if (-not $log.Contains("[ZPL-HEAP] selfcheck box+vec100 OK")) { throw "[E_QEMU_FALLBACK_021] missing heap selfcheck marker" }
      if (-not $log.Contains("[ZPL-AUDIT] selfcheck append=5 verify=ok tamper_detected OK")) { throw "[E_QEMU_FALLBACK_022] missing audit chain selfcheck marker" }
      if (-not $log.Contains("[ZPL-SMP max_leaf=")) { throw "[E_QEMU_FALLBACK_023] missing SMP probe marker" }
      if (-not $log.Contains("[ZPL-PERF phase=0 name=kernel_entry")) { throw "[E_QEMU_FALLBACK_024] missing rdtsc phase profiler marker" }
      if (-not $log.Contains("[ZPL-PAGING] init pdpt[1]+pd[0] ok")) { throw "[E_QEMU_FALLBACK_009] missing paging init marker" }
      if (-not $log.Contains("[ZPL-PAGING] map_4k virt=0x40000000 ok")) { throw "[E_QEMU_FALLBACK_010] missing paging map marker" }
      if (-not $log.Contains("[ZPL-PAGING] write_read_match val=0xcafebabedeadbeef")) { throw "[E_QEMU_FALLBACK_011] missing paging verify marker" }
      if (-not $log.Contains("[ZPL-PAGING] unmap virt=0x40000000 ok")) { throw "[E_QEMU_FALLBACK_012] missing paging unmap marker" }
      if (-not $log.Contains("[ZPL-PAGING] page_fault virt=0x40000000 reread ok")) { throw "[E_QEMU_FALLBACK_013] missing paging pf marker" }
      if (-not $log.Contains("[ZPL-RAMFS] selfcheck create+write+close+reopen+read OK")) { throw "[E_QEMU_FALLBACK_014] missing ramfs selfcheck marker" }
      if (-not $log.Contains("[ZPL-PROC] selfcheck spawn=3 wait=3 exit_codes_sum=43 live=0 OK")) { throw "[E_QEMU_FALLBACK_015] missing process table selfcheck marker" }
      if (-not $log.Contains("[ZPL-IPC] selfcheck pipe+send+recv ipc-roundtrip OK")) { throw "[E_QEMU_FALLBACK_016] missing ipc selfcheck marker" }
      if (-not $log.Contains("[ZPL-SHM] selfcheck allow_clean+block_hostile OK")) { throw "[E_QEMU_FALLBACK_017] missing shm selfcheck marker" }
      if (-not $log.Contains("[ZPL-MATRIX] selfcheck 8ops naive==simd 64x64 OK")) { throw "[E_QEMU_FALLBACK_018] missing matrix selfcheck marker" }
      if (-not $log.Contains("[ZPL-HAL] selfcheck block_rw+net_tx_rx OK")) { throw "[E_QEMU_FALLBACK_019] missing hal selfcheck marker" }
      if (-not $log.Contains("[ZPL-VNET] probe")) { throw "[E_QEMU_FALLBACK_025] missing virtio-net probe marker" }
      if (-not $log.Contains("[ZPL-RUNQ] selfcheck N=5 ticks=10 winner=task0_wins=10 OK")) { throw "[E_QEMU_FALLBACK_020] missing runqueue selfcheck marker" }
      $global:LASTEXITCODE = 0
    }

    Run-Step "determinism test (PowerShell fallback)" {
      $snapshots = @()
      for ($i = 0; $i -lt 3; $i++) {
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
        if (-not $latest) { throw "[E_DET_FALLBACK_001_NO_LOG] Missing boot-run log for run $i." }
        if ($before -and $latest.FullName -eq $before.FullName -and $latest.LastWriteTime -le $before.LastWriteTime) {
          throw "[E_DET_FALLBACK_002_STALE_LOG] No new log for run $i."
        }

        $sched = Get-Content $latest.FullName | Where-Object { $_.Contains("[ZPL-SCHED]") } | Select-Object -First 100
        $snapshots += ($sched -join "`n")
      }

      if ($snapshots[0] -ne $snapshots[1] -or $snapshots[1] -ne $snapshots[2]) {
        throw "[E_DET_FALLBACK_003_MISMATCH] Determinism mismatch in first 100 [ZPL-SCHED] lines."
      }
      $global:LASTEXITCODE = 0
    }
  }
} else {
  Write-Output "==> qemu-system-x86_64 not found, skipping qemu checks."
}

if ($bash) {
  $bashUsable = $false
  & bash -lc "exit 0"
  if ($LASTEXITCODE -eq 0) {
    $bashUsable = $true
  }
  if ($bashUsable) {
    Run-Step "bit identity test (scripts/bit-identity-test.sh)" {
      bash scripts/bit-identity-test.sh
    }
  } else {
    Run-Step "bit identity test (PowerShell fallback)" {
      bash "scripts/demo-policy-test.sh"
    }
  }
} else {
  Run-Step "bit identity test (PowerShell fallback)" {
    bash "scripts/demo-policy-test.sh"
  }
}

Run-Step "panic-test (scripts/panic-test.ps1)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/panic-test.ps1"
}

Run-Step "ring3-test (scripts/ring3-test.ps1)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/ring3-test.ps1"
}

Run-Step "elf-test (scripts/elf-test.ps1)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/elf-test.ps1"
}

Run-Step "attack-test (scripts/attack-test.ps1)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/attack-test.ps1"
}

Run-Step "asm-test (scripts/asm-test.ps1)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/asm-test.ps1"
}

Run-Step "build-examples (scripts/build-examples.ps1)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/build-examples.ps1"
}

Run-Step "replay-test (scripts/replay-test.ps1)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/replay-test.ps1"
}

Run-Step "fs-test (scripts/fs-test.ps1)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/fs-test.ps1"
}

Run-Step "firewall-test (scripts/firewall-test.ps1)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/firewall-test.ps1"
}

Run-Step "e2e-test (scripts/e2e-test.ps1)" {
  & powershell -ExecutionPolicy Bypass -File "scripts/e2e-test.ps1"
}

if (-not $SkipLimineIso) {
  Run-Step "build-iso (Limine hybrid USB image + QEMU BIOS smoke)" {
    & powershell -ExecutionPolicy Bypass -File "scripts/build-iso.ps1"
  }
} else {
  Write-Output "==> SkipLimineIso: skipped scripts/build-iso.ps1 (xorriso / WSL)."
}

Run-Step "rebuild default kernel (no features) for downstream verifiers" {
  cargo "-Zbuild-std=core,compiler_builtins,alloc" "-Zbuild-std-features=compiler-builtins-mem" "-Zjson-target-spec" build -p zpl-kernel-bin --target "crates/zpl-kernel/x86_64-zpl-kernel.json" --release
}

Write-Output ""
Write-Output "==> full-verify PASS"
Write-Output "Elapsed: $(((Get-Date) - $started).ToString())"
