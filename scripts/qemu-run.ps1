param(
  [string]$KernelImage = ".\\target\\x86_64-zpl-kernel\\release\\zpl-kernel-bin",
  [int]$MemoryMb = 256,
  [switch]$Debug,
  # Stop the guest after this many seconds. 0 keeps the old behaviour: wait for it to
  # exit on its own.
  #
  # The default build never exits on its own -- its scheduler keeps deciding, which is
  # the point of it -- so a caller that just waits waits forever. That is how
  # `determinism-test.sh` came to hang: it ran three of these in a row with nothing to
  # stop them, and only ever "passed" on machines where QEMU was missing and the whole
  # step was skipped. A log of several megabytes was the only sign.
  [int]$TimeoutSeconds = 0
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($MemoryMb -lt 64) {
  throw "[E_QEMU_001_MEMORY_TOO_LOW] MemoryMb must be >= 64."
}

$kernelPath = Resolve-Path -Path $KernelImage -ErrorAction SilentlyContinue
if (-not $kernelPath) {
  throw "[E_QEMU_002_KERNEL_NOT_FOUND] Kernel image not found: $KernelImage"
}

$qemuCmd = Get-Command "qemu-system-x86_64" -ErrorAction SilentlyContinue
if (-not $qemuCmd) {
  throw "[E_QEMU_003_QEMU_MISSING] qemu-system-x86_64 is not installed or not on PATH."
}

$artifactDir = Join-Path (Get-Location) "artifacts\\kernel"
if (-not (Test-Path -Path $artifactDir)) {
  New-Item -ItemType Directory -Path $artifactDir | Out-Null
}
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$serialLogPath = Join-Path $artifactDir "boot-run-$timestamp.log"

$commonArgs = @(
  "-netdev", "user,id=n0",
  "-device", "virtio-net-pci,netdev=n0",
  "-serial", "file:$serialLogPath",
  "-display", "none",
  "-no-reboot",
  "-no-shutdown",
  "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
  "-m", "$MemoryMb"
)

if ($Debug) {
  $debugLogPath = Join-Path $artifactDir "qemu-debug-$timestamp.log"
  $commonArgs += @(
    "-d", "int,cpu_reset,guest_errors",
    "-D", "$debugLogPath"
  )
}

Write-Output "Starting QEMU kernel run"
Write-Output "QEMU: $($qemuCmd.Source)"
Write-Output "Kernel: $($kernelPath.Path)"
Write-Output "Memory: $MemoryMb MB"
Write-Output "Serial log: $serialLogPath"
if ($Debug) {
  Write-Output "Debug log: $debugLogPath"
}
# Returns the exit code and nothing else. A `Write-Output` in here would be *part of*
# the return value -- PowerShell collects everything a function emits -- which is how
# the first version of this ended up throwing "exited with code Stopped the guest
# after 20s ... 0". The notice is printed by the caller.
$script:StoppedByTimeout = $false
function Invoke-Qemu {
  param([string[]]$QemuArgs)
  if ($TimeoutSeconds -le 0) {
    & $qemuCmd.Source @QemuArgs
    return $LASTEXITCODE
  }
  $proc = Start-Process -FilePath $qemuCmd.Source -ArgumentList $QemuArgs -NoNewWindow -PassThru
  if ($proc.WaitForExit($TimeoutSeconds * 1000)) {
    return $proc.ExitCode
  }
  Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
  $script:StoppedByTimeout = $true
  # Not a failure: the caller asked for a bounded run and got one. Whether the boot was
  # any good is decided by reading the serial log, which is where it was decided anyway.
  return 0
}

$kernelArgs = @("-kernel", "$($kernelPath.Path)") + $commonArgs
$code = Invoke-Qemu -QemuArgs $kernelArgs
if ($script:StoppedByTimeout) {
  Write-Output "Stopped the guest after ${TimeoutSeconds}s (it does not exit on its own)."
}
if ($code -eq 0) {
  return
}

Write-Output "Kernel mode failed (exit=$code), retrying with ELF loader mode."
$loaderArgs = @(
  "-machine", "q35",
  "-device", "loader,file=$($kernelPath.Path)"
) + $commonArgs
$code = Invoke-Qemu -QemuArgs $loaderArgs
if ($code -ne 0) {
  throw "[E_QEMU_004_PROCESS_FAILED] qemu-system-x86_64 exited with code $code."
}
