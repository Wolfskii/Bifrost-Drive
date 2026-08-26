[CmdletBinding()]
param(
    [switch]$ListOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if ($env:OS -ne "Windows_NT") {
    throw "Bifrost drive cleanup is available only on Windows."
}

function Get-BifrostIdentity {
    param([AllowNull()][string]$Value)

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return $null
    }

    if ($Value -match '^\\+(?<server>bifrost-(?<pid>\d+)-\d+)\\(?<share>.+)$' -or
        $Value -match '^##(?<server>bifrost-(?<pid>\d+)-\d+)#(?<share>.*)$') {
        return [pscustomobject]@{
            Key = ("{0}#{1}" -f $Matches.server, $Matches.share).ToLowerInvariant()
            OwnerPid = [int]$Matches.pid
            Server = $Matches.server
            Share = $Matches.share
        }
    }

    return $null
}

function Test-BifrostOwnerActive {
    param([int]$OwnerPid)

    try {
        return (Get-Process -Id $OwnerPid -ErrorAction Stop).ProcessName -ieq "bifrost-drive"
    }
    catch {
        return $false
    }
}

function Get-NormalizedDriveLetter {
    param([AllowNull()][string]$Value)

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return $null
    }
    $letter = $Value.Trim().TrimEnd(':').ToUpperInvariant()
    if ($letter -notmatch '^[A-Z]$') {
        throw "Invalid drive letter '$Value'."
    }
    return $letter
}

function Get-ProtectedDriveLetters {
    $letters = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $operatingSystem = Get-CimInstance Win32_OperatingSystem
    foreach ($value in @($env:SystemDrive, $operatingSystem.SystemDrive)) {
        $letter = Get-NormalizedDriveLetter $value
        if ($null -ne $letter) {
            $null = $letters.Add($letter)
        }
    }
    return ,$letters
}

function Get-RegistryPropertyValue {
    param(
        [string]$Path,
        [string]$Name
    )

    $item = Get-ItemProperty -LiteralPath $Path -ErrorAction SilentlyContinue
    if ($null -eq $item) {
        return $null
    }
    $property = $item.PSObject.Properties[$Name]
    if ($null -eq $property) {
        return $null
    }
    return $property.Value
}

function New-CleanupRecord {
    param(
        [string]$Kind,
        [string]$RegistryPath,
        [AllowNull()][string]$Letter,
        [pscustomobject]$Identity
    )

    [pscustomobject]@{
        Kind = $Kind
        RegistryPath = $RegistryPath
        Letter = $Letter
        IdentityKey = $Identity.Key
        OwnerPid = $Identity.OwnerPid
        Server = $Identity.Server
        Share = $Identity.Share
    }
}

function Invoke-ExplorerRefresh {
    Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class BifrostShellRefresh {
    [DllImport("shell32.dll")]
    public static extern void SHChangeNotify(uint eventId, uint flags, IntPtr item1, IntPtr item2);
}
"@
    [BifrostShellRefresh]::SHChangeNotify(0x08000000, 0x1000, [IntPtr]::Zero, [IntPtr]::Zero)
    Get-Process explorer -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Process explorer.exe
}

$protectedDriveLetters = Get-ProtectedDriveLetters
$records = @()
$networkRoot = "Registry::HKEY_CURRENT_USER\Network"
if (Test-Path -LiteralPath $networkRoot) {
    foreach ($key in Get-ChildItem -LiteralPath $networkRoot) {
        $remotePath = Get-RegistryPropertyValue $key.PSPath "RemotePath"
        $identity = Get-BifrostIdentity $remotePath
        if ($null -ne $identity) {
            $letter = Get-NormalizedDriveLetter $key.PSChildName
            if (-not $protectedDriveLetters.Contains($letter)) {
                $records += New-CleanupRecord "Network mapping" $key.PSPath $letter $identity
            }
        }
    }
}

foreach ($disk in Get-CimInstance Win32_LogicalDisk) {
    $identity = Get-BifrostIdentity $disk.ProviderName
    if ($null -ne $identity) {
        $letter = Get-NormalizedDriveLetter $disk.DeviceID
        if (-not $protectedDriveLetters.Contains($letter)) {
            $records += New-CleanupRecord "Logical drive" "" $letter $identity
        }
    }
}

