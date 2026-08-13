[CmdletBinding()]
param(
  [switch]$Force,
  [switch]$SkipNative
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$projectRoot = Split-Path -Parent $PSScriptRoot
$binaryDirectory = Join-Path $projectRoot 'src-tauri\binaries'
$buildDirectory = Join-Path $projectRoot 'build\dependencies'
$downloadDirectory = Join-Path $buildDirectory 'downloads'
$nativeBuildDirectory = Join-Path ([IO.Path]::GetPathRoot($projectRoot)) "ctn-$PID"
$bridgeOutput = Join-Path $buildDirectory 'bridge'
$bridgeDestination = Join-Path $binaryDirectory 'control-webcam-bridge-x86_64-pc-windows-msvc.exe'
$cameraHostDestination = Join-Path $binaryDirectory 'camera-tuner-camera-host-x86_64-pc-windows-msvc.exe'
$mediaSourceDestination = Join-Path $binaryDirectory 'camera-tuner-media-source.dll'
$virtualCameraDestination = Join-Path $binaryDirectory 'camera-tuner-virtual-camera-x86_64-pc-windows-msvc.exe'

$windowsCamera = @{
  Commit = '790ac218eba8b6995393e9cc9537dfd7730fdb83'
  Url = 'https://github.com/microsoft/Windows-Camera/archive/790ac218eba8b6995393e9cc9537dfd7730fdb83.zip'
  Sha256 = '94510372b21ab64d7f1d766eb87d665431366f01ca09009923fb251a27b17ed3'
}

function Assert-Command([string]$Name, [string]$Guidance) {
  if ($null -eq (Get-Command $Name -ErrorAction SilentlyContinue)) {
    throw "No se encontró '$Name'. $Guidance"
  }
}

function Assert-NativeBuildPath([string]$Path) {
  $separator = [IO.Path]::DirectorySeparatorChar
  $resolvedBuild = [IO.Path]::GetFullPath($nativeBuildDirectory).TrimEnd($separator) + $separator
  $resolvedPath = [IO.Path]::GetFullPath($Path)
  if (-not $resolvedPath.StartsWith($resolvedBuild, [StringComparison]::OrdinalIgnoreCase)) {
    throw "La ruta nativa quedó fuera del directorio temporal permitido: $resolvedPath"
  }
}

function Get-Sha256([string]$Path) {
  $stream = [IO.File]::OpenRead($Path)
  try {
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
      return ([BitConverter]::ToString($sha256.ComputeHash($stream))).Replace('-', '').ToLowerInvariant()
    } finally {
      $sha256.Dispose()
    }
  } finally {
    $stream.Dispose()
  }
}

function Get-VerifiedArchive([hashtable]$Dependency, [string]$FileName) {
  New-Item -ItemType Directory -Force -Path $downloadDirectory | Out-Null
  $archive = Join-Path $downloadDirectory $FileName
  if ($Force -or -not (Test-Path -LiteralPath $archive)) {
    Invoke-WebRequest -Uri $Dependency.Url -OutFile $archive
  }
  $actual = Get-Sha256 $archive
  if ($actual -ne $Dependency.Sha256) {
    throw "La suma SHA-256 de $FileName no coincide. Esperada: $($Dependency.Sha256); recibida: $actual."
  }
  return $archive
}

function Copy-BuildArtifact([string]$Source, [string]$Destination) {
  $pending = "$Destination.pending"
  if (Test-Path -LiteralPath $pending) {
    try {
      Copy-Item -LiteralPath $pending -Destination $Destination -Force
      Remove-Item -LiteralPath $pending -Force
    } catch {
      # The destination is still in use. The normal copy below refreshes the
      # pending artifact when the new build is different.
    }
  }
  if (Test-Path -LiteralPath $Destination) {
    $sourceHash = Get-Sha256 $Source
    $destinationHash = Get-Sha256 $Destination
    if ($sourceHash -eq $destinationHash) { return }
  }
  try {
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
  } catch {
    Copy-Item -LiteralPath $Source -Destination $pending -Force
    Write-Warning "'$Destination' está en uso. Cierra CameraTuner y vuelve a ejecutar este script para aplicar la actualización pendiente."
    return
  }
}

function Find-MSBuild {
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  if (-not (Test-Path -LiteralPath $vswhere)) {
    throw 'Faltan Microsoft Visual Studio 2022 Build Tools con la carga de trabajo C++. Ejecuta scripts\setup-toolchain.ps1 desde una consola con permisos de administrador.'
  }
  $installation = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  if ([string]::IsNullOrWhiteSpace($installation)) {
    throw 'Visual Studio Build Tools no tiene instaladas las herramientas C++ x64.'
  }
  $msbuild = Join-Path $installation 'MSBuild\Current\Bin\MSBuild.exe'
  if (-not (Test-Path -LiteralPath $msbuild)) { throw 'No se encontró MSBuild en la instalación de Visual Studio.' }
  return $msbuild
}

