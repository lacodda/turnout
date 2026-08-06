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
    /// Bind an app to a server for development - the daily switch command
    Use {
        app: String,
        server: String,
        /// Skip the stand reachability check
        #[arg(long)]
        no_check: bool,
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
    /// Deploy an app to a server over SSH/SFTP: build, upload, restart
    Deploy {
        app: Option<String>,
        /// Target server (defaults to the app's current binding)
        #[arg(long)]
        server: Option<String>,
        /// Skip the build step
        #[arg(long)]
        no_build: bool,
        /// Clear the remote directory before uploading
        #[arg(long)]
        clear: bool,
    },
    /// Print a shell completion script (source it from your shell profile)
    Completions {
        /// Target shell
        shell: clap_complete::Shell,
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
        #[arg(long, default_value = "password")]
        kind: String,
        #[arg(long)]
        login: Option<String>,
    },
    /// Copy the secret (or login) to the clipboard
    Copy {
        server: String,
        #[arg(long, default_value = "password")]
        kind: String,
        /// Copy the login instead of the secret
        #[arg(long)]
        login: bool,
        /// Print to stdout instead of copying to the clipboard
        #[arg(long)]
        show: bool,
    },
    /// Show access metadata for a server (never prints secrets)
    Show { server: String },
    /// List all stored access metadata
    List,
    /// Remove stored access to a server
    Remove {
        server: String,
        #[arg(long, default_value = "password")]
        kind: String,
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
        #[arg(long)]
        path: Option<PathBuf>,
        /// Local gateway port for this app
        #[arg(long)]
        port: Option<u16>,
        /// Build artifact directory, relative to the project path
        #[arg(long)]
        dist: Option<String>,
        /// Set a command as NAME=CMD (repeatable); overrides detected defaults
        #[arg(long = "command", value_name = "NAME=CMD")]
        commands: Vec<String>,
        /// Allow a server for this app (repeatable)
        #[arg(long = "server", value_name = "SERVER")]
        servers: Vec<String>,
    },
    /// List apps
    List,
    /// Show one app in detail
    Show { name: String },
    /// Edit an app; with no flags an interactive wizard walks the fields
    Edit {
        name: String,
        #[arg(long)]
        path: Option<PathBuf>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        dist: Option<String>,
        /// Set a command as NAME=CMD, or NAME= to remove it (repeatable)
        #[arg(long = "command", value_name = "NAME=CMD")]
        commands: Vec<String>,
        /// Allow a server (repeatable)
        #[arg(long = "add-server", value_name = "SERVER")]
        add_servers: Vec<String>,
        /// Disallow a server (repeatable)
        #[arg(long = "rm-server", value_name = "SERVER")]
        rm_servers: Vec<String>,
    },
    /// Remove an app from the catalog (the project on disk is not touched)
    Remove {
        name: String,
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
        #[arg(long)]
        url: Option<String>,
        /// Human-friendly label
        #[arg(long)]
        label: Option<String>,
        /// SSH access for deploy and remote operations
        #[arg(long, value_name = "USER@HOST[:PORT]")]
        ssh: Option<String>,
        /// Accept self-signed or invalid TLS certificates for this server
        #[arg(long)]
        insecure: bool,
    },
    /// List servers
    List,
    /// Show one server in detail
    Show { name: String },
    /// Edit a server; with no flags an interactive wizard walks the fields
    Edit {
        name: String,
        #[arg(long)]
        url: Option<String>,
        #[arg(long)]
        label: Option<String>,
        #[arg(long, value_name = "USER@HOST[:PORT]")]
        ssh: Option<String>,
        /// Private key file for SSH auth (empty value removes it)
        #[arg(long = "ssh-key", value_name = "PATH")]
        ssh_key: Option<String>,
        /// Accept invalid TLS certificates for this server
        #[arg(long, conflicts_with = "secure")]
        insecure: bool,
        /// Require valid TLS certificates for this server
        #[arg(long)]
        secure: bool,
        /// Set an app's deploy directory as APP=DIR, or APP= to remove (repeatable)
        #[arg(long = "deploy-path", value_name = "APP=DIR")]
        deploy_paths: Vec<String>,
        /// Set an app's post-deploy command as APP=CMD, or APP= to remove (repeatable)
        #[arg(long = "restart-cmd", value_name = "APP=CMD")]
        restart_cmds: Vec<String>,
    },
    /// Remove a server from the catalog; apps that allowed it are updated
    Remove {
        name: String,
        /// Skip the confirmation prompt
        #[arg(short = 'y', long = "yes")]
        assume_yes: bool,
    },
}
