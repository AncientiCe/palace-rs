$ErrorActionPreference = "Stop"

$Repo = if ($env:PALACE_REPO) { $env:PALACE_REPO } else { "AncientiCe/palace-rs" }
$InstallDir = if ($env:PALACE_INSTALL_DIR) { $env:PALACE_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\palace\bin" }
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("palace-" + [System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $TempDir | Out-Null

function Get-Target {
    # PowerShell 5.1's .NET Framework may not expose RuntimeInformation.OSArchitecture,
    # so fall back to the PROCESSOR_ARCHITECTURE environment variable.
    $arch = $null
    try {
        $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    } catch { }
    if (-not $arch) {
        $arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
    }
    switch -Regex ($arch) {
        "^(X64|AMD64)$" { return "x86_64-pc-windows-msvc" }
        "^ARM64$" { return "aarch64-pc-windows-msvc" }
        default { throw "Unsupported architecture: '$arch'" }
    }
}

function Install-Binary {
    param([string]$Source, [string]$Dest)
    $Dir = Split-Path $Dest
    # Binaries moved aside by earlier installs: remove the ones nothing runs any more.
    Get-ChildItem -Path $Dir -Filter "palace.old-*.exe" -ErrorAction SilentlyContinue |
        ForEach-Object { Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue }
    try {
        Copy-Item $Source $Dest -Force
        return
    } catch {
        if (-not (Test-Path $Dest)) { throw }
    }
    # A running palace (an agent's MCP server or hook) locks palace.exe against
    # writes, but Windows still allows renaming it: move it aside, copy the new one.
    $Aside = Join-Path $Dir ("palace.old-" + (Get-Date -Format "yyyyMMddHHmmss") + ".exe")
    Move-Item $Dest $Aside -Force
    try {
        Copy-Item $Source $Dest -Force
    } catch {
        Move-Item $Aside $Dest -Force
        throw
    }
    Write-Host "palace.exe was in use; the old one is now $(Split-Path $Aside -Leaf) and is deleted by the next install."
    Write-Host "Restart running agents to load the new version."
}

try {
    $Target = Get-Target

    $VersionOverride = if ($env:PALACE_VERSION) { $env:PALACE_VERSION } else { $null }
    $LocalArchive = if ($env:PALACE_LOCAL_ARCHIVE) { $env:PALACE_LOCAL_ARCHIVE } else { $null }

    if ($VersionOverride -eq "local") {
        if (-not $LocalArchive) {
            throw "PALACE_LOCAL_ARCHIVE is required when PALACE_VERSION=local"
        }
        $Archive = $LocalArchive
    } else {
        if ($VersionOverride) {
            # Release tags start with "v"; accept "0.20.5" as well as "v0.20.5".
            $Tag = if ($VersionOverride.StartsWith("v")) { $VersionOverride } else { "v$VersionOverride" }
        } else {
            $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
            $Tag = $Release.tag_name
        }
        $Version = $Tag.TrimStart("v")
        $Asset = "palace-$Version-$Target.zip"
        $Archive = Join-Path $TempDir $Asset
        $Checksum = Join-Path $TempDir "palace-$Target.sha256"
        Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$Tag/$Asset" -OutFile $Archive
        Invoke-WebRequest -Uri "https://github.com/$Repo/releases/download/$Tag/palace-$Target.sha256" -OutFile $Checksum

        $Expected = ((Get-Content $Checksum | Select-Object -First 1) -split "\s+")[0]
        $Actual = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
        if ($Actual -ne $Expected.ToLowerInvariant()) {
            throw "Checksum mismatch for $Asset"
        }
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Expand-Archive -Path $Archive -DestinationPath $TempDir -Force
    $Binary = Get-ChildItem -Path $TempDir -Recurse -Filter "palace.exe" | Select-Object -First 1
    if (-not $Binary) {
        throw "Archive did not contain palace.exe"
    }
    Install-Binary $Binary.FullName (Join-Path $InstallDir "palace.exe")


    # Add the install dir to the *user* PATH in the registry (never use `setx PATH "...;%PATH%"`:
    # PowerShell does not expand %PATH%, and setx would overwrite the user PATH with a literal string).
    $UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
    $UserParts = if ($UserPath) { ($UserPath -split ";") | Where-Object { $_ } } else { @() }
    if ($UserParts -notcontains $InstallDir) {
        $NewUserPath = if ($UserPath) { ($UserPath.TrimEnd(";") + ";" + $InstallDir) } else { $InstallDir }
        [Environment]::SetEnvironmentVariable("Path", $NewUserPath, "User")
        Write-Host "Added $InstallDir to your user PATH. Restart your terminal to pick it up."
    }
    if ((($env:PATH -split ";") | Where-Object { $_ }) -notcontains $InstallDir) {
        $env:PATH = "$InstallDir;$env:PATH"
    }

    & (Join-Path $InstallDir "palace.exe") install --all

    Write-Host "palace installed."
    Write-Host "Next: palace init <project>; palace mine <project>"
    Write-Host "Restart Cursor, Codex, or Claude Code to load the MCP server."
} finally {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}
