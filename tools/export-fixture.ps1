# Writes the settings export of one turnout build into tests/fixtures/exports,
# where tests/export_fixtures.rs imports every one of them with the current
# build. A file per released version, from that version's own binary, is how
# "a settings export from any version since v0.11 still imports" stays true.
#
#   pwsh tools/export-fixture.ps1 -Binary target/debug/turnout.exe
#   pwsh tools/export-fixture.ps1 -Binary C:\old\turnout.exe     # an older release
#
# The release ritual runs it once per version, after the bump. The catalog is
# made by the binary's own commands, through whatever its `--help` offers, so
# an older release writes exactly the shape it wrote for its users. Nothing it
# runs reaches a network or the OS keyring: the data directory is a fresh one,
# secrets go to the insecure file backend, the update check is off.
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [string]$OutDir = (Join-Path $PSScriptRoot "../tests/fixtures/exports")
)
$ErrorActionPreference = "Stop"

# The passphrase the sealed secrets in every fixture open with; the test reads
# the same one.
$passphrase = "fixture-passphrase"
# Where the project directories appear to live in the file. The sandbox path
# of the machine that wrote the fixture is replaced by this one, so no user
# name or home directory of whoever made the release ends up in the repository.
$neutralRoot = "C:\dev"

$Binary = (Resolve-Path $Binary).Path
$version = ((& $Binary --version) -split " ")[1]
$sandbox = Join-Path ([IO.Path]::GetTempPath()) "turnout-fixture-$version-$PID"
New-Item -ItemType Directory -Force $sandbox | Out-Null

$env:TURNOUT_DATA_DIR = Join-Path $sandbox "data"
$env:TURNOUT_KEYRING = "insecure-file"
$env:TURNOUT_UPDATE_CHECK = "0"
$env:TURNOUT_TOASTS = Join-Path $sandbox "toasts.jsonl"

# Every call gets a piped stdin: on a terminal the wizards would ask instead of
# taking the flags, and `pass set` and `export --with-secrets` read their
# secret from stdin when it is not a terminal.
function Invoke-Turnout([string]$stdin, [string[]]$arguments) {
    $output = $stdin | & $Binary @arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "turnout $($arguments -join ' ') failed ($LASTEXITCODE):`n$($output -join "`n")"
    }
}
function Help([string[]]$command) { (& $Binary @command --help 2>&1) -join "`n" }

try {
    $appHelp = Help @("app", "add")
    $credentialHelp = Help @("credential", "add")
    $targetHelp = Help @("target", "add")

    foreach ($name in "web", "api", "keys") { New-Item -ItemType Directory -Force (Join-Path $sandbox $name) | Out-Null }
    $keyFile = Join-Path $sandbox "keys/id_ed25519"
    Set-Content -Path $keyFile -Value "not a real key"

    Invoke-Turnout "" @("setup", "--yes")

    # Logins: a password with its secret, a key file with its passphrase, and
    # the agent where the release knows it (v0.13+).
    Invoke-Turnout "" @("credential", "add", "deploy", "--user", "deploy", "--auth", "password")
    Invoke-Turnout "" @("credential", "add", "ci-key", "--user", "ci", "--key", $keyFile)
    if ($credentialHelp -match "agent") {
        Invoke-Turnout "" @("credential", "add", "agent-login", "--user", "ops", "--auth", "agent")
    }
    Invoke-Turnout "fixture-password" @("pass", "set", "deploy")
    Invoke-Turnout "fixture-key-passphrase" @("pass", "set", "ci-key")

    Invoke-Turnout "" @("server", "add", "staging", "--url", "https://staging.example.com", "--label", "Staging", "--credential", "deploy")
    Invoke-Turnout "" @("server", "add", "prod", "--url", "https://app.example.com", "--host", "deploy.example.com:2222", "--credential", "ci-key")
    Invoke-Turnout "" @("server", "add", "lab", "--url", "https://lab.example.com", "--insecure")

    Invoke-Turnout "" @("path", "add", "site", "--dir", "/var/www/site", "--restart", "systemctl restart site")
    Invoke-Turnout "" @("path", "add", "api-dir", "--dir", "/srv/api")

    # `web` names every field its release has a flag for. `api` names none of
    # the optional ones - before v0.18 that meant an app with no gateway port.
    $web = @("app", "add", "web", "--path", (Join-Path $sandbox "web"), "--port", "7150", "--dist", "dist",
        "--command", "dev=npm run dev", "--command", "build=npm run build", "--server", "staging", "--server", "prod")
    if ($appHelp -match "--env-var") { $web += "--env-var", "VITE_API_URL" }
    if ($appHelp -match "--env-file") { $web += "--env-file", ".env.local" }
    if ($appHelp -match "--dev-port") { $web += "--dev-port", "5150" }
    Invoke-Turnout "" $web
    # `api` takes the first gateway port turnout hands out, 7100 - unless this
    # machine refuses it: Hyper-V and WSL reserve ranges of ports on Windows
    # (`netsh int ipv4 show excludedportrange protocol=tcp`), and one can cover
    # all of 7100-7199. Then the port is named, and the file is the one an
    # unobstructed machine writes.
    $api = @("app", "add", "api", "--path", (Join-Path $sandbox "api"), "--command", "build=cargo build --release")
    $first = $null
    try { $first = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 7100); $first.Start() } catch { $api += "--port", "7100" } finally { if ($first) { $first.Stop() } }
    Invoke-Turnout "" $api

    Invoke-Turnout "" @("group", "add", "everything", "--app", "web", "--app", "api")

    $staging = @("target", "add", "web-staging", "--app", "web", "--server", "staging", "--path", "site")
    if ($targetHelp -match "--url") { $staging += "--url", "https://web.staging.example.com" }
    if ($targetHelp -match "--health ") { $staging += "--health", "/health" }
    if ($targetHelp -match "--health-wait") { $staging += "--health-wait", "30" }
    if ($targetHelp -match "--keep-backups") { $staging += "--keep-backups", "3" }
    Invoke-Turnout "" $staging
    $prod = @("target", "add", "api-prod", "--app", "api", "--server", "prod", "--credential", "deploy", "--path", "api-dir")
    if ($targetHelp -match "--health-command") { $prod += "--health-command", "curl -fsS localhost:8080/health" }
    Invoke-Turnout "" $prod

    $file = Join-Path $sandbox "export.json"
    Invoke-Turnout $passphrase @("export", "--with-secrets", "--output", $file)

    $escaped = $sandbox.TrimEnd('\', '/').Replace('\', '\\')
    $json = [IO.File]::ReadAllText($file).Replace($escaped, $neutralRoot.Replace('\', '\\'))
    if ($json.Contains([IO.Path]::GetTempPath().Replace('\', '\\'))) { throw "the export still names the sandbox: $file" }

    New-Item -ItemType Directory -Force $OutDir | Out-Null
    $out = Join-Path $OutDir "v$version.json"
    [IO.File]::WriteAllText($out, $json.Replace("`r`n", "`n"))
    Write-Output "wrote $out"
}
finally {
    Remove-Item -Recurse -Force $sandbox -ErrorAction SilentlyContinue
}
