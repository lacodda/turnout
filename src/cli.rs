use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Point local apps at any backend stand, keep servers and secrets at hand,
/// build and deploy - from any directory.
#[derive(Parser)]
#[command(name = "turnout", version, about, propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

/// Where a job's output goes: the flags every job-running command shares.
///
/// One definition flattened into `dev`, `build`, `test`, `lint`, `run` and
/// `deploy`, so the six cannot drift apart - and so clap builds these
/// arguments in a function of their own. The derive builds every subcommand's
/// arguments in one function, and in a debug build each argument keeps its
/// own slot of that function's stack frame.
#[derive(Args, Clone, Copy, Default)]
pub struct Console {
    /// Stream the output in full instead of a loader
    #[arg(short, long)]
    pub verbose: bool,
    /// Run in the background and return at once; see `turnout ps`, `logs`, `stop`
    #[arg(short, long, conflicts_with = "verbose")]
    pub detach: bool,
    /// Stay in this terminal even when TURNOUT_DETACH sends the command to the background
    #[arg(long, conflicts_with = "detach")]
    pub foreground: bool,
}

/// How a command that reports prints: for a person, or for a program.
///
/// One definition flattened into every reading command, so `--json` means the
/// same everywhere and only appears where it does something.
#[derive(Args, Clone, Copy, Default)]
pub struct Format {
    /// Print JSON instead of text - the same fields, for scripts and status bars
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize the data directory and walk through first-run setup
    Setup {
        /// Skip confirmation prompts and accept defaults
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
    /// Find out why something does not work: tools, ports, dotenv files, the gateway, stands, sessions
    Doctor {
        /// Check a server instead: sshd, the sign-in, deploy directories, disk space, the restart, the web server
        #[arg(short, long, value_name = "NAME", num_args = 0..=1, conflicts_with = "fix")]
        server: Option<Option<String>>,
        /// Repair what turnout can: dotenv files, .gitignore, stale records - and end a process on a dev port after a yes
        #[arg(long)]
        fix: bool,
        /// With --fix: end such a process without asking
        #[arg(short = 'y', long = "yes", requires = "fix")]
        assume_yes: bool,
        #[command(flatten)]
        format: Format,
    },
    /// Show what turnout knows: apps and where they point, the gateway, jobs
    Status {
        #[command(flatten)]
        format: Format,
        /// One line for a shell prompt or a status bar: `prod · gw ok · 2 dev`
        #[arg(short, long, conflicts_with = "json")]
        short: bool,
    },
    /// Manage apps - the local projects turnout works with
    App {
        #[command(subcommand)]
        command: AppCommand,
    },
    /// Manage servers - the stands turnout routes to and deploys on
    Server {
        #[command(subcommand)]
        command: ServerCommand,
    },
    /// Manage credentials - who logs in, and with a password, a key or the SSH agent
    #[command(alias = "cred")]
    Credential {
        #[command(subcommand)]
        command: CredentialCommand,
    },
    /// Manage paths - named remote directories to deploy into
    Path {
        #[command(subcommand)]
        command: PathCommand,
    },
    /// Manage targets - a named deploy route: app, server, credential and path
    Target {
        #[command(subcommand)]
        command: TargetCommand,
    },
    /// Manage the secrets credentials use, stored in the OS keyring
    Pass {
        #[command(subcommand)]
        command: PassCommand,
    },
    /// Set up SSH key access to a server, so the password is needed once
    Key {
        #[command(subcommand)]
        command: KeyCommand,
    },
    /// Bind an app (or a whole group) to a server - the daily switch command
    Use {
        /// App or group name, or `-` to undo the last switch; picked interactively when omitted
        app: Option<String>,
        /// Target server, or `-` for the one it used before; picked interactively when omitted
        server: Option<String>,
        /// Skip the stand health check
        #[arg(short = 'n', long)]
        no_check: bool,
    },
    /// Manage app groups - switch a whole contour with one `use`
    Group {
        #[command(subcommand)]
        command: GroupCommand,
    },
    /// Run the local dev gateway that routes apps to their bound servers
    Gateway {
        #[command(subcommand)]
        command: GatewayCommand,
    },
    /// Hand the gateway a stand's session cookies - for stands you can only log in to in a browser
    Cookie {
        #[command(subcommand)]
        command: CookieCommand,
    },
    /// Run the app's `dev` command (app resolved from the current directory if omitted)
    Dev {
        app: Option<String>,
        #[command(flatten)]
        console: Console,
        /// Open the app's front door in the browser once the server is up
        #[arg(short, long)]
        open: bool,
    },
    /// Open a target's stand in the browser, or an app at http://NAME.localhost (through the gateway's front door)
    Open {
        /// Target or app name; the app of the current directory when omitted
        name: Option<String>,
    },
    /// Run the app's `build` command
    Build {
        app: Option<String>,
        #[command(flatten)]
        console: Console,
    },
    /// Run the app's `test` command
    Test {
        app: Option<String>,
        #[command(flatten)]
        console: Console,
    },
    /// Run the app's `lint` command
    Lint {
        app: Option<String>,
        #[command(flatten)]
        console: Console,
    },
    /// Run any named command from the app config
    Run {
        command: String,
        app: Option<String>,
        #[command(flatten)]
        console: Console,
        /// Open the app's front door in the browser once the command reports a server
        #[arg(short, long)]
        open: bool,
    },
    /// Show the jobs turnout runs: dev servers, builds, deploys and the gateway
    Ps {
        /// Keep the table on screen and refresh it every second
        #[arg(short, long, conflicts_with = "json")]
        watch: bool,
        #[command(flatten)]
        format: Format,
    },
    /// Print a job's output (the app of the current directory when omitted)
    Logs {
        /// App name, or `gateway`
        name: Option<String>,
        /// Which of the app's commands (defaults to its running job, else its latest)
        command: Option<String>,
        /// Keep printing as the job writes, until it ends
        #[arg(short, long)]
        follow: bool,
        /// Only the last N lines
        #[arg(short = 'n', long, value_name = "N")]
        lines: Option<usize>,
        /// The output of the last failure, kept even after the job ran again
        #[arg(long, conflicts_with = "follow")]
        failed: bool,
    },
    /// Stop a job: every running job of an app, one command of it, or the gateway
    Stop {
        /// App name, or `gateway`
        name: Option<String>,
        /// Only this command of the app
        command: Option<String>,
    },
    /// Run a detached job and keep its record (internal, what `--detach` spawns)
    #[command(name = "job-run", hide = true)]
    JobRun {
        #[arg(long)]
        app: String,
        #[arg(long)]
        command: String,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        label: String,
        /// Watch for the server to come up
        #[arg(long)]
        ready: bool,
        /// The address to report when the server names none
        #[arg(long)]
        own: Option<String>,
        /// Open the front door once the server is up
        #[arg(long)]
        open: bool,
        /// The program is turnout itself, with these arguments
        #[arg(long = "self")]
        itself: bool,
        /// Where the notification of a success leads (a deploy's stand)
        #[arg(long)]
        link: Option<String>,
        /// The command line, or turnout's arguments with `--self`
        #[arg(last = true, required = true, num_args = 1..)]
        program: Vec<String>,
    },
    /// Open an SSH session on a server - host, port, user and key from the catalogs
    Ssh {
        /// Target, server or app name; the app of the current directory when omitted
        name: Option<String>,
        /// Credential to log in with (defaults to the target's or the server's)
        #[arg(short = 'C', long)]
        credential: Option<String>,
    },
    /// Run a command on a server, in the target's deploy directory
    Exec {
        /// Target, server or app name; the app of the current directory when omitted
        name: Option<String>,
        /// Credential to log in with (defaults to the target's or the server's)
        #[arg(short = 'C', long)]
        credential: Option<String>,
        /// Directory to run in (defaults to the target's deploy directory)
        #[arg(short, long)]
        dir: Option<String>,
        /// The command, after `--`
        #[arg(last = true, required = true, num_args = 1..)]
        command: Vec<String>,
    },
    /// Set up deployment of an app to a server in one wizard
    #[command(name = "deploy-setup", alias = "setup-deploy")]
    DeploySetup {
        /// App name; picked interactively when omitted
        app: Option<String>,
        /// Target server; picked interactively when omitted
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Deploy to a target over SSH/SFTP: build, upload, restart, check
    Deploy {
        /// Target names, or app names to use their target on the bound server; one build per app, deployed in order
        targets: Vec<String>,
        /// Target server (defaults to the app's current binding)
        #[arg(short, long)]
        server: Option<String>,
        /// Credential to log in with (defaults to the server's)
        #[arg(short = 'C', long)]
        credential: Option<String>,
        /// Named path to deploy into (defaults to the server's for this app)
        #[arg(short = 'p', long)]
        path: Option<String>,
        /// Skip the build step
        #[arg(short = 'n', long)]
        no_build: bool,
        /// Back up the remote directory before touching it
        #[arg(short, long)]
        backup: bool,
        /// Clear the remote directory before uploading
        #[arg(short, long)]
        clear: bool,
        /// Upload file by file instead of packing the artifacts into one archive
        #[arg(short = 'A', long)]
        no_archive: bool,
        /// Show what the deploy would change on the server - and write nothing
        #[arg(long, conflicts_with_all = ["detach", "open"])]
        diff: bool,
        /// With --diff: print the comparison as JSON
        #[arg(long, requires = "diff")]
        json: bool,
        /// Open the stand in the browser once the deploy is through
        #[arg(short, long, conflicts_with = "detach")]
        open: bool,
        #[command(flatten)]
        console: Console,
    },
    /// Back up a target's deploy directory on the server; older backups beyond what the target keeps are removed
    #[command(args_conflicts_with_subcommands = true)]
    Backup {
        #[command(subcommand)]
        command: Option<BackupCommand>,
        /// Target name, or an app name to use its target on the bound server
        target: Option<String>,
        /// Target server (defaults to the app's current binding)
        #[arg(short, long)]
        server: Option<String>,
        /// Credential to log in with (defaults to the server's)
        #[arg(short = 'C', long)]
        credential: Option<String>,
        /// Named path to back up (defaults to the server's for this app)
        #[arg(short = 'p', long)]
        path: Option<String>,
    },
    /// Restore a target's deploy directory from a backup
    Restore {
        /// Target name, or an app name to use its target on the bound server
        target: Option<String>,
        /// Target server (defaults to the app's current binding)
        #[arg(short, long)]
        server: Option<String>,
        /// Credential to log in with (defaults to the server's)
        #[arg(short = 'C', long)]
        credential: Option<String>,
        /// Named path to restore into (defaults to the server's for this app)
        #[arg(short = 'p', long)]
        path: Option<String>,
        /// Backup archive name (defaults to the newest)
        #[arg(short, long)]
        from: Option<String>,
        /// List available backups and exit
        #[arg(short, long)]
        list: bool,
        /// With --list: print the backups as JSON
        #[arg(long, requires = "list")]
        json: bool,
    },
    /// Show the deploys made from this machine: when, where, which build, and how each ended
    History {
        /// Only the deploys of this target or app
        #[arg(conflicts_with_all = ["show", "diff"])]
        name: Option<String>,
        /// How many of the newest deploys to list
        #[arg(short = 'n', long, value_name = "N", default_value_t = 20)]
        limit: usize,
        /// One deploy in full: its build, check and the restart's output
        #[arg(long, value_name = "DEPLOY", conflicts_with = "diff")]
        show: Option<String>,
        /// The commits between two deploys of one app
        #[arg(long, num_args = 2, value_names = ["FROM", "TO"])]
        diff: Option<Vec<String>>,
        #[command(flatten)]
        format: Format,
    },
    /// Put a stand back on the build of an earlier deploy - the one before what it runs, or the one named
    Rollback {
        /// Target name, or an app name to use its target on the bound server; with --to, the deploy names it
        target: Option<String>,
        /// The deploy whose build to send again, as `turnout history` numbers it
        #[arg(long, value_name = "DEPLOY")]
        to: Option<String>,
    },
    /// Print a shell completion script (source it from your shell profile)
    Completions {
        /// Target shell
        shell: clap_complete::Shell,
    },
    /// List entity names for shell completion (internal)
    #[command(hide = true)]
    Complete {
        /// What to list
        what: CompleteKind,
    },
    /// Write the catalogs to a file for another machine
    Export {
        /// Where to write; defaults to turnout-export.json in the current directory
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
        /// Include stored secrets, encrypted with a passphrase you choose
        #[arg(short = 'S', long)]
        with_secrets: bool,
    },
    /// Read a file written by `turnout export`
    Import {
        /// The export file to read
        file: std::path::PathBuf,
        /// Overwrite entries that already exist instead of keeping them
        #[arg(short, long)]
        force: bool,
    },
    /// Update turnout itself to the latest release
    SelfUpdate {
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
        /// Replace the binary even when a package manager owns it
        #[arg(short, long)]
        force: bool,
    },
    /// Refresh the cached latest release (internal, run in the background)
    #[command(hide = true)]
    CheckUpdate,
}

#[derive(Subcommand)]
pub enum BackupCommand {
    /// Remove all but the newest backups of a target's deploy directory
    Prune {
        /// Target name, or an app name to use its target on the bound server
        target: Option<String>,
        /// Target server (defaults to the app's current binding)
        #[arg(short, long)]
        server: Option<String>,
        /// Credential to log in with (defaults to the server's)
        #[arg(short = 'C', long)]
        credential: Option<String>,
        /// Named path whose backups to prune (defaults to the server's for this app)
        #[arg(short = 'p', long)]
        path: Option<String>,
        /// How many to keep (defaults to the target's --keep-backups, else 5); 0 removes them all
        #[arg(short, long, value_name = "N")]
        keep: Option<u32>,
        #[command(flatten)]
        format: Format,
    },
}

/// Entity kinds the completion helper can list.
#[derive(Clone, Copy, clap::ValueEnum)]
pub enum CompleteKind {
    Apps,
    Servers,
    Credentials,
    Paths,
    /// Named deploy targets - what `deploy` accepts
    Targets,
    Groups,
    /// Apps and groups together - what `use` accepts first
    Bindable,
    /// Targets and apps together - what `open` accepts
    Openable,
    /// Command names defined across all apps - what `run` accepts
    Commands,
}

#[derive(Subcommand)]
pub enum GroupCommand {
    /// Create a group; apps are picked interactively when not passed
    Add {
        /// Group name (lowercase letters, digits, dashes)
        name: Option<String>,
        /// App to include (repeatable)
        #[arg(short = 'a', long = "app", value_name = "APP")]
        apps: Vec<String>,
    },
    /// List groups
    List {
        #[command(flatten)]
        format: Format,
    },
    /// Show a group and where its apps point
    Show {
        /// Group name; picked interactively when omitted
        name: Option<String>,
        #[command(flatten)]
        format: Format,
    },
    /// Add or remove apps in a group
    Edit {
        /// Group name; picked interactively when omitted
        name: Option<String>,
        /// Add an app (repeatable)
        #[arg(short = 'a', long = "add-app", value_name = "APP")]
        add_apps: Vec<String>,
        /// Remove an app (repeatable)
        #[arg(short = 'r', long = "rm-app", value_name = "APP")]
        rm_apps: Vec<String>,
    },
    /// Remove a group (its apps are untouched)
    Remove {
        /// Group name; picked interactively when omitted
        name: Option<String>,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
}

#[derive(Subcommand)]
pub enum GatewayCommand {
    /// Start the gateway in the background
    Start,
    /// Run the gateway in the foreground (what `start` spawns)
    Run {
        /// Port for the front door; picked (80, then 7000) when omitted
        #[arg(long, hide = true)]
        front_port: Option<u16>,
        /// The log file `start` sends the output to; marks the run as detached
        #[arg(long, hide = true)]
        log: Option<PathBuf>,
    },
    /// Stop the gateway - the same as `turnout stop gateway`
    Stop,
}

#[derive(Subcommand)]
pub enum CookieCommand {
    /// Replace the app's cookies for its stand with ones copied from a browser
    ///
    /// Reads the copied text from standard input when it is piped, otherwise
    /// from the clipboard: the `Cookie` request header, or a whole request
    /// copied as cURL from the browser's developer tools.
    Set {
        /// App name (defaults to the app of the current directory)
        app: Option<String>,
        /// The stand the cookies belong to (defaults to the app's current one)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Show which apps the gateway holds cookies for, by name only
    List {
        #[command(flatten)]
        format: Format,
    },
    /// Forget the app's cookies for its stand - the gateway is logged out there
    Clear {
        /// App name (defaults to the app of the current directory)
        app: Option<String>,
        /// The stand to forget (defaults to the app's current one)
        #[arg(short, long)]
        server: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum PassCommand {
    /// Save or replace a credential's secret in the OS keyring
    Set {
        /// Credential name; picked interactively when omitted
        credential: Option<String>,
    },
    /// Copy the secret (or the user) to the clipboard
    Copy {
        /// Credential name; picked interactively when omitted
        credential: Option<String>,
        /// Copy the user name instead of the secret
        #[arg(short, long)]
        user: bool,
        /// Print to stdout instead of copying to the clipboard
        #[arg(short, long)]
        show: bool,
    },
    /// List which credentials have a secret stored (never prints secrets)
    List {
        #[command(flatten)]
        format: Format,
    },
    /// Remove a credential's stored secret; the credential itself stays
    Remove {
        /// Credential name; picked interactively when omitted
        credential: Option<String>,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
}

#[derive(Subcommand)]
pub enum KeyCommand {
    /// Generate or reuse a key, authorize it on the server, and switch the
    /// credential to it
    Setup {
        /// Server name; picked interactively when omitted
        server: Option<String>,
        /// Credential to give key access (defaults to the server's)
        #[arg(short, long)]
        credential: Option<String>,
        /// Existing private key file to authorize instead of generating one
        #[arg(short = 'K', long, value_name = "PATH")]
        key: Option<String>,
    },
    /// Check that a credential's key signs in to a server
    Check {
        /// Server name; picked interactively when omitted
        server: Option<String>,
        /// Credential to check (defaults to the server's)
        #[arg(short, long)]
        credential: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum CredentialCommand {
    /// Add a credential; missing details are asked interactively
    Add {
        /// Credential name (lowercase letters, digits, dashes)
        name: Option<String>,
        /// Remote user this logs in as
        #[arg(short, long)]
        user: Option<String>,
        /// How it authenticates: password, key or agent
        #[arg(short, long)]
        auth: Option<String>,
        /// Private key file (implies --auth key)
        #[arg(short = 'K', long, value_name = "PATH")]
        key: Option<String>,
    },
    /// List credentials
    List {
        #[command(flatten)]
        format: Format,
    },
    /// Show one credential in detail (never prints secrets)
    Show {
        /// Credential name; picked interactively when omitted
        name: Option<String>,
        #[command(flatten)]
        format: Format,
    },
    /// Edit a credential; with no flags an interactive wizard walks the fields
    Edit {
        /// Credential name; picked interactively when omitted
        name: Option<String>,
        #[arg(short, long)]
        user: Option<String>,
        /// How it authenticates: password, key or agent
        #[arg(short, long)]
        auth: Option<String>,
        /// Private key file (empty value removes it)
        #[arg(short = 'K', long, value_name = "PATH")]
        key: Option<String>,
    },
    /// Remove a credential and its stored secret
    Remove {
        /// Credential name; picked interactively when omitted
        name: Option<String>,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
}

#[derive(Subcommand)]
pub enum PathCommand {
    /// Add a path; missing details are asked interactively
    Add {
        /// Path name (lowercase letters, digits, dashes)
        name: Option<String>,
        /// Absolute directory on the server
        #[arg(short, long)]
        dir: Option<String>,
        /// Command to run on the server after writing here
        #[arg(short, long)]
        restart: Option<String>,
    },
    /// List paths
    List {
        #[command(flatten)]
        format: Format,
    },
    /// Show one path in detail
    Show {
        /// Path name; picked interactively when omitted
        name: Option<String>,
        #[command(flatten)]
        format: Format,
    },
    /// Edit a path; with no flags an interactive wizard walks the fields
    Edit {
        /// Path name; picked interactively when omitted
        name: Option<String>,
        #[arg(short, long)]
        dir: Option<String>,
        /// Post-write command (empty value removes it)
        #[arg(short, long)]
        restart: Option<String>,
    },
    /// Remove a path; servers that used it are updated
    Remove {
        /// Path name; picked interactively when omitted
        name: Option<String>,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
}

/// How a target proves a deploy worked: one probe, and how long to ask it.
#[derive(Args, Clone, Default)]
pub struct HealthArgs {
    /// After a deploy, GET this - a path under the target's address or a full URL; no answer below 400 rolls the deploy back. Empty clears the check
    #[arg(long, value_name = "URL", conflicts_with = "health_command")]
    pub health: Option<String>,
    /// After a deploy, run this on the server in the deploy directory; a non-zero exit rolls the deploy back. Empty clears the check
    #[arg(long, value_name = "CMD")]
    pub health_command: Option<String>,
    /// How long the health check keeps asking, in seconds (default 60)
    #[arg(long, value_name = "SECS")]
    pub health_wait: Option<u64>,
}

impl HealthArgs {
    pub fn any(&self) -> bool {
        self.health.is_some() || self.health_command.is_some() || self.health_wait.is_some()
    }
}

#[derive(Subcommand)]
pub enum TargetCommand {
    /// Add a target; missing parts are picked interactively
    Add {
        /// Target name (defaults to APP-SERVER)
        name: Option<String>,
        /// The app whose artifacts travel
        #[arg(short, long)]
        app: Option<String>,
        /// The server they land on
        #[arg(short, long)]
        server: Option<String>,
        /// The credential that logs in (defaults to the server's)
        #[arg(short = 'C', long)]
        credential: Option<String>,
        /// The named path they are written to
        #[arg(short, long)]
        path: Option<String>,
        /// Where the deployed app answers, when that is not the server's URL
        #[arg(short, long)]
        url: Option<String>,
        #[command(flatten)]
        health: HealthArgs,
        /// How many backups of its deploy directory the stand keeps (default 5)
        #[arg(long, value_name = "N")]
        keep_backups: Option<u32>,
    },
    /// List targets
    List {
        #[command(flatten)]
        format: Format,
    },
    /// Show one target in detail
    Show {
        /// Target name; picked interactively when omitted
        name: Option<String>,
        #[command(flatten)]
        format: Format,
    },
    /// Edit a target; with no flags an interactive wizard walks the fields
    Edit {
        /// Target name; picked interactively when omitted
        name: Option<String>,
        #[arg(short, long)]
        server: Option<String>,
        #[arg(short = 'C', long)]
        credential: Option<String>,
        #[arg(short, long)]
        path: Option<String>,
        /// Where the deployed app answers, when that is not the server's URL; empty clears it
        #[arg(short, long)]
        url: Option<String>,
        #[command(flatten)]
        health: HealthArgs,
        /// How many backups of its deploy directory the stand keeps (default 5)
        #[arg(long, value_name = "N")]
        keep_backups: Option<u32>,
    },
    /// Rename a target
    Rename {
        /// Target name; picked interactively when omitted
        name: Option<String>,
        /// The new name
        to: Option<String>,
    },
    /// Remove a target (the entities it names are untouched)
    Remove {
        /// Target name; picked interactively when omitted
        name: Option<String>,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
}

#[derive(Subcommand)]
pub enum AppCommand {
    /// Add an app; missing details are asked interactively
    Add {
        /// App name (lowercase letters, digits, dashes)
        name: Option<String>,
        /// Project directory
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Pin the app's gateway port (default: assigned from 7100-7199 when the app is added)
        #[arg(short = 'P', long)]
        port: Option<u16>,
        /// Environment variable that carries the gateway URL to the app's commands (default: TURNOUT_GATEWAY_URL)
        #[arg(short = 'e', long = "env-var", value_name = "NAME")]
        env_var: Option<String>,
        /// Dotenv file turnout keeps in step with the port, relative to the project (default: .env.development.local)
        #[arg(long = "env-file", value_name = "FILE")]
        env_file: Option<String>,
        /// Port the dev server listens on (default: assigned from 5100-5199 on the first `dev`)
        #[arg(long = "dev-port", value_name = "PORT")]
        dev_port: Option<u16>,
        /// Build artifact directory, relative to the project path
        #[arg(short, long)]
        dist: Option<String>,
        /// Set a command as NAME=CMD (repeatable); overrides detected defaults
        #[arg(short = 'c', long = "command", value_name = "NAME=CMD")]
        commands: Vec<String>,
        /// Allow a server for this app (repeatable)
        #[arg(short = 's', long = "server", value_name = "SERVER")]
        servers: Vec<String>,
    },
    /// List apps
    List {
        #[command(flatten)]
        format: Format,
    },
    /// Show one app in detail
    Show {
        /// App name; picked interactively when omitted
        name: Option<String>,
        #[command(flatten)]
        format: Format,
    },
    /// Edit an app; with no flags an interactive wizard walks the fields
    Edit {
        /// App name; picked interactively when omitted
        name: Option<String>,
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// The app's gateway port; 0 leaves the app without one
        #[arg(short = 'P', long)]
        port: Option<u16>,
        /// Environment variable that carries the gateway URL to the app's commands
        #[arg(short = 'e', long = "env-var", value_name = "NAME")]
        env_var: Option<String>,
        /// Dotenv file turnout keeps in step with the port, relative to the project
        #[arg(long = "env-file", value_name = "FILE")]
        env_file: Option<String>,
        /// Port the dev server listens on; 0 lets the next `dev` assign one
        #[arg(long = "dev-port", value_name = "PORT")]
        dev_port: Option<u16>,
        #[arg(short, long)]
        dist: Option<String>,
        /// Set a command as NAME=CMD, or NAME= to remove it (repeatable)
        #[arg(short = 'c', long = "command", value_name = "NAME=CMD")]
        commands: Vec<String>,
        /// Allow a server (repeatable)
        #[arg(short = 'a', long = "add-server", value_name = "SERVER")]
        add_servers: Vec<String>,
        /// Disallow a server (repeatable)
        #[arg(short = 'r', long = "rm-server", value_name = "SERVER")]
        rm_servers: Vec<String>,
    },
    /// Remove an app from the catalog (the project on disk is not touched)
    Remove {
        /// App name; picked interactively when omitted
        name: Option<String>,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
}

#[derive(Subcommand)]
pub enum ServerCommand {
    /// Add a server; missing details are asked interactively
    Add {
        /// Server name (lowercase letters, digits, dashes)
        name: Option<String>,
        /// Base URL, e.g. https://staging.example.com
        #[arg(short, long)]
        url: Option<String>,
        /// Human-friendly label
        #[arg(short, long)]
        label: Option<String>,
        /// SSH host and port when they differ from the URL's host
        #[arg(short = 'H', long, value_name = "HOST[:PORT]")]
        host: Option<String>,
        /// Credential used to log in here
        #[arg(short, long)]
        credential: Option<String>,
        /// Accept self-signed or invalid TLS certificates for this server
        #[arg(short, long)]
        insecure: bool,
    },
    /// List servers
    List {
        #[command(flatten)]
        format: Format,
    },
    /// Show one server in detail
    Show {
        /// Server name; picked interactively when omitted
        name: Option<String>,
        #[command(flatten)]
        format: Format,
    },
    /// Edit a server; with no flags an interactive wizard walks the fields
    Edit {
        /// Server name; picked interactively when omitted
        name: Option<String>,
        #[arg(short, long)]
        url: Option<String>,
        #[arg(short, long)]
        label: Option<String>,
        /// SSH host and port (empty value falls back to the URL's host)
        #[arg(short = 'H', long, value_name = "HOST[:PORT]")]
        host: Option<String>,
        /// Credential used to log in here (empty value unsets it)
        #[arg(short, long)]
        credential: Option<String>,
        /// Accept invalid TLS certificates for this server
        #[arg(short, long, conflicts_with = "secure")]
        insecure: bool,
        /// Require valid TLS certificates for this server
        #[arg(short = 'S', long)]
        secure: bool,
    },
    /// Remove a server from the catalog; apps that allowed it are updated
    Remove {
        /// Server name; picked interactively when omitted
        name: Option<String>,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
}
