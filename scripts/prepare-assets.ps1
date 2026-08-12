param(
  [switch]$Force
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$binaryDirectory = Join-Path $projectRoot 'src-tauri\binaries'
$bridgeOutput = Join-Path $projectRoot 'bridge\publish'
$bridgeDestination = Join-Path $binaryDirectory 'control-webcam-bridge-x86_64-pc-windows-msvc.exe'
$ffmpegDestination = Join-Path $binaryDirectory 'ffmpeg.exe'

New-Item -ItemType Directory -Force -Path $binaryDirectory | Out-Null

if ($Force -or -not (Test-Path -LiteralPath $bridgeDestination)) {
  dotnet publish (Join-Path $projectRoot 'bridge\ControlWebcamBridge.csproj') `
    -c Release -r win-x64 --self-contained true -o $bridgeOutput
  Copy-Item -LiteralPath (Join-Path $bridgeOutput 'control-webcam-bridge.exe') -Destination $bridgeDestination -Force
}

if ($Force -or -not (Test-Path -LiteralPath $ffmpegDestination)) {
  $tempRoot = Join-Path $env:TEMP ('control-webcam-ffmpeg-' + [guid]::NewGuid().ToString())
  $archive = Join-Path $tempRoot 'ffmpeg-release-essentials.zip'
  $extract = Join-Path $tempRoot 'extract'
  New-Item -ItemType Directory -Force -Path $tempRoot, $extract | Out-Null

  Invoke-WebRequest -Uri 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip' -OutFile $archive
  Expand-Archive -LiteralPath $archive -DestinationPath $extract -Force
  $ffmpeg = Get-ChildItem -Path $extract -Filter 'ffmpeg.exe' -Recurse | Select-Object -First 1
  if ($null -eq $ffmpeg) { throw 'No se encontró ffmpeg.exe en el paquete descargado.' }
  Copy-Item -LiteralPath $ffmpeg.FullName -Destination $ffmpegDestination -Force
}

Write-Host 'Recursos de Control Webcam preparados correctamente.'

