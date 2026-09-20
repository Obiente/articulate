param([switch]$Test, [switch]$Check)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root

# This pinned upstream bundle includes portable CPU dispatch and Vulkan.
# Rust still uses the matching published 0.2.3 bindings and checks the ABI.
$version = '0.2.3'
$asset = "transcribe-native-$version-windows-x86_64-cpu-vulkan.tar.gz"
$expected = 'dac5b6038aaf8777cab541b0f854a79e34f13e5892266229f64c61d34a879e49'
$cache = Join-Path $root '.local\native'
$prefix = Join-Path $cache 'install'
New-Item -ItemType Directory -Force $cache, "$prefix\bin", "$prefix\lib" | Out-Null
$archive = Join-Path $cache $asset
if (!(Test-Path -LiteralPath $archive)) {
    & gh release download "v$version" --repo handy-computer/transcribe.cpp --pattern $asset --dir $cache
    if ($LASTEXITCODE -ne 0) { throw 'Native runtime download failed' }
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
    throw 'Native runtime SHA-256 mismatch'
}
& tar -xf $archive -C $cache
if ($LASTEXITCODE -ne 0) { throw 'Native runtime extraction failed' }
$bundle = Join-Path $cache 'transcribe-native-windows-x86_64-cpu-vulkan'
$contract = Get-Content -Raw (Join-Path $bundle 'contract.json') | ConvertFrom-Json
if ($contract.version -ne $version -or $contract.header_hash -ne '7df72bf9e667b8c2') { throw 'Unexpected native ABI' }
Copy-Item -Path "$bundle\*.dll" -Destination "$prefix\bin" -Force

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (!$vs) { throw 'Install Visual Studio Build Tools with Desktop development with C++' }
Import-Module (Join-Path $vs 'Common7\Tools\Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $vs -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64'
& (Join-Path $PSScriptRoot 'build-discord-native.ps1')
if ($LASTEXITCODE -ne 0 -or !$env:ARTICULATE_NATIVE_AUDIO_DIR) { throw 'Native Discord adapter build failed' }

# Release archives contain DLLs, not the import .lib used by MSVC's linker.
# Generate the import library from their actual exports, without altering DLLs.
$exports = & dumpbin.exe /nologo /exports "$prefix\bin\transcribe.dll"
if ($LASTEXITCODE -ne 0) { throw 'Could not inspect native exports' }
$symbols = @($exports | ForEach-Object { if ($_ -match '^\s+\d+\s+[0-9A-F]+\s+[0-9A-F]+\s+(transcribe_\w+)\s*$') { $Matches[1] } })
if ($symbols.Count -lt 50) { throw 'Unexpected native export table' }
$def = Join-Path $prefix 'transcribe.def'
@('LIBRARY transcribe.dll', 'EXPORTS') + $symbols | Set-Content -LiteralPath $def -Encoding ascii
& lib.exe /nologo /machine:x64 "/def:$def" "/out:$prefix\lib\transcribe.lib"
if ($LASTEXITCODE -ne 0) { throw 'Import library generation failed' }
@{ shared = $true; lib_dir = 'lib'; libraries = @('transcribe'); module_dir = 'bin' } |
    ConvertTo-Json | Set-Content -LiteralPath "$prefix\lib\transcribe-link.json" -Encoding ascii
$env:TRANSCRIBE_DIR = $prefix
if ($Test) { & cargo test --release --locked --features dynamic-backends }
elseif ($Check) { & cargo clippy --release --locked --features dynamic-backends -- -D warnings }
else { & cargo build --release --locked --features dynamic-backends }
if ($LASTEXITCODE -ne 0) { throw 'Rust build or checks failed' }
if (!$Test -and !$Check) {
    $targetRoot = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR) } else { Join-Path $root 'target' }
    $binaryDirectory = Join-Path $targetRoot 'release'
    Copy-Item -Path "$bundle\*.dll" -Destination $binaryDirectory -Force
    Copy-Item -LiteralPath "$bundle\licenses" -Destination $binaryDirectory -Recurse -Force
    $articulateNotices = Join-Path $binaryDirectory 'licenses\articulate'
    New-Item -ItemType Directory -Force $articulateNotices | Out-Null
    Copy-Item -LiteralPath (Join-Path $root 'assets\LICENSES.md') -Destination $articulateNotices -Force
    Copy-Item -LiteralPath (Join-Path $root 'assets\fonts\OFL.txt') -Destination (Join-Path $articulateNotices 'Inter-OFL.txt') -Force
    Copy-Item -LiteralPath (Join-Path $root 'assets\icons\LICENSE.txt') -Destination (Join-Path $articulateNotices 'Phosphor-LICENSE.txt') -Force
    Write-Host "Ready: $(Join-Path $binaryDirectory 'articulate.exe')"
}
