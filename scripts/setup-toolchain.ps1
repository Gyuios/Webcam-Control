[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
  throw 'Abre PowerShell como administrador y vuelve a ejecutar este script. Visual Studio Build Tools requiere elevación UAC.'
}

winget install --id Microsoft.VisualStudio.2022.BuildTools --exact `
  --override '--wait --passive --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended' `
  --accept-package-agreements --accept-source-agreements
if ($LASTEXITCODE -ne 0) { throw "Visual Studio Build Tools devolvió el código $LASTEXITCODE." }

winget install --id Microsoft.DotNet.SDK.10 --exact --accept-package-agreements --accept-source-agreements
if ($LASTEXITCODE -ne 0) { throw ".NET SDK devolvió el código $LASTEXITCODE." }

winget install --id OpenJS.NodeJS.LTS --exact --accept-package-agreements --accept-source-agreements
if ($LASTEXITCODE -ne 0) { throw "Node.js LTS devolvió el código $LASTEXITCODE." }

if ($null -eq (Get-Command rustup -ErrorAction SilentlyContinue)) {
  winget install --id Rustlang.Rustup --exact --accept-package-agreements --accept-source-agreements
  if ($LASTEXITCODE -ne 0) { throw "Rustup devolvió el código $LASTEXITCODE." }
}

Write-Host 'Toolchain instalada. Cierra y vuelve a abrir la terminal antes de compilar.'
