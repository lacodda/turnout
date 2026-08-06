# turnout installer for Windows:
#   irm https://raw.githubusercontent.com/lacodda/turnout/main/tools/install.ps1 | iex
$ErrorActionPreference = "Stop"

$repo = "lacodda/turnout"
$tag = (Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest").tag_name
if (-not $tag) { throw "Cannot resolve the latest release of $repo" }

$name = "turnout-$tag-x86_64-pc-windows-msvc"
$url = "https://github.com/$repo/releases/download/$tag/$name.zip"
$dir = if ($env:TURNOUT_INSTALL_DIR) { $env:TURNOUT_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Programs\turnout" }
$tmp = Join-Path ([IO.Path]::GetTempPath()) "turnout-install-$([guid]::NewGuid())"
New-Item -ItemType Directory -Force $tmp | Out-Null

try {
    Write-Host "Downloading $url"
    Invoke-WebRequest $url -OutFile (Join-Path $tmp "turnout.zip")
    Expand-Archive (Join-Path $tmp "turnout.zip") -DestinationPath $tmp -Force
    New-Item -ItemType Directory -Force $dir | Out-Null
    Copy-Item (Join-Path $tmp "$name\turnout.exe") $dir -Force
} finally {
    Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (($userPath -split ";") -notcontains $dir) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$dir", "User")
    Write-Host "Added $dir to your user PATH - restart the terminal to pick it up."
}
Write-Host "Installed turnout $tag to $dir\turnout.exe"
