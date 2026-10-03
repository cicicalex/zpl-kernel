param(
  [switch]$SkipQemu,
  [switch]$SkipLimineDownload
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Pinned Limine binary release — bump tag + SHA when upgrading (GitHub Asset digest.sha256).
$LimineTag = "v12.2.0"
$LimineBinaryZipUrl = "https://github.com/Limine-Bootloader/Limine/releases/download/$LimineTag/limine-binary.zip"
$LimineBinaryZipSha256 = "6a7d1daaa8fd336dd4dc477db1d2533dc070b9f5cba36926a9be9e3fbb438635"
$MaxIsoBytes = [int64](20 * 1024 * 1024)

$root = (Resolve-Path "$PSScriptRoot/..").Path
Set-Location $root

Write-Output "==> build-iso: repo root=$root"

function Get-Sha256File([string]$path) {
  $h = Get-FileHash -Path $path -Algorithm SHA256
  return ($h.Hash).ToLowerInvariant()
}

function Test-Sha256([string]$path, [string]$expectedLower) {
  $got = Get-Sha256File $path
  if ($got -ne $expectedLower) {
    throw "[E_ISO_SHA256_MISMATCH] Expected $expectedLower got $got for $path"
  }
}

function Resolve-LimineHostExe {
   param([string]$LimineUnpackRoot)
   $candidates = @(Get-ChildItem -LiteralPath $LimineUnpackRoot -Recurse -File -Filter "limine.exe" -ErrorAction SilentlyContinue |
     Where-Object { $_.FullName -match "tool-windows" })
   if ($candidates.Length -eq 1) {
     return $candidates[0].FullName
   }
   if ($candidates.Length -gt 1) {
     return ($candidates | Sort-Object FullName)[0].FullName
   }
   throw "[E_ISO_NO_LIMINE_EXE] limine.exe (windows tool) not found under extracted Limine binaries."
 }

function Invoke-LimineHybridXorriso {
   param(
     [string]$stagingWin,
     [string]$isoOutWin,
     [string]$biosCdRelative,
     [string]$efiCdRelative
   )

   # Limine upstream (USAGE.md BIOS/UEFI hybrid ISO).

   function InvokeNative([string]$xorrisoExe, [string]$stage, [string]$out) {
     $xorArgs = @(
       "-as", "mkisofs",
       "-R", "-r", "-J",
       "-b", $biosCdRelative,
       "-no-emul-boot",
       "-boot-load-size", "4",
       "-boot-info-table",
       "-hfsplus",
       "-apm-block-size", "2048",
       "--efi-boot", $efiCdRelative,
       "-efi-boot-part",
       "--efi-boot-image",
       "--protective-msdos-label",
       $stage,
       "-o", $out
     )
     $p = Start-Process -FilePath $xorrisoExe -ArgumentList $xorArgs -Wait -PassThru -NoNewWindow
     if ($p.ExitCode -ne 0) {
       throw "[E_ISO_MKISOFS_EXIT] $($xorrisoExe) exit $($p.ExitCode)"
     }
   }

   $bundledXor = Join-Path $root "tools/xorriso/xorriso.exe"
   if (Test-Path -LiteralPath $bundledXor) {
     Write-Output "==> xorriso: bundled $bundledXor"
     InvokeNative -xorrisoExe $bundledXor -stage $stagingWin -out $isoOutWin
     return
   }

   $pathXor = Get-Command "xorriso" -ErrorAction SilentlyContinue
   if (-not $pathXor) {
     $pathXor = Get-Command "xorriso.exe" -ErrorAction SilentlyContinue
   }
   if ($pathXor) {
     Write-Output "==> xorriso: $($pathXor.Source)"
     InvokeNative -xorrisoExe $pathXor.Source -stage $stagingWin -out $isoOutWin
     return
   }

  $wslExe = Get-Command "wsl" -ErrorAction SilentlyContinue
  if (-not $wslExe) {
    throw "[E_ISO_004_NEED_XORRISO] xorriso not found (tools/xorriso/xorriso.exe or PATH) and WSL is not installed."
  }
  $wslListed = (& wsl -l -q 2>&1 | Out-String).Trim()
  if ([string]::IsNullOrWhiteSpace($wslListed)) {
    throw '[E_ISO_004_NEED_XORRISO] xorriso missing and WSL has no Linux distribution registered. Install a distro (wsl --install or wsl --install -d Ubuntu), then sudo apt install xorriso, or place xorriso.exe under tools/xorriso/ (see tools/xorriso/README.md).'
  }
  Write-Output "==> xorriso: invoking via WSL (need `xorriso` package in the default distro)."
  & wsl -e bash -lc "command -v xorriso >/dev/null 2>&1"
  if ($LASTEXITCODE -ne 0) {
    throw "[E_ISO_005_WSL_XORRISO] WSL distro present but xorriso not installed. Example: sudo apt-get install xorriso"
  }
   # Convert Windows paths to WSL paths manually (avoid PowerShell escape issues with `wsl wslpath`).
   # Don't use Resolve-Path because the ISO output file doesn't exist yet.
   function Convert-WinToWsl([string]$winPath) {
     if (-not [System.IO.Path]::IsPathRooted($winPath)) {
       $winPath = Join-Path (Get-Location).Path $winPath
     }
     $drive = $winPath.Substring(0, 1).ToLower()
     $rest = $winPath.Substring(2) -replace '\\', '/'
     return "/mnt/$drive$rest"
   }
   $stagingUnix = Convert-WinToWsl $stagingWin
   $isoUnix = Convert-WinToWsl $isoOutWin
   # ISO root follows Limine upstream: last positional is staging directory (cwd after cd).
   $inner = "set -eu; xorriso -as mkisofs -R -r -J " +
     "-b '$biosCdRelative' -no-emul-boot -boot-load-size 4 -boot-info-table -hfsplus -apm-block-size 2048 " +
     "--efi-boot '$efiCdRelative' -efi-boot-part --efi-boot-image --protective-msdos-label " +
     "'.' -o '$isoUnix'"
   & wsl -e bash -lc "cd '$stagingUnix' && $inner"
   if ($LASTEXITCODE -ne 0) {
     throw "[E_ISO_MKISOFS_EXIT] wsl xorriso exit $LASTEXITCODE"
   }
 }

function Invoke-LimineBiosInstall {
   param([string]$LimineExe, [string]$IsoAbs)
   # Call operator: `Start-Process -Wait -PassThru` can throw "process has exited"
   # on fast-exiting limine.exe under StrictMode on some hosts.
   & $LimineExe @("bios-install", $IsoAbs)
   if ($LASTEXITCODE -ne 0) {
     throw "[E_ISO_006_LIMINE_BIOS_INSTALL] limine bios-install exit $($LASTEXITCODE)"
   }
 }

 $artifactsIso = Join-Path $root "artifacts/iso"
 if (-not (Test-Path $artifactsIso)) {
   New-Item -ItemType Directory -Force -Path $artifactsIso | Out-Null
 }

 $staging = Join-Path $artifactsIso "_staging_limine_iso"
 $limineUnpack = Join-Path $artifactsIso "limine-dist/$LimineTag"

 if (-not $SkipLimineDownload) {
   if (-not (Test-Path $limineUnpack)) {
     New-Item -ItemType Directory -Force -Path $limineUnpack | Out-Null
   }
   $zipPath = Join-Path $limineUnpack "limine-binary.zip"
   if (-not (Test-Path $zipPath)) {
     Write-Output "==> downloading $LimineBinaryZipUrl"
     Invoke-WebRequest -Uri $LimineBinaryZipUrl -OutFile $zipPath -UseBasicParsing
   }
   Test-Sha256 $zipPath $LimineBinaryZipSha256
   if (-not (Test-Path (Join-Path $limineUnpack "extracted-marker.txt"))) {
     Expand-Archive -LiteralPath $zipPath -DestinationPath $limineUnpack -Force
     "ok" | Set-Content (Join-Path $limineUnpack "extracted-marker.txt")
   }
 } else {
   if (-not (Test-Path $limineUnpack)) {
     throw "[E_ISO_SKIP_BUT_MISSING] SkipLimineDownload set but $($limineUnpack) missing."
   }
   $zipExisting = Join-Path $limineUnpack "limine-binary.zip"
   if (Test-Path -LiteralPath $zipExisting) {
     Test-Sha256 $zipExisting $LimineBinaryZipSha256
   }
 }

 $limBin = Get-ChildItem -LiteralPath $limineUnpack -Directory | Where-Object { $_.Name -eq "limine-binary" }
 if (-not $limBin) {
   $limBin = Get-ChildItem -LiteralPath $limineUnpack -Recurse -Directory -Filter "limine-binary" | Select-Object -First 1
 }
 if (-not $limBin) {
   throw "[E_ISO_DIST_LAYOUT] Expected limine-binary directory under extracted zip."
 }

 $biosCd = Join-Path $limBin.FullName "limine-bios-cd.bin"
 $uefiCd = Join-Path $limBin.FullName "limine-uefi-cd.bin"
 $biosSys = Join-Path $limBin.FullName "limine-bios.sys"
 $bootx64 = Join-Path $limBin.FullName "BOOTX64.EFI"
 $limExe = Resolve-LimineHostExe -LimineUnpackRoot $limBin.FullName

 foreach ($must in @($biosCd, $uefiCd, $biosSys, $bootx64, $limExe)) {
   if (-not (Test-Path -LiteralPath $must)) {
     throw "[E_ISO_DIST_FILE] Missing required Limine artifact: $must"
   }
 }

 Write-Output "==> nightly build limine-bridge (--release)"

& cargo "+nightly" `
   "-Zbuild-std=core,compiler_builtins,alloc" `
   "-Zbuild-std-features=compiler-builtins-mem" `
   "-Zjson-target-spec" `
   build -p limine-bridge `
   --target "$root/limine-bridge/x86_64-zpl-limine.json" `
   --release

 if ($LASTEXITCODE -ne 0) {
   throw "[E_ISO_BUILD_BRIDGE] cargo build limine-bridge failed exit $LASTEXITCODE"
 }

 $bridgeElf = Join-Path $root "target/x86_64-zpl-limine/release/limine-bridge"
 if (-not (Test-Path $bridgeElf)) {
   throw "[E_ISO_BRIDGE_ELF_MISSING] Expected $bridgeElf"
 }

 Write-Output "==> staging ISO tree at $staging"
 if (Test-Path $staging) {
   Remove-Item -LiteralPath $staging -Recurse -Force
 }
 New-Item -ItemType Directory -Force -Path (Join-Path $staging "boot/limine") | Out-Null
 New-Item -ItemType Directory -Force -Path (Join-Path $staging "EFI/BOOT") | Out-Null

Copy-Item -LiteralPath $bridgeElf -Destination (Join-Path $staging "boot/zpl-kernel.elf") -Force
Copy-Item -LiteralPath $biosSys -Destination (Join-Path $staging "boot/limine/limine-bios.sys") -Force
# Hybrid CD images live under boot/limine/ with limine-bios.sys (Limine USAGE layout).
Copy-Item -LiteralPath $biosCd -Destination (Join-Path $staging "boot/limine/limine-bios-cd.bin") -Force
Copy-Item -LiteralPath $uefiCd -Destination (Join-Path $staging "boot/limine/limine-uefi-cd.bin") -Force
Copy-Item -LiteralPath $bootx64 -Destination (Join-Path $staging "EFI/BOOT/BOOTX64.EFI") -Force

# Config: Limine reads boot/limine/limine.conf. Top-level keys must start at column 0 (no leading
# spaces — PowerShell here-strings preserve script indentation and Limine rejects indented keys).
# Sub-keys under the menu entry use 4 spaces (Limine YAML-style config).
$limineConfPath = Join-Path $staging "boot/limine/limine.conf"
$menuEntryName = "/ZPL Kernel ($LimineTag)"
$limineConfLines = @(
  "timeout: 0"
  "default_entry: 1"
  "verbose: yes"
  "serial: yes"
  ""
  $menuEntryName
  "    protocol: limine"
  "    path: boot():/boot/zpl-kernel.elf"
)
$limineConfText = ($limineConfLines -join [Environment]::NewLine) + [Environment]::NewLine
Set-Content -LiteralPath $limineConfPath -Value $limineConfText -Encoding ascii

 $outIso = Join-Path $artifactsIso "zpl-kernel.iso"
 if (Test-Path $outIso) {
   Remove-Item -LiteralPath $outIso -Force
 }

Invoke-LimineHybridXorriso -stagingWin $staging -isoOutWin $outIso `
  -biosCdRelative "boot/limine/limine-bios-cd.bin" `
  -efiCdRelative "boot/limine/limine-uefi-cd.bin"

 Invoke-LimineBiosInstall -LimineExe $limExe -IsoAbs $outIso

 $isoLen = (Get-Item -LiteralPath $outIso).Length
 if ($isoLen -gt $MaxIsoBytes) {
   throw "[E_ISO_007_TOO_LARGE] ISO size=$isoLen max=$MaxIsoBytes"
 }

 Write-Output "==> iso size=$((Get-Item $outIso).Length) bytes (<=$MaxIsoBytes ok)"

 # Name the image after what is in it, by reading the bytes.
 #
 # This script builds from the repository it sits in, so which build comes out
 # depends on where it was run rather than on any flag, and the two look the same
 # on screen. The check below is present only in the tree that can produce a
 # non-public build; see the README for what that build is and why it is not here.
 $gate = Join-Path $PSScriptRoot "iso-public-gate.ps1"
 $carriesEngine = $false
 if (Test-Path -LiteralPath $gate) {
   & powershell -NoProfile -ExecutionPolicy Bypass -File $gate -Path $outIso
   if ($LASTEXITCODE -ne 0) { $carriesEngine = $true }
 } else {
   # Expected in a copy that can only produce the public build: there is nothing
   # for the check to find, and the check itself stays with the tree that needs it.
   Write-Output "==> iso-public-gate.ps1 is not present. That is normal here: this copy"
   Write-Output "    builds one kind of image and there is nothing to tell apart. In a tree"
   Write-Output "    that builds more than one, it means the check is missing."
 }

 if ($carriesEngine) {
   $privName = "zpl-kernel-NU-PUBLICA.iso"
   $privPath = Join-Path $artifactsIso $privName
   if (Test-Path -LiteralPath $privPath) { Remove-Item -LiteralPath $privPath -Force }
   Move-Item -LiteralPath $outIso -Destination $privPath
   $outIso = $privPath
   $isoBaseName = $privName
   Write-Output "==> THIS IMAGE IS NOT THE PUBLIC BUILD. Renamed to $privName so it"
   Write-Output "    cannot be mistaken for one. Do not publish it, attach it to a release"
   Write-Output "    or film it. For a public image, run this script from an exported copy."
 } else {
   $isoBaseName = "zpl-kernel.iso"
 }

 $outShaPath = "$outIso.sha256"
 $hashHex = Get-Sha256File $outIso
 "$hashHex $isoBaseName" | Set-Content -Encoding ascii $outShaPath

 Write-Output "==> SHA256 $($outShaPath):" (Get-Content $outShaPath -Raw)

 if (-not $SkipQemu) {
   $qemu = Get-Command "qemu-system-x86_64" -ErrorAction SilentlyContinue
   if (-not $qemu) {
     Write-Output "==> qemu-system-x86_64 missing; skipping iso smoke (**not** PASS verified)."
   } else {
     $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
     $kernDir = Join-Path $artifactsIso "kernel"
     if (-not (Test-Path $kernDir)) { New-Item -ItemType Directory -Force -Path $kernDir | Out-Null }
     $serialIso = Join-Path $kernDir "iso-boot-$stamp.log"
     Write-Output "==> qemu cdrom bios smoke -> serial $serialIso"
     $qemuArgsFinal = @(
       "-cdrom", "$outIso",
       "-boot", "d",
       "-netdev", "user,id=n0",
       "-device", "virtio-net-pci,netdev=n0",
       "-serial", "file:$serialIso",
       "-display", "none",
       "-no-reboot",
       "-no-shutdown",
       "-device", "isa-debug-exit,iobase=0xf4,iosize=0x04",
       "-m", "256"
     )
     # Wait for the three markers, not for QEMU to exit.
     #
     # This kernel ends its boot at a prompt and waits for a person, so it never exits on
     # its own and `WaitForExit` always ran out the clock -- the step threw on a boot that
     # had in fact reached the halt loop forty seconds earlier. It only looked fine while
     # QEMU happened not to be on PATH and the whole smoke was skipped.
     $proc = Start-Process -FilePath $qemu.Source -ArgumentList $qemuArgsFinal -NoNewWindow -PassThru
     $deadline = (Get-Date).AddSeconds(45)
     $sawHalt = $false
     while ((Get-Date) -lt $deadline) {
       Start-Sleep -Milliseconds 500
       if (Test-Path -LiteralPath $serialIso) {
         $soFar = Get-Content -LiteralPath $serialIso -Raw -ErrorAction SilentlyContinue
         if ($soFar -and $soFar -match '\[ZPL-BOOT\] halt loop entered') { $sawHalt = $true; break }
       }
       if ($proc.HasExited) { break }
     }
     if (-not $proc.HasExited) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
     if (-not $sawHalt) {
       throw "[E_ISO_QEMU_TIMEOUT] ISO smoke saw no halt-loop marker within 45s."
     }
     $logTxt = Get-Content -LiteralPath $serialIso -Raw
     if (-not ($logTxt -match '\[ZPL-BOOT\] kernel_entry reached')) { throw "[E_ISO_QEMU_MARKER] missing kernel_entry" }
     if (-not ($logTxt -match '\[ZPL-BOOT\] serial initialized')) { throw "[E_ISO_QEMU_MARKER] missing serial" }
     if (-not ($logTxt -match '\[ZPL-BOOT\] halt loop entered')) { throw "[E_ISO_QEMU_MARKER] missing halt" }
     Write-Output "==> ISO QEMU BIOS smoke PASS verified ($serialIso)."
   }
 } else {
   Write-Output "==> SkipQemu: ISO QEMU smoke skipped by flag."
 }

 Write-Output "==> build-iso PASS verified -> $outIso"
