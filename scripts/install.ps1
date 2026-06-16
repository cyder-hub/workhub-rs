$ErrorActionPreference = "Stop"

$Repo = "cyder-hub/workhub-rs"
$RepoUrl = "https://github.com/$Repo"
$ApiUrl = "https://api.github.com/repos/$Repo/releases/latest"
$LocalAppData = $env:LOCALAPPDATA
if ([string]::IsNullOrWhiteSpace($LocalAppData)) {
    $LocalAppData = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
}
if ([string]::IsNullOrWhiteSpace($LocalAppData)) {
    throw "Unable to find the Windows LocalApplicationData directory"
}
$InstallDir = Join-Path $LocalAppData "Programs\workhub\bin"
$InstallPath = Join-Path $InstallDir "workhub.exe"
$Headers = @{ "User-Agent" = "workhub-installer" }

function Fail {
    param([string] $Message)
    Write-Error "Error: $Message"
    exit 1
}

function Test-Windows {
    $isWindowsVariable = Get-Variable -Name IsWindows -ErrorAction SilentlyContinue
    if ($null -ne $isWindowsVariable) {
        return ([bool]$isWindowsVariable.Value)
    }
    return $true
}

function Get-AssetName {
    if (-not (Test-Windows)) {
        Fail "this installer is for Windows; use install.sh on Linux or macOS"
    }

    $arch = $env:PROCESSOR_ARCHITEW6432
    if ([string]::IsNullOrWhiteSpace($arch)) {
        $arch = $env:PROCESSOR_ARCHITECTURE
    }
    if ([string]::IsNullOrWhiteSpace($arch)) {
        Fail "unable to detect Windows CPU architecture"
    }

    switch ($arch) {
        "AMD64" { return "workhub-windows-x86_64.exe" }
        "x86_64" { return "workhub-windows-x86_64.exe" }
        default { Fail "unsupported Windows CPU architecture: $arch" }
    }
}

function Get-LatestTag {
    try {
        $release = Invoke-RestMethod -Uri $ApiUrl -Headers $Headers
    } catch {
        Fail "failed to read latest GitHub release: $($_.Exception.Message)"
    }

    if ([string]::IsNullOrWhiteSpace($release.tag_name)) {
        Fail "latest GitHub release does not contain tag_name"
    }

    return ([string]$release.tag_name)
}

function Get-InstalledVersion {
    if (-not (Test-Path -LiteralPath $InstallPath)) {
        return $null
    }

    try {
        $output = & $InstallPath -v 2>$null | Select-Object -First 1
        if ([string]::IsNullOrWhiteSpace($output)) {
            return "unknown"
        }
        return ([string]$output)
    } catch {
        return "unknown"
    }
}

function Compare-WorkhubVersion {
    param(
        [string] $Current,
        [string] $Latest
    )

    try {
        $currentVersion = [version] $Current
        $latestVersion = [version] $Latest
        return $currentVersion.CompareTo($latestVersion)
    } catch {
        return $null
    }
}

function Save-Uri {
    param(
        [string] $Uri,
        [string] $OutFile
    )

    try {
        Invoke-WebRequest -Uri $Uri -OutFile $OutFile -UseBasicParsing -Headers $Headers
    } catch {
        Fail "failed to download $Uri`: $($_.Exception.Message)"
    }
}

function Test-Checksum {
    param(
        [string] $AssetPath,
        [string] $ChecksumPath
    )

    $checksumLine = Get-Content -LiteralPath $ChecksumPath -TotalCount 1
    if ([string]::IsNullOrWhiteSpace($checksumLine)) {
        Fail "checksum file is empty"
    }

    $expected = (([string]$checksumLine).Trim() -split "\s+")[0].ToLowerInvariant()
    if ([string]::IsNullOrWhiteSpace($expected)) {
        Fail "checksum file is empty"
    }

    $actual = (Get-FileHash -LiteralPath $AssetPath -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) {
        Fail "checksum verification failed"
    }
}

