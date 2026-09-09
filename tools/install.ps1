# turnout installer for Windows:
#   irm https://raw.githubusercontent.com/lacodda/turnout/main/tools/install.ps1 | iex
$ErrorActionPreference = "Stop"

$repo = "lacodda/turnout"

# The tag comes from the /releases/latest redirect rather than the REST API:
# unauthenticated API calls are capped at 60 per hour per IP, and an installer
# that fails because someone else on the same address ran it is no installer.
# $env:TURNOUT_VERSION pins a specific release.
$tag = $env:TURNOUT_VERSION
if (-not $tag) {
    $request = [Net.HttpWebRequest]::Create("https://github.com/$repo/releases/latest")
    $request.AllowAutoRedirect = $false
    $request.UserAgent = "turnout-installer"
    try {
        $response = $request.GetResponse()
        $tag = ($response.Headers["Location"] -split "/")[-1]
        $response.Close()
    } catch {
        throw "Cannot resolve the latest release of ${repo}: $($_.Exception.Message)"
    }
}
if (-not $tag -or $tag -notmatch '^v\d') {
    throw "Cannot resolve the latest release of $repo - set `$env:TURNOUT_VERSION to a tag like v0.17.0"
}

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

# The user PATH is edited in the registry, not through the .NET environment
# API: that call rewrites the value as REG_SZ, and a PATH stored as REG_SZ no
# longer expands entries such as
# %JAVA_HOME%\bin - they turn into literal text and silently stop working.
# Reading the raw value keeps those entries as typed; writing it back as
# ExpandString keeps the type. A PATH that cannot be edited is not a failed
# install: the binary is in place, the user is told where.
try {
    $key = Get-Item "HKCU:\Environment"
    $raw = [string]$key.GetValue("Path", "", "DoNotExpandEnvironmentNames")
    $entries = @($raw -split ";" | Where-Object { $_ })
    $wanted = $dir.TrimEnd("\")
    $present = $entries | Where-Object { $_.TrimEnd("\") -ieq $wanted }
    if (-not $present) {
        $value = if ($entries.Count -gt 0) { ($entries + $wanted) -join ";" } else { $wanted }
        Set-ItemProperty -Path "HKCU:\Environment" -Name Path -Value $value -Type ExpandString
        # Tell running shells and Explorer that the environment changed, the
        # way the System Properties dialog does; without it only new logins see
        # the entry.
        if (-not ("TurnoutInstall.Env" -as [type])) {
            Add-Type -Namespace TurnoutInstall -Name Env -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll", SetLastError = true, CharSet = System.Runtime.InteropServices.CharSet.Unicode)]
public static extern System.IntPtr SendMessageTimeout(System.IntPtr hWnd, uint Msg, System.UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out System.UIntPtr lpdwResult);
'@
        }
        $result = [System.UIntPtr]::Zero
        # HWND_BROADCAST = 0xffff, WM_SETTINGCHANGE = 0x1A, SMTO_ABORTIFHUNG = 0x2
        [TurnoutInstall.Env]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, "Environment", 0x2, 5000, [ref]$result) | Out-Null
        Write-Host "Added $dir to your user PATH - open a new terminal to pick it up."
    }
} catch {
    Write-Host "Note: could not update the user PATH ($($_.Exception.Message)); add $dir to it yourself."
}
Write-Host "Installed turnout $tag to $dir\turnout.exe"

# Short alias `tn` as a hard link, not a copy: a copy doubles the install for
# no new code and goes stale the moment self-update replaces the binary. A
# symlink would need elevation on Windows; a hard link does not, as long as
# both names are on one volume - and they are, since the alias lands beside the
# binary. Skipped when another `tn` already answers in PATH;
# $env:TURNOUT_NO_ALIAS=1 opts out.
if (-not $env:TURNOUT_NO_ALIAS) {
    $alias = Join-Path $dir "tn.exe"
    $existing = Get-Command tn -ErrorAction SilentlyContinue
    if (-not $existing -or $existing.Source -eq $alias) {
        # A link cannot be created over an existing name, and an earlier
        # install may have left a full copy sitting there.
        Remove-Item $alias -Force -ErrorAction SilentlyContinue
        try {
            New-Item -ItemType HardLink -Path $alias -Target (Join-Path $dir "turnout.exe") -ErrorAction Stop | Out-Null
            Write-Host "Alias tn -> turnout"
        } catch {
            # A different volume, or a filesystem without hard links: a copy
            # still works, it just has to be refreshed by the installer.
            Copy-Item (Join-Path $dir "turnout.exe") $alias -Force
            Write-Host "Alias tn -> turnout (copied - this filesystem has no hard links)"
        }
    } else {
        Write-Host "Note: 'tn' already resolves to $($existing.Source) - alias skipped."
    }
}

# Binaries left behind by updates that ran before the alias became a link.
Remove-Item (Join-Path $dir "turnout.exe.old") -Force -ErrorAction SilentlyContinue
Remove-Item (Join-Path $dir "tn.exe.old") -Force -ErrorAction SilentlyContinue
