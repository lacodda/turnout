use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Point local apps at any backend stand, keep servers and secrets at hand,
/// build and deploy - from any directory.
#[derive(Parser)]
#[command(name = "turnout", version, about, propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize the data directory and walk through first-run setup
    Setup {
        /// Skip confirmation prompts and accept defaults
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
    /// Show what turnout knows: apps, servers, bindings, gateway state
    Status,
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
    /// Manage credentials - who logs in, and with a key or a password
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
        /// App or group name; picked interactively when omitted
        app: Option<String>,
        /// Target server; picked interactively when omitted
        server: Option<String>,
        /// Skip the stand reachability check
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
    /// Run the app's `dev` command (app resolved from the current directory if omitted)
    Dev { app: Option<String> },
    /// Run the app's `build` command
    Build { app: Option<String> },
    /// Run the app's `test` command
    Test { app: Option<String> },
    /// Run the app's `lint` command
    Lint { app: Option<String> },
    /// Run any named command from the app config
    Run { command: String, app: Option<String> },
    /// Set up deployment of an app to a server in one wizard
    #[command(name = "deploy-setup", alias = "setup-deploy")]
    DeploySetup {
        /// App name; picked interactively when omitted
        app: Option<String>,
        /// Target server; picked interactively when omitted
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Deploy to a target over SSH/SFTP: build, upload, restart
    Deploy {
        /// Target name, or an app name to use its target on the bound server
        target: Option<String>,
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
    },
    /// Back up a target's deploy directory on the server
    Backup {
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
    List,
    /// Show a group and where its apps point
    Show {
        /// Group name; picked interactively when omitted
        name: Option<String>,
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
    Run,
    /// Stop the background gateway
    Stop,
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
    List,
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
    List,
    /// Show one credential in detail (never prints secrets)
    Show {
        /// Credential name; picked interactively when omitted
        name: Option<String>,
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
    List,
    /// Show one path in detail
    Show {
        /// Path name; picked interactively when omitted
        name: Option<String>,
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
    },
    /// List targets
    List,
    /// Show one target in detail
    Show {
        /// Target name; picked interactively when omitted
        name: Option<String>,
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
        /// Local gateway port for this app
        #[arg(short = 'P', long)]
        port: Option<u16>,
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
    List,
    /// Show one app in detail
    Show {
        /// App name; picked interactively when omitted
        name: Option<String>,
    },
    /// Edit an app; with no flags an interactive wizard walks the fields
    Edit {
        /// App name; picked interactively when omitted
        name: Option<String>,
        #[arg(short, long)]
        path: Option<PathBuf>,
        #[arg(short = 'P', long)]
        port: Option<u16>,
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
    List,
    /// Show one server in detail
    Show {
        /// Server name; picked interactively when omitted
        name: Option<String>,
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
