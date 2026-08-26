$ErrorActionPreference = "Stop"

$version = "2.1.25156"
$expectedHash = "073A70E00F77423E34BED98B86E600DEF93393BA5822204FAC57A29324DB9F7A"
$url = "https://github.com/winfsp/winfsp/releases/download/v2.1/winfsp-$version.msi"
$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\..\..\.."))
$cacheDirectory = Join-Path $repositoryRoot ".cache\winfsp"
$destination = Join-Path $cacheDirectory "winfsp-$version.msi"
$download = "$destination.download"

function Get-Sha256Hash {
    param([Parameter(Mandatory = $true)][string]$Path)

    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        try {
            return ([System.BitConverter]::ToString($sha256.ComputeHash($stream))).Replace("-", "")
        } finally {
            $sha256.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

New-Item -ItemType Directory -Force -Path $cacheDirectory | Out-Null

if (Test-Path $destination) {
    $actualHash = Get-Sha256Hash $destination
    if ($actualHash -eq $expectedHash) {
        Write-Host "Using verified WinFsp $version installer from $destination"
        exit 0
    }
    Remove-Item -Force $destination
}

try {
    Invoke-WebRequest -Uri $url -OutFile $download
    $actualHash = Get-Sha256Hash $download
    if ($actualHash -ne $expectedHash) {
        throw "WinFsp installer checksum mismatch. Expected $expectedHash, received $actualHash."
    }
    Move-Item -Force $download $destination
    Write-Host "Downloaded and verified WinFsp $version installer."
} finally {
    if (Test-Path $download) {
        Remove-Item -Force $download
    }
}