New-Item -ItemType Directory -Force -Path $binaryDirectory, $buildDirectory | Out-Null
Assert-Command 'dotnet' 'Instala .NET SDK 10.'
Assert-Command 'cargo' 'Instala Rust estable con el toolchain MSVC.'

& cargo build --manifest-path (Join-Path $projectRoot 'Cargo.toml') --release -p camera-host
if ($LASTEXITCODE -ne 0) { throw 'Falló la compilación del host nativo Media Foundation.' }
$cameraHostOutput = Join-Path $projectRoot 'target\release\camera-tuner-camera-host.exe'
if (-not (Test-Path -LiteralPath $cameraHostOutput)) { throw 'Cargo no produjo camera-tuner-camera-host.exe.' }
Copy-BuildArtifact $cameraHostOutput $cameraHostDestination

# The bridge is inexpensive to publish and must always match Program.cs. The
# hash-aware copy keeps the packaged asset untouched when the output is equal.
& dotnet publish (Join-Path $projectRoot 'bridge\ControlWebcamBridge.csproj') `
  -c Release -r win-x64 --self-contained true -o $bridgeOutput
if ($LASTEXITCODE -ne 0) { throw 'Falló la compilación del puente DirectShow.' }
Copy-BuildArtifact (Join-Path $bridgeOutput 'control-webcam-bridge.exe') $bridgeDestination

if (-not $SkipNative) {
  $msbuild = Find-MSBuild
  if ($Force -or -not (Test-Path -LiteralPath $mediaSourceDestination)) {
    Assert-Command 'git' 'Instala Git para aplicar el parche versionado del Media Source.'
    $archive = Get-VerifiedArchive $windowsCamera 'windows-camera.zip'
    $extract = Join-Path $nativeBuildDirectory 'wc'
    Assert-NativeBuildPath $extract
    if (Test-Path -LiteralPath $extract) { Remove-Item -LiteralPath $extract -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $extract | Out-Null
    Expand-Archive -LiteralPath $archive -DestinationPath $extract -Force
    $sourceRoot = Get-ChildItem -LiteralPath $extract -Directory | Select-Object -First 1
    if ($null -eq $sourceRoot) { throw 'No se pudo extraer el código fijado de Windows-Camera.' }
    & git -C $sourceRoot.FullName apply --recount --ignore-space-change --ignore-whitespace (Join-Path $projectRoot 'native\windows-camera.patch')
    if ($LASTEXITCODE -ne 0) { throw 'No se pudo aplicar el parche de CameraTuner al Media Source de Microsoft.' }

    $mediaSourceProject = Join-Path $sourceRoot.FullName 'Samples\VirtualCamera\VirtualCameraMediaSource\VirtualCameraMediaSource.vcxproj'
    $mediaSourceDirectory = Split-Path $mediaSourceProject -Parent
    $cppWinRtPath = Join-Path $mediaSourceDirectory 'packages\Microsoft.Windows.CppWinRT.3.0.260520.1\bin'
    & $msbuild $mediaSourceProject /restore /m /p:RestorePackagesConfig=true /p:Configuration=Release /p:Platform=x64 "/p:SolutionDir=$mediaSourceDirectory/" "/p:CppWinRTPath=$cppWinRtPath/" /verbosity:minimal
    if ($LASTEXITCODE -ne 0) { throw 'Falló la compilación del Media Source de la cámara virtual.' }
    $mediaSource = Get-ChildItem -LiteralPath (Join-Path $sourceRoot.FullName 'Samples\VirtualCamera') -Filter 'VirtualCameraMediaSource.dll' -File -Recurse | Where-Object { $_.FullName -match '\\Release\\' } | Select-Object -First 1
    if ($null -eq $mediaSource) { throw 'MSBuild no produjo VirtualCameraMediaSource.dll.' }
    Copy-BuildArtifact $mediaSource.FullName $mediaSourceDestination
  }

  # This helper is cheap to build and must always reflect its C++ source. The
  # hash-aware copy avoids rewriting the packaged asset when nothing changed.
  $controlProject = Join-Path $projectRoot 'native\virtual-camera-control\virtual-camera-control.vcxproj'
  $controlOutput = Join-Path $buildDirectory 'virtual-camera-control'
  & $msbuild $controlProject /m /p:Configuration=Release /p:Platform=x64 "/p:OutDir=$controlOutput/" /verbosity:minimal
  if ($LASTEXITCODE -ne 0) { throw 'Falló la compilación del controlador de registro de la cámara virtual.' }
  Copy-BuildArtifact (Join-Path $controlOutput 'virtual-camera-control.exe') $virtualCameraDestination
}

Write-Host 'Recursos de CameraTuner preparados y verificados correctamente.'
