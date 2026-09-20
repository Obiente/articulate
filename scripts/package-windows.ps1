param([switch]$SkipBuild, [string]$Python = 'python', [string]$Makensis = '')
$ErrorActionPreference = 'Stop'
$articulateRoot = Split-Path $PSScriptRoot -Parent
Set-Location $articulateRoot
$version = [regex]::Match((Get-Content Cargo.toml -Raw), '(?m)^version = "(\d+\.\d+\.\d+)"').Groups[1].Value
if (!$version) { throw 'A stable three-part package version is required.' }
$dist = Join-Path $articulateRoot 'dist'
$zip = Join-Path $dist "Articulate-$version-windows-x86_64.zip"
$setup = Join-Path $dist "Articulate-$version-windows-x86_64-setup.exe"
if ((Test-Path -LiteralPath $zip) -or (Test-Path -LiteralPath $setup)) { throw 'Output exists. Move the previous release artifacts before repackaging.' }
$build = Join-Path $articulateRoot '.local/package-target'
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss-fff'
$stage = Join-Path $articulateRoot ".local/package-stage/$stamp/Articulate-$version-windows-x86_64"
New-Item -ItemType Directory -Force $dist,$stage | Out-Null

# Acquire build tooling before the lengthy Rust build. SourceForge can return
# an HTML mirror page to PowerShell's web client; curl follows its download flow.
if (!$Makensis) {
    $tools = Join-Path $articulateRoot '.local/tools'
    $nsisArchive = Join-Path $tools 'nsis-3.11.zip'
    $nsisHash = 'c7d27f780ddb6cffb4730138cd1591e841f4b7edb155856901cdf5f214394fa1'
    New-Item -ItemType Directory -Force $tools | Out-Null
    $validArchive = (Test-Path -LiteralPath $nsisArchive) -and ((Get-FileHash -LiteralPath $nsisArchive -Algorithm SHA256).Hash.ToLowerInvariant() -eq $nsisHash)
    if (!$validArchive) {
        $nsisPart = Join-Path $tools ('nsis-' + [guid]::NewGuid().ToString('N') + '.part')
        try {
            & curl.exe --fail --location --silent --show-error --retry 3 --max-time 180 --output $nsisPart 'https://downloads.sourceforge.net/project/nsis/NSIS%203/3.11/nsis-3.11.zip'
            if ($LASTEXITCODE -ne 0) { throw 'NSIS archive download failed.' }
            if ((Get-FileHash -LiteralPath $nsisPart -Algorithm SHA256).Hash.ToLowerInvariant() -ne $nsisHash) { throw 'NSIS archive SHA-256 mismatch.' }
            Move-Item -LiteralPath $nsisPart -Destination $nsisArchive -Force
        } finally {
            if (Test-Path -LiteralPath $nsisPart) { Remove-Item -LiteralPath $nsisPart }
        }
    }
    Expand-Archive -LiteralPath $nsisArchive -DestinationPath $tools -Force
    $Makensis = Join-Path $tools 'nsis-3.11/makensis.exe'
}
if (!(Test-Path -LiteralPath $Makensis)) { throw 'Install NSIS 3.11 or pass -Makensis.' }

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (!$vs) { throw 'Visual Studio C++ Build Tools are required to package.' }
Import-Module (Join-Path $vs 'Common7/Tools/Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64'
$oldFlags = $env:CARGO_ENCODED_RUSTFLAGS
$oldTarget = $env:CARGO_TARGET_DIR
$oldResource = $env:ARTICULATE_RESOURCE
try {
    $env:CARGO_TARGET_DIR = $build
    $env:CARGO_ENCODED_RUSTFLAGS = (@("--remap-path-prefix=$($env:USERPROFILE)=/build/user", "--remap-path-prefix=$articulateRoot=/src/articulate", '-C', 'strip=symbols') -join [char]31)
    $rc = Join-Path $articulateRoot '.local/articulate.rc'
    $resource = Join-Path $articulateRoot '.local/articulate.res'
    $resourceText = (Get-Content packaging/articulate.rc -Raw).Replace('0,1,0,0', ($version.Replace('.', ',') + ',0')).Replace('0.1.0', $version)
    # RC resolves asset references relative to the generated .rc file, not Cargo.
    $resourceText = $resourceText.Replace('assets\\brand\\articulate.ico', (Join-Path $articulateRoot 'assets/brand/articulate.ico').Replace('\', '\\'))
    [IO.File]::WriteAllText($rc, $resourceText)
    & rc.exe /nologo "/fo$resource" $rc
    if ($LASTEXITCODE -ne 0) { throw 'Resource compilation failed.' }
    $env:ARTICULATE_RESOURCE = $resource
    if (!$SkipBuild) {
        & (Join-Path $PSScriptRoot 'build-windows.ps1')
        if ($LASTEXITCODE -ne 0) { throw 'Release build failed.' }
    }
    $exe = Join-Path $build 'release/transcribe-local.exe'
    if (!(Test-Path -LiteralPath $exe)) { throw 'Build the packaged executable first.' }
    Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'Articulate.exe')
    $native = Join-Path $articulateRoot '.local/native/transcribe-native-windows-x86_64-cpu-vulkan'
    $dlls = @('transcribe','ggml','ggml-base','ggml-vulkan','ggml-cpu-x64','ggml-cpu-sse42','ggml-cpu-sandybridge','ggml-cpu-haswell','ggml-cpu-cannonlake','ggml-cpu-cascadelake','ggml-cpu-alderlake','ggml-cpu-skylakex','ggml-cpu-icelake')
    foreach ($dll in $dlls) { Copy-Item -LiteralPath (Join-Path $native "$dll.dll") -Destination $stage }
    $redist = Get-ChildItem -LiteralPath (Join-Path $vs 'VC/Redist/MSVC') -Directory | Where-Object { $_.Name -match '^\d+(\.\d+)+$' } | Sort-Object { [version]$_.Name } -Descending | Select-Object -First 1
    $crt = Join-Path $redist.FullName 'x64/Microsoft.VC143.CRT'
    if (!(Test-Path -LiteralPath $crt)) { throw 'Microsoft x64 redistributable runtime not found.' }
    Get-ChildItem -LiteralPath $crt -Filter '*.dll' -File | Copy-Item -Destination $stage
    $licenses = Join-Path $stage 'licenses'
    New-Item -ItemType Directory -Force $licenses | Out-Null
    Copy-Item -LiteralPath (Join-Path $native 'licenses') -Destination (Join-Path $licenses 'native') -Recurse
    Copy-Item -LiteralPath LICENSE -Destination (Join-Path $licenses 'Articulate-AGPL-3.0-or-later.txt')
    Copy-Item -LiteralPath NOTICE -Destination (Join-Path $licenses 'Articulate-NOTICE.txt')
    Copy-Item -LiteralPath assets/fonts/OFL.txt -Destination (Join-Path $licenses 'Inter-OFL.txt')
    Copy-Item -LiteralPath assets/icons/LICENSE.txt -Destination (Join-Path $licenses 'Phosphor-MIT.txt')
    Copy-Item -LiteralPath packaging/START-HERE.txt -Destination $stage
    @('Microsoft Visual C++ Runtime', 'App-local redistributable files from the Visual Studio x64 CRT distribution.', 'Copyright Microsoft Corporation. All rights reserved.', 'https://visualstudio.microsoft.com/license-terms/') | Set-Content -LiteralPath (Join-Path $licenses 'Microsoft-runtime.txt')
    # Preserve the installed distribution's actual notices. Articulate's license
    # license does not relicense Microsoft's runtime files.
    Copy-Item -LiteralPath (Join-Path $vs 'Licenses/1033/Redist.txt') -Destination (Join-Path $licenses 'Microsoft-Redist.txt')
    Copy-Item -LiteralPath (Join-Path $vs 'Licenses/1033/ThirdPartyNotices.txt') -Destination (Join-Path $licenses 'Microsoft-ThirdPartyNotices.txt')
    $metadata = Join-Path $articulateRoot '.local/package-metadata.json'
    & cargo metadata --locked --format-version 1 --filter-platform x86_64-pc-windows-msvc --features dynamic-backends | Set-Content -LiteralPath $metadata -Encoding utf8
    if ($LASTEXITCODE -ne 0) { throw 'Dependency metadata failed.' }
    & $Python (Join-Path $PSScriptRoot 'collect-license-notices.py') $metadata (Join-Path $licenses 'rust')
    if ($LASTEXITCODE -ne 0) { throw 'License collection failed.' }
    # Reject developer data and machine paths before an artifact can leave staging.
    foreach ($file in Get-ChildItem -LiteralPath $stage -Recurse -File) {
        if ($file.Extension -in @('.gguf','.wav','.pdb','.onnx','.part') -or $file.Name -eq 'settings.json') { throw 'Private or model data found in package.' }
        $bytes = [IO.File]::ReadAllBytes($file.FullName)
        foreach ($encoding in @([Text.Encoding]::UTF8, [Text.Encoding]::Unicode)) {
            $text = $encoding.GetString($bytes)
            foreach ($privatePath in @($env:USERPROFILE, $articulateRoot)) {
                foreach ($variant in @($privatePath, $privatePath.Replace('\','/'), $privatePath.Replace('\','\\'))) {
                    if ($variant -and $text.IndexOf($variant, [StringComparison]::OrdinalIgnoreCase) -ge 0) { throw "Machine path found in $($file.Name)." }
                }
            }
        }
    }
    $manifest = @(Get-ChildItem -LiteralPath $stage -Recurse -File | ForEach-Object { [ordered]@{ path=$_.FullName.Substring($stage.Length+1).Replace('\','/'); sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant(); bytes=$_.Length } })
    [ordered]@{ product='Articulate'; version=$version; platform='windows-x86_64'; files=$manifest } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $stage 'manifest.json') -Encoding utf8
    $remove = Join-Path $articulateRoot ".local/package-stage/$stamp/remove-files.nsh"
    $lines = @(Get-ChildItem -LiteralPath $stage -Recurse -File | ForEach-Object { 'Delete "$INSTDIR\' + $_.FullName.Substring($stage.Length+1).Replace('$','$$') + '"' })
    $lines += @(Get-ChildItem -LiteralPath $stage -Recurse -Directory | Sort-Object { $_.FullName.Length } -Descending | ForEach-Object { 'RMDir "$INSTDIR\' + $_.FullName.Substring($stage.Length+1).Replace('$','$$') + '"' })
    $lines | Set-Content -LiteralPath $remove -Encoding utf8
    $stagedSetup = Join-Path $articulateRoot ".local/package-stage/$stamp/Articulate-$version-windows-x86_64-setup.exe"
    & $Makensis /V2 "/DVERSION=$version" "/DSTAGE=$stage" "/DOUTPUT=$stagedSetup" "/DREMOVE_FILES=$remove" (Join-Path $articulateRoot 'packaging/windows.nsi')
    if ($LASTEXITCODE -ne 0) { throw 'Installer compilation failed.' }
    $stagedZip = Join-Path $articulateRoot ".local/package-stage/$stamp/portable.zip"
    Get-ChildItem -LiteralPath $stage -Recurse -File | Where-Object { $_.LastWriteTime.Year -lt 1980 } | ForEach-Object { $_.LastWriteTime = [datetime]'2026-01-01' }
    Compress-Archive -LiteralPath $stage -DestinationPath $stagedZip -CompressionLevel Optimal
    Move-Item -LiteralPath $stagedSetup -Destination $setup
    Move-Item -LiteralPath $stagedZip -Destination $zip
    @($setup,$zip) | ForEach-Object { (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant() + '  ' + [IO.Path]::GetFileName($_) } | Set-Content -LiteralPath (Join-Path $dist 'SHA256SUMS.txt') -Encoding ascii
    Write-Host "Release artifacts ready in $dist"
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $oldFlags
    $env:CARGO_TARGET_DIR = $oldTarget
    $env:ARTICULATE_RESOURCE = $oldResource
}
