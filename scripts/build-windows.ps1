<#
Builds the RAD desktop app for Windows 10/11 (x64 or ARM64) with its engines inside the installer:
  - the web browser engine (Servo)        -> scripts/build-browser.sh   (skip: -NoBrowser)
  - the text-to-speech engine (MioTTS)    -> scripts/build-tts.sh
  - the Linux sandbox engine is macOS-only for now and is left out.
Result: target\release\bundle\nsis\RAD_<version>_x64-setup.exe (and \msi\*.msi with -Bundles nsis,msi)

  powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -InstallDeps   first time (winget)
  powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1                 later builds
  ... -NoBrowser            quicker: RAD without the built-in browser
  ... -Bundles nsis,msi     also the .msi (default: nsis)

The engine scripts are bash; they run in Git for Windows' bash (not WSL's), which also
provides curl and sha256sum. Keep the checkout in a short path (e.g. C:\src\RAD): the browser
engine build has very deep paths.
#>
param(
    [switch]$InstallDeps,
    [switch]$NoBrowser,
    [string]$Bundles = "nsis",
    [Parameter(ValueFromRemainingArguments = $true)][string[]]$TauriArgs = @()
)
$ErrorActionPreference = "Stop"
Set-Location (Split-Path -Parent $PSScriptRoot)
$root = (Get-Location).Path

function Need($what, $hint) { $script:missing += "  - $what — $hint" }
function Refresh-Path {
    $env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";" + [Environment]::GetEnvironmentVariable("Path", "User")
}

# ------------------------------------------------------------------ tools
if ($InstallDeps) {
    $ids = @(
        @("Git.Git", ""),
        @("Kitware.CMake", ""),
        @("OpenJS.NodeJS.LTS", ""),
        @("LLVM.LLVM", ""),                       # clang for the browser engine (SpiderMonkey)
        @("Python.Python.3.12", ""),
        @("Rustlang.Rustup", ""),
        # C++ compiler and Windows SDK
        @("Microsoft.VisualStudio.2022.BuildTools", "--wait --quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended")
    )
    foreach ($p in $ids) {
        Write-Host "Installing $($p[0])…"
        if ($p[1]) { winget install --id $p[0] -e --accept-source-agreements --accept-package-agreements --override $p[1] }
        else { winget install --id $p[0] -e --accept-source-agreements --accept-package-agreements }
    }
    Refresh-Path
    if (Get-Command rustup -ErrorAction SilentlyContinue) { rustup default stable-msvc }
}

$missing = @()
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { Need "Rust" "re-run with -InstallDeps, or install from https://rustup.rs" }
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) { Need "CMake" "re-run with -InstallDeps" }
if (-not (Get-Command npm -ErrorAction SilentlyContinue)) { Need "Node.js 18+" "re-run with -InstallDeps" }
$bash = @("$env:ProgramFiles\Git\bin\bash.exe", "${env:ProgramFiles(x86)}\Git\bin\bash.exe", "$env:LOCALAPPDATA\Programs\Git\bin\bash.exe") |
    Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $bash) { Need "Git for Windows (its bash runs the engine scripts)" "re-run with -InstallDeps" }
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not ((Test-Path $vswhere) -and (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath))) {
    Need "Visual Studio C++ build tools" "re-run with -InstallDeps"
}
if ($missing.Count) {
    Write-Host "Missing before RAD can be built:" -ForegroundColor Red
    $missing | ForEach-Object { Write-Host $_ }
    exit 1
}

$host_triple = (rustc -vV | Select-String '^host: ').ToString().Substring(6).Trim()
if ($host_triple -notlike "*windows-msvc") {
    Write-Host "Rust's default toolchain is $host_triple; RAD needs MSVC: rustup default stable-msvc" -ForegroundColor Red
    exit 1
}
if ($root.Length -gt 40 -and -not $NoBrowser) {
    Write-Warning "The checkout path ($root) is long; the browser engine build may hit Windows' path limit. Move it to e.g. C:\src\RAD, or use -NoBrowser."
}
git config core.longpaths true | Out-Null
cargo tauri --version 2>$null | Out-Null
if ($LASTEXITCODE -ne 0) { cargo install tauri-cli --version "^2" --locked; if ($LASTEXITCODE) { exit 1 } }

# The browser engine's SpiderMonkey build uses clang via libclang.
if (-not $env:LIBCLANG_PATH -and (Test-Path "$env:ProgramFiles\LLVM\bin\libclang.dll")) { $env:LIBCLANG_PATH = "$env:ProgramFiles\LLVM\bin" }
if (-not $env:MOZILLABUILD -and (Test-Path "C:\mozilla-build")) { $env:MOZILLABUILD = "C:\mozilla-build" }

# ------------------------------------------------------------------ models and engines
function Run-Bash($script) {
    & $bash -lc "cd '$($root -replace '\\','/')' && $script"
    if ($LASTEXITCODE -ne 0) { throw "failed: $script" }
}
Run-Bash "./scripts/fetch-models.sh"
if (-not $NoBrowser) { Run-Bash "./scripts/build-browser.sh $host_triple" }
Run-Bash "./scripts/build-tts.sh $host_triple"

# Bundle whichever engines are built (Tauri adds the target triple and .exe itself).
$bins = @()
foreach ($b in @("rad-browser", "mio-tts-server")) {
    if ($b -eq "rad-browser" -and $NoBrowser) { continue }
    if (Test-Path "app\src-tauri\binaries\$b-$host_triple.exe") { $bins += "binaries/$b" }
}
New-Item -ItemType Directory -Force target | Out-Null
$cfg = Join-Path $root "target\rad-bundle.conf.json"
# Written by hand: Windows PowerShell 5.1's ConvertTo-Json and Set-Content add surprises
# (single-item arrays, a BOM) that Tauri's config parser rejects.
$list = ($bins | ForEach-Object { '"' + $_ + '"' }) -join ","
[IO.File]::WriteAllText($cfg, "{""bundle"":{""externalBin"":[$list]}}")

# ------------------------------------------------------------------ app
Push-Location app\ui
if (-not (Test-Path node_modules)) { npm ci; if ($LASTEXITCODE) { exit 1 } }
Pop-Location
Push-Location app
cargo tauri build --config $cfg --bundles $Bundles @TauriArgs
$code = $LASTEXITCODE
Pop-Location
if ($code) { exit $code }

Write-Host ""
Write-Host "Built with: $(if ($bins) { $bins -join ', ' } else { 'no engines' })"
Get-ChildItem target\release\bundle\nsis\*.exe, target\release\bundle\msi\*.msi -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "  $($_.FullName)" }
