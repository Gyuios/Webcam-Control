[CmdletBinding()]
param()

$checks = [Collections.Generic.List[object]]::new()
function Add-Check([string]$Name, [bool]$Ok, [string]$Detail) {
  $checks.Add([pscustomobject]@{ Requisito = $Name; Estado = $(if ($Ok) { 'OK' } else { 'FALTA' }); Detalle = $Detail })
}

$windowsBuild = [Environment]::OSVersion.Version.Build
Add-Check 'Windows 11' ($windowsBuild -ge 22000) "Build $windowsBuild (mínimo 22000)"

foreach ($command in @('git', 'node', 'npm', 'cargo', 'rustc', 'dotnet')) {
  $found = Get-Command $command -ErrorAction SilentlyContinue
  Add-Check $command ($null -ne $found) $(if ($null -ne $found) { $found.Source } else { 'No está en PATH' })
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$cpp = $false
$detail = 'Visual Studio Installer no está presente'
if (Test-Path -LiteralPath $vswhere) {
  $installation = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  $cpp = -not [string]::IsNullOrWhiteSpace($installation)
  $detail = $(if ($cpp) { $installation } else { 'Falta la carga de trabajo C++ x64' })
}
Add-Check 'MSVC C++ x64' $cpp $detail

$checks | Format-Table -AutoSize
if ($checks.Where({ $_.Estado -ne 'OK' }).Count -gt 0) {
  Write-Error 'El entorno todavía no está listo. Para las herramientas del sistema, ejecuta scripts\setup-toolchain.ps1 como administrador.'
  exit 1
}
Write-Host 'El entorno está listo para compilar CameraTuner.'
