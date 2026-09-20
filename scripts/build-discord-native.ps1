param([string]$OutputDirectory = '', [switch]$SkipTests)
$ErrorActionPreference = 'Stop'
$repository = Split-Path $PSScriptRoot -Parent
if (!$OutputDirectory) { $OutputDirectory = Join-Path $repository '.local/discord-native-install' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
$buildDirectory = Join-Path $repository '.local/discord-native-build'
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$installation = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (!$installation) { throw 'Building the native audio adapter requires Visual Studio C++ Build Tools.' }
Import-Module (Join-Path $installation 'Common7/Tools/Microsoft.VisualStudio.DevShell.dll')
Enter-VsDevShell -VsInstallPath $installation -SkipAutomaticLocation -DevCmdArguments '-arch=x64 -host_arch=x64'
& cmake -S (Join-Path $repository 'plugins/discord-native') -B $buildDirectory -G Ninja -DCMAKE_BUILD_TYPE=Release
if ($LASTEXITCODE -ne 0) { throw 'Native audio adapter configuration failed.' }
& cmake --build $buildDirectory
if ($LASTEXITCODE -ne 0) { throw 'Native audio adapter build failed.' }
if (!$SkipTests) {
    & ctest --test-dir $buildDirectory --output-on-failure
    if ($LASTEXITCODE -ne 0) { throw 'Native audio adapter contract tests failed.' }
}
& cmake --install $buildDirectory --prefix $OutputDirectory
if ($LASTEXITCODE -ne 0) { throw 'Native audio adapter staging failed.' }
foreach ($relative in @('articulate_discord_audio.node', 'articulate-audio-preload.cjs', 'licenses/MinHook.txt', 'licenses/Node-API-Headers.txt', 'licenses/Articulate-AGPL.txt')) {
    if (!(Test-Path -LiteralPath (Join-Path $OutputDirectory $relative) -PathType Leaf)) { throw "Native payload is missing $relative" }
}
# Child cargo builds inherit the verified, locally built payload. End users do not build C++.
$env:ARTICULATE_NATIVE_AUDIO_DIR = $OutputDirectory
