param([Parameter(Mandatory=$true)][string]$PrivateKeyPath)
$ErrorActionPreference = 'Stop'
$root = [IO.Path]::GetFullPath((Split-Path $PSScriptRoot -Parent))
$private = [IO.Path]::GetFullPath($PrivateKeyPath)
if ($private.StartsWith($root + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or $private -eq $root) { throw 'Private key must be outside the repository.' }
if (Test-Path -LiteralPath $private) { throw 'Refusing to overwrite an existing private key.' }
node (Join-Path $PSScriptRoot 'new-update-key.mjs') $private (Join-Path $root 'assets/update-signing.pub')
if ($LASTEXITCODE -ne 0) { throw 'Key generation failed.' }
