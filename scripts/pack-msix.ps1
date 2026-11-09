# Pack the MSIX package around an already signed Launcher.exe, unsigned (it is signed in the second
# signing session, RELEASING.md). Run by windows-installers.yml, under pwsh, on the runner of the
# package's architecture.
#
# Usage: pack-msix.ps1 -Arch <x64|arm64> -Exe <signed Launcher.exe> -Version <X.Y.Z> -Out <folder>
#        [-Publisher <subject>]   dry runs with a test certificate only
#
# makeappx comes from Microsoft.Windows.SDK.BuildTools 10.0.26100.9169 (NuGet), checked against
# NuGet's SHA-512, not from whatever SDK the runner image carries. The package holds the exe
# unchanged, the app's resources and Tauri's tile icons; the script checks the exe inside the
# finished package is byte-identical to -Exe. The manifest is src-tauri/msix/AppxManifest.xml;
# Microsoft's manifest schema requires its Publisher to match the signing certificate's subject,
# hence -Publisher for a test certificate.
param(
    [Parameter(Mandatory = $true)][ValidateSet('x64', 'arm64')][string]$Arch,
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$Out,
    [string]$Publisher = ''
)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$root = Split-Path -Parent $PSScriptRoot
$tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }

$nupkg = Join-Path $tmp 'buildtools.nupkg'
Invoke-WebRequest 'https://api.nuget.org/v3-flatcontainer/microsoft.windows.sdk.buildtools/10.0.26100.9169/microsoft.windows.sdk.buildtools.10.0.26100.9169.nupkg' -OutFile $nupkg
$sha512 = [Convert]::ToBase64String([Security.Cryptography.SHA512]::Create().ComputeHash([IO.File]::ReadAllBytes($nupkg)))
if ($sha512 -ne 'hOpNh7bCzJOOm2hvKuodr1NUKHeqoUhxdAzTmhO/q+u/E+5FOyhK+THhNhUUfKLbWnSslZ/m35dwghmS2BzaWg==') { throw "build tools package SHA-512 $sha512 is not NuGet's" }
$bt = Join-Path $tmp 'buildtools'
Expand-Archive $nupkg $bt -Force
$mk = Join-Path $bt "bin\10.0.26100.0\$Arch\makeappx.exe"
$mksig = Get-AuthenticodeSignature $mk
"makeappx: $($mksig.Status), $($mksig.SignerCertificate.Subject)"
if ($mksig.Status -ne 'Valid') { throw 'makeappx is not validly signed' }

$dir = Join-Path $tmp "msix-$Arch"
if (Test-Path $dir) { throw "$dir exists" }
New-Item -ItemType Directory $dir, "$dir\Assets" | Out-Null
Copy-Item -LiteralPath $Exe "$dir\Launcher.exe"
Copy-Item -Recurse "$root\src-tauri\resources" "$dir\resources"
foreach ($n in 'StoreLogo.png', 'Square44x44Logo.png', 'Square150x150Logo.png') { Copy-Item "$root\src-tauri\icons\$n" "$dir\Assets\$n" }
$xml = [IO.File]::ReadAllText("$root\src-tauri\msix\AppxManifest.xml")
foreach ($t in '@VERSION@', '@ARCH@') { if (([regex]::Matches($xml, $t)).Count -ne 1) { throw "expected $t once in the manifest" } }
$xml = $xml.Replace('@VERSION@', $Version).Replace('@ARCH@', $Arch)
if ($Publisher) {
    $m = [regex]::Matches($xml, 'Publisher="[^"]*"')
    if ($m.Count -ne 1) { throw 'expected one Publisher attribute in the manifest' }
    $xml = $xml.Replace($m[0].Value, 'Publisher="' + [Security.SecurityElement]::Escape($Publisher) + '"')
    "dry run: Publisher set to $Publisher"
}
[IO.File]::WriteAllText("$dir\AppxManifest.xml", $xml, (New-Object Text.UTF8Encoding $false))

New-Item -ItemType Directory -Force $Out | Out-Null
$file = Join-Path $Out "Launcher_$($Version)_$Arch.msix"
& $mk pack /d $dir /p $file /o | Select-Object -Last 1
if ($LASTEXITCODE -ne 0) { throw "makeappx failed: $LASTEXITCODE" }

Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [IO.Compression.ZipFile]::OpenRead($file)
try {
    $entry = $zip.GetEntry('Launcher.exe')
    $s = $entry.Open(); $ms = New-Object IO.MemoryStream; $s.CopyTo($ms); $s.Close()
    $inside = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash($ms.ToArray())).Replace('-', '').ToLowerInvariant()
} finally { $zip.Dispose() }
$want = (Get-FileHash -LiteralPath $Exe -Algorithm SHA256).Hash.ToLowerInvariant()
if ($inside -ne $want) { throw "Launcher.exe inside the package ($inside) is not $Exe ($want)" }
"The package contains exactly the signed Launcher.exe ($want)."
