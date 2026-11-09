# Tauri's Windows signing command in CI (RELEASING.md). It never signs anything: the key is
# never in CI. Tauri runs it for every file it would sign (tauri-bundler 2.10.4: the main exe,
# the NSIS plugin DLLs, the uninstaller from makensis's !uninstfinalize, the finished
# installers, WiX's own DLLs). Only the NSIS uninstaller matters here; makensis hands it over
# as a temporary .tmp file. Everything else is left exactly as it is.
#
#   SIGN_COMMAND_MODE=capture  (the tag's build) copy the uninstaller to
#                              $SIGN_COMMAND_DIR\uninstall.exe, to be signed with the exes
#   SIGN_COMMAND_MODE=swap     (windows-installers.yml) replace the uninstaller with the signed
#                              copy in $SIGN_COMMAND_DIR\uninstall.exe, but only if that copy is
#                              this uninstaller plus a signature (scripts/compare-signed.py);
#                              writes swapped.txt there as proof
#
# Windows PowerShell 5.1. Any failure exits non-zero.
param([Parameter(Mandatory = $true, Position = 0)][string]$Target)
$ErrorActionPreference = 'Stop'
$mode = $env:SIGN_COMMAND_MODE
$dir = $env:SIGN_COMMAND_DIR
if ($mode -notin 'capture', 'swap') { throw "SIGN_COMMAND_MODE must be capture or swap, not '$mode'" }
if (-not $dir -or -not (Test-Path -LiteralPath $dir -PathType Container)) { throw "SIGN_COMMAND_DIR '$dir' is not a folder" }
$item = Get-Item -LiteralPath $Target
if ($item.Extension -ne '.tmp') { "sign-command ($mode): left unchanged: $($item.Name)"; exit 0 }

$copy = Join-Path $dir 'uninstall.exe'
$hash = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
if ($mode -eq 'capture') {
    if (Test-Path -LiteralPath $copy) {
        $old = (Get-FileHash -LiteralPath $copy -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($old -ne $hash) { throw "a different uninstaller was captured already ($old, now $hash)" }
        "sign-command (capture): the same uninstaller again ($hash)"
        exit 0
    }
    Copy-Item -LiteralPath $item.FullName -Destination $copy
    "sign-command (capture): uninstaller $($item.Name), sha256 $hash, copied to $copy"
    exit 0
}

if (-not (Test-Path -LiteralPath $copy)) { throw "no signed uninstaller at $copy" }
& python (Join-Path $PSScriptRoot 'compare-signed.py') pe $item.FullName $copy
if ($LASTEXITCODE -ne 0) { throw "the signed uninstaller is not this uninstaller ($hash) plus a signature" }
Copy-Item -LiteralPath $copy -Destination $item.FullName -Force
$now = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
Set-Content -LiteralPath (Join-Path $dir 'swapped.txt') -Value "$hash -> $now" -Encoding Ascii
"sign-command (swap): uninstaller $hash replaced by the signed copy $now"
exit 0
