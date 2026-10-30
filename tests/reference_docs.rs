//! Every flag of every command is named on that command's reference page.
//!
//! `lyrn check` holds the README and the site to the command list - a page per
//! command. It does not open the pages: a flag added to a command, or renamed,
//! leaves its page describing the command as it was, and nothing turns red.
//! The flag tables drifted that way more than once before 1.0. Here the
//! program's own `--help` is the list, read the way a user reads it.

use std::collections::BTreeMap;
use std::path::Path;

mod common;

fn help(args: &[&str]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let output = common::turnout(dir.path()).args(args).arg("--help").output().unwrap();
    assert!(output.status.success(), "turnout {} --help failed", args.join(" "));
    String::from_utf8(output.stdout).unwrap()
}

/// The names in a help text's `Commands:` block, `help` left out.
fn commands(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|line| !line.starts_with("Commands:"))
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_string)
        .collect()
}

/// The long flags in a help text's `Options:` block, the two every command
/// has left out.
fn flags(help: &str) -> Vec<String> {
    help.lines()
        .skip_while(|line| !line.starts_with("Options:"))
        .skip(1)
        .take_while(|line| line.is_empty() || line.starts_with(' '))
        .filter_map(|line| {
            let line = line.trim_start();
            let line = if line.starts_with("-") && !line.starts_with("--") {
                line.split_once(", ")?.1
            } else {
                line
            };
            line.strip_prefix("--").map(|rest| {
                let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
                format!("--{name}")
            })
        })
        .filter(|flag| flag != "--help" && flag != "--version")
        .collect()
}

/// Which page documents a command: its own, or the one it shares.
fn page_of(command: &str) -> &str {
    match command {
        "dev" | "build" | "test" | "lint" => "run",
        "restore" => "backup",
        "deploy-setup" => "deploy",
        other => other,
    }
}

#[test]
fn every_flag_is_named_on_its_page() {
    let reference = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/src/content/docs/reference");
    let top = commands(&help(&[]));
    assert!(top.len() > 30, "could not read the commands out of --help: {top:?}");
    // A command per thread: a hundred `--help` runs one after another took
    // half a minute.
    let missing: BTreeMap<String, Vec<String>> = std::thread::scope(|scope| {
        let workers: Vec<_> = top
            .iter()
            .map(|command| {
                let reference = &reference;
                scope.spawn(move || {
                    let page_path = reference.join(format!("{}.md", page_of(command)));
                    let page = std::fs::read_to_string(&page_path).unwrap_or_else(|_| panic!("`turnout {command}` has no page: {}", page_path.display()));
                    let own = help(&[command]);
                    let mut invocations = vec![(command.clone(), own.clone())];
                    for sub in commands(&own) {
                        let text = help(&[command, &sub]);
                        invocations.push((format!("{command} {sub}"), text));
                    }
                    invocations
                        .into_iter()
                        .map(|(invocation, text)| {
                            let absent: Vec<String> = flags(&text).into_iter().filter(|flag| !page.contains(flag)).collect();
                            (format!("{invocation} ({}.md)", page_of(command)), absent)
                        })
                        .filter(|(_, absent)| !absent.is_empty())
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|worker| worker.join().unwrap()).collect()
    });
    assert!(missing.is_empty(), "flags no reference page names:\n{missing:#?}");
}

#[test]
fn the_help_parsers_read_what_clap_prints() {
    let text = "Usage: x\n\nCommands:\n  add   Add one\n  help  Print this\n\nOptions:\n  -u, --url <URL>  The address\n      --dev-port <PORT>\n          Long help\n  -h, --help  Print help\n";
    assert_eq!(commands(text), ["add"]);
    assert_eq!(flags(text), ["--url", "--dev-port"]);
}
