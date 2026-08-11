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
    /// Manage access to servers: logins and secrets in the OS keyring
    Pass {
        #[command(subcommand)]
        command: PassCommand,
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
    /// Deploy an app to a server over SSH/SFTP: build, upload, restart
    Deploy {
        app: Option<String>,
        /// Target server (defaults to the app's current binding)
        #[arg(short, long)]
        server: Option<String>,
        /// Skip the build step
        #[arg(short = 'n', long)]
        no_build: bool,
        /// Back up the remote directory before touching it
        #[arg(short, long)]
        backup: bool,
        /// Clear the remote directory before uploading
        #[arg(short, long)]
        clear: bool,
    },
    /// Back up an app's deploy directory on the server
    Backup {
        app: Option<String>,
        /// Target server (defaults to the app's current binding)
        #[arg(short, long)]
        server: Option<String>,
    },
    /// Restore an app's deploy directory from a backup
    Restore {
        app: Option<String>,
        /// Target server (defaults to the app's current binding)
        #[arg(short, long)]
        server: Option<String>,
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
    /// Write apps, servers and groups to a file for another machine
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
    Groups,
    /// Apps and groups together - what `use` accepts first
    Targets,
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
    /// Save or update access to a server; asks interactively for what's missing
    Set {
        /// Server name from the catalog
        server: Option<String>,
        /// What this access is: password, token, ssh, ...
        #[arg(short, long, default_value = "password")]
        kind: String,
        #[arg(short, long)]
        login: Option<String>,
    },
    /// Copy the secret (or login) to the clipboard
    Copy {
        /// Server name; picked interactively when omitted
        server: Option<String>,
        /// Access kind; when omitted the picker offers every stored kind
        #[arg(short, long)]
        kind: Option<String>,
        /// Copy the login instead of the secret
        #[arg(short, long)]
        login: bool,
        /// Print to stdout instead of copying to the clipboard
        #[arg(short, long)]
        show: bool,
    },
    /// Show access metadata for a server (never prints secrets)
    Show {
        /// Server name; picked interactively when omitted
        server: Option<String>,
    },
    /// List all stored access metadata
    List,
    /// Remove stored access to a server
    Remove {
        /// Server name; picked interactively when omitted
        server: Option<String>,
        /// Access kind; when omitted the picker offers every stored kind
        #[arg(short, long)]
        kind: Option<String>,
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
        /// SSH access for deploy and remote operations
        #[arg(short, long, value_name = "USER@HOST[:PORT]")]
        ssh: Option<String>,
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
        #[arg(short, long, value_name = "USER@HOST[:PORT]")]
        ssh: Option<String>,
        /// Private key file for SSH auth (empty value removes it)
        #[arg(short = 'K', long = "ssh-key", value_name = "PATH")]
        ssh_key: Option<String>,
        /// Accept invalid TLS certificates for this server
        #[arg(short, long, conflicts_with = "secure")]
        insecure: bool,
        /// Require valid TLS certificates for this server
        #[arg(short = 'S', long)]
        secure: bool,
        /// Set an app's deploy directory as APP=DIR, or APP= to remove (repeatable)
        #[arg(short = 'd', long = "deploy-path", value_name = "APP=DIR")]
        deploy_paths: Vec<String>,
        /// Set an app's post-deploy command as APP=CMD, or APP= to remove (repeatable)
        #[arg(short = 'r', long = "restart-cmd", value_name = "APP=CMD")]
        restart_cmds: Vec<String>,
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
