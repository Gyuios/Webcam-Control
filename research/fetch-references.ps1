[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$lockPath = Join-Path $PSScriptRoot 'reference-lock.json'
$destinationRoot = Join-Path $PSScriptRoot 'reference-implementations'
$lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json

New-Item -ItemType Directory -Path $destinationRoot -Force | Out-Null

foreach ($repository in $lock.repositories) {
    $destination = Join-Path $destinationRoot $repository.name

    if (Test-Path -LiteralPath $destination) {
        $actualCommit = (& git -C $destination rev-parse HEAD).Trim()
        $actualOrigin = (& git -C $destination remote get-url origin).Trim()

        if ($actualOrigin -ne $repository.url) {
            throw "El origen de '$($repository.name)' no coincide: '$actualOrigin'."
        }

        if ($actualCommit -ne $repository.commit) {
            throw "'$($repository.name)' está en $actualCommit y el lock exige $($repository.commit). No se modificó el clon."
        }

        Write-Host "[ok] $($repository.name) @ $actualCommit"
        continue
    }

    Write-Host "[clone] $($repository.name)"
    & git clone --filter=blob:none --no-checkout $repository.url $destination
    if ($LASTEXITCODE -ne 0) {
        throw "No se pudo clonar '$($repository.name)'."
    }

    & git -C $destination fetch --depth 1 origin $repository.commit
    if ($LASTEXITCODE -ne 0) {
        throw "No se pudo obtener la revisión fijada de '$($repository.name)'."
    }

    & git -C $destination checkout --detach $repository.commit
    if ($LASTEXITCODE -ne 0) {
        throw "No se pudo activar la revisión fijada de '$($repository.name)'."
    }

    Write-Host "[ok] $($repository.name) @ $($repository.commit)"
}

Write-Host 'Todas las referencias coinciden con reference-lock.json.'
