# Stands up the Windows server tests/windows_server.rs deploys to: OpenSSH
# Server on this machine, answering with cmd.exe as it does out of the box,
# and three local accounts that sign in to it with a password - two
# administrators, whose keys share C:\ProgramData\ssh\administrators_authorized_keys,
# and one ordinary account, whose key lives in its own profile.
#
# CI only: it adds accounts and starts a service on the machine it runs on, so
# it refuses to run outside GitHub Actions. Windows PowerShell 5.1, for the
# LocalAccounts cmdlets it ships with.

$ErrorActionPreference = 'Stop'

if (-not $env:GITHUB_ENV) {
    throw 'windows-sshd.ps1 sets up a throwaway CI runner and refuses to run anywhere else'
}

function Set-JobVariable([string]$Name, [string]$Value) {
    # Straight through .NET: 5.1's Out-File and Add-Content write UTF-16 or a
    # BOM, and the runner would read either as a broken variable name.
    [IO.File]::AppendAllText($env:GITHUB_ENV, "$Name=$Value`n")
}

function New-Password {
    # Letters and digits only: the password crosses a command line once, and
    # nothing in it should need quoting there.
    $alphabet = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789'
    $bytes = New-Object byte[] 32
    [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($bytes)
    $password = -join ($bytes | ForEach-Object { $alphabet[$_ % $alphabet.Length] })
    # One of each class, whatever the complexity policy asks.
    "${password}Aa7"
}

function New-Account([string]$Name, [string]$Variable, [switch]$Administrator) {
    $password = New-Password
    Write-Host "::add-mask::$password"
    $secure = ConvertTo-SecureString $password -AsPlainText -Force
    New-LocalUser -Name $Name -Password $secure -PasswordNeverExpires -AccountNeverExpires | Out-Null
    if ($Administrator) {
        # By SID: the group's name is translated on a localized Windows.
        Add-LocalGroupMember -SID 'S-1-5-32-544' -Member $Name
    }
    Set-JobVariable $Variable $Name
    Set-JobVariable "${Variable}_PASSWORD" $password
    Write-Host "Account $Name$(if ($Administrator) { ' (administrator)' })"
}

# Windows Server 2025 ships the server and leaves it stopped; older images
# install it as a capability.
if (-not (Get-Service sshd -ErrorAction SilentlyContinue)) {
    Write-Host 'Installing OpenSSH Server'
    Add-WindowsCapability -Online -Name 'OpenSSH.Server~~~~0.0.1.0' | Out-Null
}
Start-Service sshd

$deadline = (Get-Date).AddSeconds(60)
while ($true) {
    $client = New-Object Net.Sockets.TcpClient
    try {
        $client.Connect('127.0.0.1', 22)
        break
    } catch {
        if ((Get-Date) -gt $deadline) { throw 'sshd did not start listening on 127.0.0.1:22' }
        Start-Sleep -Seconds 1
    } finally {
        $client.Dispose()
    }
}
$version = (Get-Item "$env:WINDIR\System32\OpenSSH\sshd.exe" -ErrorAction SilentlyContinue).VersionInfo.ProductVersion
Write-Host "sshd $version listening on 127.0.0.1:22"

# Who sshd lets in, as it reads its config - the first thing to know when a
# sign-in is refused.
& "$env:WINDIR\System32\OpenSSH\sshd.exe" -T |
    Select-String -Pattern '^(allowgroups|allowusers|denygroups|denyusers|passwordauthentication|pubkeyauthentication|authorizedkeysfile) ' |
    ForEach-Object { Write-Host "sshd: $_" }

New-Account 'turnout-admin' 'TURNOUT_E2E_ADMIN' -Administrator
New-Account 'turnout-admin2' 'TURNOUT_E2E_SECOND_ADMIN' -Administrator
New-Account 'turnout-user' 'TURNOUT_E2E_USER'
# Windows Server 2025 admits only `AllowGroups administrators "openssh users"`:
# an ordinary deploy account has to be put in the second group, on a real
# stand as here.
if (Get-LocalGroup -Name 'OpenSSH Users' -ErrorAction SilentlyContinue) {
    Add-LocalGroupMember -Group 'OpenSSH Users' -Member 'turnout-user'
    Write-Host 'Account turnout-user added to OpenSSH Users'
}

# Where the deploys land. The administrators may write there: it is the
# deploy account's directory on a real stand too.
$deploys = Join-Path $env:RUNNER_TEMP 'turnout e2e'
New-Item -ItemType Directory -Force $deploys | Out-Null
& icacls $deploys /grant '*S-1-5-32-544:(OI)(CI)F' | Out-Null
if ($LASTEXITCODE -ne 0) { throw "icacls could not open $deploys to the administrators" }

Set-JobVariable 'TURNOUT_E2E_SSH' '127.0.0.1:22'
Set-JobVariable 'TURNOUT_E2E_DIR' $deploys