$mountPointsRoot = "Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Explorer\MountPoints2"
if (Test-Path -LiteralPath $mountPointsRoot) {
    foreach ($key in Get-ChildItem -LiteralPath $mountPointsRoot) {
        $identity = Get-BifrostIdentity $key.PSChildName
        if ($null -ne $identity) {
            $records += New-CleanupRecord "Explorer metadata" $key.PSPath $null $identity
        }
    }
}

$staleGroups = @(
    $records |
        Where-Object { -not (Test-BifrostOwnerActive $_.OwnerPid) } |
        Group-Object IdentityKey
)

if ($staleGroups.Count -eq 0) {
    Write-Host "No stale Bifrost drive entries were found."
    exit 0
}

Write-Host "Stale Bifrost drive entries:"
for ($index = 0; $index -lt $staleGroups.Count; $index++) {
    $group = $staleGroups[$index].Group
    $letters = @($group.Letter | Where-Object { $_ } | Sort-Object -Unique)
    $location = if ($letters.Count -gt 0) {
        ($letters | ForEach-Object { "${_}:" }) -join ", "
    }
    else {
        "Explorer metadata only"
    }
    $kinds = ($group.Kind | Sort-Object -Unique) -join ", "
    Write-Host ("[{0}] {1}  {2}  ({3}; owner PID {4} is inactive)" -f ($index + 1), $location, $group[0].Share, $kinds, $group[0].OwnerPid)
}

if ($ListOnly) {
    exit 0
}

Write-Host "[A] Remove all listed entries"
Write-Host "[Q] Quit"
$selection = (Read-Host "Select an entry").Trim()
if ($selection -ieq "Q") {
    exit 0
}

if ($selection -ieq "A") {
    $selectedGroups = $staleGroups
}
else {
    $number = 0
    if (-not [int]::TryParse($selection, [ref]$number) -or $number -lt 1 -or $number -gt $staleGroups.Count) {
        throw "Invalid selection '$selection'."
    }
    $selectedGroups = @($staleGroups[$number - 1])
}

$driveIconsRoot = "Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Explorer\DriveIcons"
foreach ($selected in $selectedGroups) {
    $group = $selected.Group
    $letters = @($group.Letter | Where-Object { $_ } | Sort-Object -Unique)

    foreach ($letter in $letters) {
        $letter = Get-NormalizedDriveLetter $letter
        if ($protectedDriveLetters.Contains($letter)) {
            throw "Refusing to remove protected operating-system drive ${letter}:."
        }

        $disk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='${letter}:'" -ErrorAction SilentlyContinue
        $diskIdentity = if ($null -eq $disk) {
            $null
        }
        else {
            Get-BifrostIdentity $disk.ProviderName
        }
        if ($null -ne $disk -and ($null -eq $diskIdentity -or $diskIdentity.Key -ne $selected.Name)) {
            throw "Refusing to remove ${letter}: because it is a current non-Bifrost drive."
        }

        $networkKey = Join-Path $networkRoot $letter
        if (Test-Path -LiteralPath $networkKey) {
            $remotePath = Get-RegistryPropertyValue $networkKey "RemotePath"
            $identity = Get-BifrostIdentity $remotePath
            if ($null -ne $identity -and $identity.Key -eq $selected.Name) {
                Remove-Item -LiteralPath $networkKey -Recurse -Force
            }
        }

        if ($null -ne $diskIdentity -and $diskIdentity.Key -eq $selected.Name) {
            & net.exe use "${letter}:" /delete /y 2>$null | Out-Null
        }

        $letterMountPoint = Join-Path $mountPointsRoot $letter
        if (Test-Path -LiteralPath $letterMountPoint) {
            Remove-Item -LiteralPath $letterMountPoint -Recurse -Force
        }

        $driveIconKey = Join-Path $driveIconsRoot $letter
        if (Test-Path -LiteralPath $driveIconKey) {
            Remove-Item -LiteralPath $driveIconKey -Recurse -Force
        }
    }

    foreach ($record in $group | Where-Object { $_.Kind -eq "Explorer metadata" }) {
        if (Test-Path -LiteralPath $record.RegistryPath) {
            Remove-Item -LiteralPath $record.RegistryPath -Recurse -Force
        }
    }

    Write-Host ("Removed stale Bifrost entry for {0}." -f $group[0].Share)
}

Invoke-ExplorerRefresh