function Path-ContainsInstallDir {
    param([string] $PathValue)

    if ([string]::IsNullOrWhiteSpace($PathValue)) {
        return $false
    }

    $normalizedInstallDir = $InstallDir.TrimEnd([char]"\")
    foreach ($entry in ($PathValue -split ";")) {
        if ($entry.Trim().TrimEnd([char]"\") -ieq $normalizedInstallDir) {
            return $true
        }
    }
    return $false
}

function Ensure-PathEntry {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (Path-ContainsInstallDir $userPath) {
        return
    }

    if ([string]::IsNullOrWhiteSpace($userPath)) {
        $newPath = $InstallDir
    } else {
        $newPath = "$userPath;$InstallDir"
    }

    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    if (-not (Path-ContainsInstallDir $env:Path)) {
        $env:Path = "$env:Path;$InstallDir"
    }

    Write-Host "Added $InstallDir to your user PATH."
    Write-Host "Restart your terminal before running workhub from a new shell."
}

function Remove-PathEntry {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if ([string]::IsNullOrWhiteSpace($userPath)) {
        return
    }

    $normalizedInstallDir = $InstallDir.TrimEnd([char]"\")
    $kept = @()
    foreach ($entry in ($userPath -split ";")) {
        if ([string]::IsNullOrWhiteSpace($entry)) {
            continue
        }
        if ($entry.Trim().TrimEnd([char]"\") -ieq $normalizedInstallDir) {
            continue
        }
        $kept += $entry
    }

    [Environment]::SetEnvironmentVariable("Path", ($kept -join ";"), "User")
}

function Install-Latest {
    param(
        [string] $Tag,
        [string] $LatestVersion,
        [string] $AssetName
    )

    $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) "workhub-install-$([guid]::NewGuid().ToString("N"))"
    New-Item -ItemType Directory -Path $tempRoot | Out-Null

    try {
        $assetPath = Join-Path $tempRoot $AssetName
        $checksumPath = Join-Path $tempRoot "$AssetName.sha256"
        $assetUrl = "$RepoUrl/releases/download/$Tag/$AssetName"

        Write-Host "Downloading $AssetName..."
        Save-Uri -Uri $assetUrl -OutFile $assetPath
        Save-Uri -Uri "$assetUrl.sha256" -OutFile $checksumPath
        Test-Checksum -AssetPath $assetPath -ChecksumPath $checksumPath

        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        Copy-Item -LiteralPath $assetPath -Destination $InstallPath -Force

        $installed = (& $InstallPath -v 2>$null | Select-Object -First 1)
        if ($installed -ne $LatestVersion) {
            Fail "installed version check failed: expected $LatestVersion, got $installed"
        }

        Write-Host "Installed workhub $installed at $InstallPath."
        Ensure-PathEntry
    } finally {
        Remove-Item -LiteralPath $tempRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Uninstall-Workhub {
    if (Test-Path -LiteralPath $InstallPath) {
        Remove-Item -LiteralPath $InstallPath -Force
        Write-Host "Removed $InstallPath."
    } else {
        Write-Host "No workhub binary found at $InstallPath."
    }

    Remove-PathEntry
    try {
        Remove-Item -LiteralPath $InstallDir -Force -ErrorAction Stop
    } catch {
    }

    Write-Host "Uninstalled workhub."
}

function Prompt-Install {
    param(
        [string] $LatestVersion,
        [string] $Tag,
        [string] $AssetName
    )

    $answer = Read-Host "Install workhub $LatestVersion? [Y/n]"
    switch -Regex ($answer) {
        "^\s*$" { Install-Latest -Tag $Tag -LatestVersion $LatestVersion -AssetName $AssetName; return }
        "^(y|yes)$" { Install-Latest -Tag $Tag -LatestVersion $LatestVersion -AssetName $AssetName; return }
        default { Write-Host "Canceled."; return }
    }
}

function Prompt-UpdateOrUninstall {
    param(
        [string] $LatestVersion,
        [string] $Tag,
        [string] $AssetName,
        [string] $DefaultChoice,
        [string] $UpdateLabel
    )

    Write-Host "Choose an action:"
    Write-Host "1. $UpdateLabel"
    Write-Host "2. Uninstall workhub"
    Write-Host "3. Cancel"
    $answer = Read-Host "Enter choice [$DefaultChoice]"
    if ([string]::IsNullOrWhiteSpace($answer)) {
        $answer = $DefaultChoice
    }

    switch ($answer) {
        "1" { Install-Latest -Tag $Tag -LatestVersion $LatestVersion -AssetName $AssetName }
        "2" { Uninstall-Workhub }
        default { Write-Host "Canceled." }
    }
}

function Prompt-UninstallOrCancel {
    Write-Host "workhub is already up to date."
    Write-Host ""
    Write-Host "Choose an action:"
    Write-Host "1. Uninstall workhub"
    Write-Host "2. Cancel"
    $answer = Read-Host "Enter choice [2]"
    if ([string]::IsNullOrWhiteSpace($answer)) {
        $answer = "2"
    }

    switch ($answer) {
        "1" { Uninstall-Workhub }
        default { Write-Host "Canceled." }
    }
}

$AssetName = Get-AssetName
$LatestTag = Get-LatestTag
if ([string]::IsNullOrWhiteSpace($LatestTag)) {
    Fail "latest GitHub release tag is empty"
}
$LatestVersion = ([string]$LatestTag).TrimStart([char]"v")
$InstalledVersion = Get-InstalledVersion
$InstalledDisplay = if ($null -eq $InstalledVersion) { "not installed" } else { $InstalledVersion }

Write-Host "workhub installer"
Write-Host ""
Write-Host "Platform: windows-x86_64"
Write-Host "Install path: $InstallPath"
Write-Host "Installed: $InstalledDisplay"
Write-Host "Latest: $LatestVersion"
Write-Host ""

if ($null -eq $InstalledVersion) {
    Prompt-Install -LatestVersion $LatestVersion -Tag $LatestTag -AssetName $AssetName
    exit 0
}

$comparison = Compare-WorkhubVersion -Current $InstalledVersion -Latest $LatestVersion
if ($null -eq $comparison) {
    Prompt-UpdateOrUninstall -LatestVersion $LatestVersion -Tag $LatestTag -AssetName $AssetName -DefaultChoice "1" -UpdateLabel "Install $LatestVersion over the current binary"
} elseif ($comparison -eq 0) {
    Prompt-UninstallOrCancel
} elseif ($comparison -lt 0) {
    Prompt-UpdateOrUninstall -LatestVersion $LatestVersion -Tag $LatestTag -AssetName $AssetName -DefaultChoice "1" -UpdateLabel "Update to $LatestVersion"
} else {
    Write-Host "Installed version is newer than the latest GitHub release."
    Write-Host ""
    Prompt-UpdateOrUninstall -LatestVersion $LatestVersion -Tag $LatestTag -AssetName $AssetName -DefaultChoice "3" -UpdateLabel "Reinstall $LatestVersion"
}
