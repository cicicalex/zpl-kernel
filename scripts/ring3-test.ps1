param(
  [int]$TimeoutSeconds = 25
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

Write-Output "==> ring3-test build (zpl-kernel-bin --features ring3_demo)"
& cargo "-Zbuild-std=core,compiler_builtins,alloc" "-Zbuild-std-features=compiler-builtins-mem" "-Zjson-target-spec" build -p zpl-kernel-bin --features ring3_demo --target "crates/zpl-kernel/x86_64-zpl-kernel.json" --release
if ($LASTEXITCODE -ne 0) {
  throw "[E_RING3_TEST_BUILD_001] kernel build with ring3_demo failed (exit $LASTEXITCODE)"
}

$artifact = Join-Path $root "target/x86_64-zpl-kernel/release/zpl-kernel-bin"
if (-not (Test-Path $artifact)) {
  throw "[E_RING3_TEST_BUILD_002] missing kernel artifact: $artifact"
}

$qemuCmd = Get-Command "qemu-system-x86_64" -ErrorAction SilentlyContinue
if (-not $qemuCmd) {
  throw "[E_RING3_TEST_QEMU_001] qemu-system-x86_64 not on PATH"
}

$artifactDir = Join-Path $root "artifacts/kernel"
if (-not (Test-Path $artifactDir)) {
  New-Item -ItemType Directory -Path $artifactDir | Out-Null
}
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$serialLog = Join-Path $artifactDir "ring3-run-$timestamp.log"

$qemuArgs = @(
  "-kernel", "$artifact",
  "-serial", "file:$serialLog",
  "-display", "none",
  "-no-reboot",
  "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
  "-m", "256"
)

Write-Output "==> ring3-test QEMU run, log=$serialLog"
$argString = ($qemuArgs | ForEach-Object {
  if ($_ -match '\s') { '"{0}"' -f $_ } else { $_ }
}) -join ' '
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $qemuCmd.Source
$psi.Arguments = $argString
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$qemuProc = [System.Diagnostics.Process]::Start($psi)
if (-not $qemuProc.WaitForExit($TimeoutSeconds * 1000)) {
  Stop-Process -Id $qemuProc.Id -Force
  throw "[E_RING3_TEST_QEMU_002] QEMU did not exit within $TimeoutSeconds s"
}
$exit = $qemuProc.ExitCode
Write-Output "==> ring3-test QEMU exit code: $exit"

if ($exit -ne 1) {
  throw "[E_RING3_TEST_EXIT_001] expected QEMU exit code 1 (qemu_exit_success), got $exit"
}

Start-Sleep -Milliseconds 500
$log = Get-Content -Path $serialLog -Raw
$expected = @(
  "[ZPL-BOOT] kernel_entry reached",
  "[ZPL-BOOT] serial initialized",
  "[ZPL-RING] tss installed selector=0x28",
  "[ZPL-RING] ring3 prepare ok",
  "[ZPL-RING] kernel->ring3 iretq triggered",
  "[ZPL-SYSCALL] log: hi",
  "[ZPL-SYSCALL] exit code=0",
  "[ZPL-RING] qemu_exit=success"
)
foreach ($marker in $expected) {
  if (-not $log.Contains($marker)) {
    throw "[E_RING3_TEST_MARKER_001] missing marker: $marker"
  }
}

Write-Output "==> ring3-test PASS"
