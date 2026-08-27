$ErrorActionPreference = "Stop"

if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
    throw "Docker CLI was not found. Install Docker Desktop and try again."
}

function Test-DockerDaemon {
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    & docker info *> $null
    $isReady = $LASTEXITCODE -eq 0
    $ErrorActionPreference = $previousErrorActionPreference
    return $isReady
}

if (Test-DockerDaemon) {
    Write-Host "Docker is ready."
    exit 0
}

$candidates = @(
    (Join-Path $env:ProgramFiles "Docker\Docker\Docker Desktop.exe"),
    (Join-Path $env:LOCALAPPDATA "Docker\Docker Desktop.exe")
)
$dockerDesktop = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $dockerDesktop) {
    throw "Docker is not running and Docker Desktop could not be found."
}

Write-Host "Starting Docker Desktop..."
Start-Process -FilePath $dockerDesktop | Out-Null
$deadline = (Get-Date).AddMinutes(2)
while (-not (Test-DockerDaemon)) {
    if ((Get-Date) -ge $deadline) {
        throw "Docker Desktop did not become ready within two minutes."
    }
    Start-Sleep -Seconds 2
}

Write-Host "Docker is ready."