//! `--with`: variables for one run, taken from another store.
//!
//! `turnout env` keeps what an app needs on a stand. Some secrets are not the
//! app's to keep - a personal token, a key shared across projects - and live
//! in a password manager already. `--with sefy:NAME` takes them from there
//! for the one run, and nothing is copied into turnout's keyring.
//!
//! The only source is [sefy](https://github.com/lacodda/sefy), read through
//! its own command line (`sefy get NAME --stdout`): sefy unlocks its vault
//! the way it always does - its agent, or a question on the terminal - and
//! knows nothing of turnout. The `sefy:` in front names the source, so that
//! another one would be another word and not another flag.

use std::process::{Command, Stdio};

use anyhow::{Result, bail};

use crate::envset;

/// Names the sefy program when it is not on the PATH.
pub const SEFY_ENV: &str = "TURNOUT_SEFY";

/// One `--with` as typed.
#[derive(Debug, PartialEq, Eq)]
pub struct With {
    /// The variable the value goes to; `None` reads the record as
    /// `NAME=value` lines, each its own variable.
    variable: Option<String>,
    /// The record, as sefy finds it: an id, a title, or text to search for.
    reference: String,
    /// A field other than the record's own secret.
    field: Option<String>,
}

impl With {
    /// `sefy:NAME`, `sefy:NAME#FIELD`, `VAR=sefy:NAME`, `VAR=sefy:NAME#FIELD`.
    pub fn parse(spec: &str) -> Result<Self> {
        let shape = || anyhow::anyhow!("--with takes `sefy:NAME` or `VAR=sefy:NAME[#FIELD]`, not '{spec}'");
        // The variable is what stands before a `=` that comes before the
        // source's own colon: a record's title may hold either character.
        let (variable, source) = match spec.split_once('=') {
            Some((variable, source)) if !variable.contains(':') => (Some(variable), source),
            _ => (None, spec),
        };
        if let Some(variable) = variable {
            envset::validate_name(variable)?;
        }
        let record = source.strip_prefix("sefy:").ok_or_else(shape)?;
        let (reference, field) = match record.rsplit_once('#') {
            Some((reference, field)) if !field.is_empty() => (reference, Some(field)),
            _ => (record, None),
        };
        if reference.trim().is_empty() {
            return Err(shape());
        }
        Ok(Self {
            variable: variable.map(str::to_string),
            reference: reference.to_string(),
            field: field.map(str::to_string),
        })
    }

    /// How the record is named back in a message.
    fn describe(&self) -> String {
        match &self.field {
            Some(field) => format!("sefy:{}#{field}", self.reference),
            None => format!("sefy:{}", self.reference),
        }
    }

    /// Ask sefy for the record and turn it into variables.
    pub fn fetch(&self) -> Result<Vec<(String, String)>> {
        let secret = self.get()?;
        match &self.variable {
            Some(variable) => Ok(vec![(variable.clone(), secret)]),
            None => self.lines(&secret),
        }
    }

    /// A record read whole: `NAME=value` lines.
    fn lines(&self, secret: &str) -> Result<Vec<(String, String)>> {
        let advice = || {
            format!(
                "{0} is not a record of NAME=value lines - name the variable its value goes to: `--with VAR={0}`",
                self.describe()
            )
        };
        // Why it does not parse is not said: the reason would quote a piece
        // of the secret.
        match envset::parse(secret) {
            Ok(pairs) if !pairs.is_empty() => Ok(pairs),
            _ => bail!("{}", advice()),
        }
    }

    /// The record's secret, as `sefy get --stdout` prints it.
    ///
    /// The terminal stays sefy's: when its vault is locked it asks for the
    /// master password there, and says there what it could not find.
    fn get(&self) -> Result<String> {
        let program = std::env::var(SEFY_ENV)
            .ok()
            .filter(|path| !path.trim().is_empty())
            .unwrap_or_else(|| "sefy".to_string());
        let mut command = Command::new(&program);
        command.args(["get", &self.reference, "--stdout"]);
        if let Some(field) = &self.field {
            command.args(["--field", field]);
        }
        let output = command
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .stdout(Stdio::piped())
            .output()
            .map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => anyhow::anyhow!(
                    "--with {} needs sefy, and there is no `{program}` to run - install it (https://github.com/lacodda/sefy), or name the program in {SEFY_ENV}",
                    self.describe()
                ),
                _ => anyhow::Error::new(err).context(format!("cannot run `{program}`")),
            })?;
        if !output.status.success() {
            bail!("sefy did not hand over {} ({})", self.describe(), output.status);
        }
        let secret = String::from_utf8(output.stdout).map_err(|_| anyhow::anyhow!("{} is not text", self.describe()))?;
        // The line break is the print's, not the secret's.
        let secret = secret.strip_suffix('\n').unwrap_or(&secret);
        Ok(secret.strip_suffix('\r').unwrap_or(secret).to_string())
    }
}

/// Every `--with` of a command, in the order typed: a later one wins a name.
pub fn fetch_all(specs: &[String]) -> Result<Vec<(String, String)>> {
    // All read before any is asked for: a typo in the third must not cost
    // two unlocks of the vault first.
    let sources = specs.iter().map(|spec| With::parse(spec)).collect::<Result<Vec<_>>>()?;
    let mut pairs = Vec::new();
    for source in sources {
        pairs.extend(source.fetch()?);
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(variable: Option<&str>, reference: &str, field: Option<&str>) -> With {
        With {
            variable: variable.map(str::to_string),
            reference: reference.to_string(),
            field: field.map(str::to_string),
        }
    }

    #[test]
    fn the_four_shapes_are_read() {
        assert_eq!(With::parse("sefy:myapp-dev").unwrap(), with(None, "myapp-dev", None));
        assert_eq!(With::parse("sefy:myapp#notes").unwrap(), with(None, "myapp", Some("notes")));
        assert_eq!(With::parse("GITHUB_TOKEN=sefy:github").unwrap(), with(Some("GITHUB_TOKEN"), "github", None));
        assert_eq!(With::parse("DB_USER=sefy:db#login").unwrap(), with(Some("DB_USER"), "db", Some("login")));
        // A title is free text: spaces, an equals sign, a hash with nothing
        // after it.
        assert_eq!(With::parse("sefy:My App = dev").unwrap(), with(None, "My App = dev", None));
        assert_eq!(With::parse("TOKEN=sefy:issue #").unwrap(), with(Some("TOKEN"), "issue #", None));
    }

    #[test]
    fn anything_else_is_refused_with_the_shape() {
        for bad in ["github", "vault:github", "sefy:", "TOKEN=github", "TOKEN=sefy:", "=sefy:x"] {
            assert!(With::parse(bad).is_err(), "{bad}");
        }
        let err = With::parse("op:github").unwrap_err().to_string();
        assert!(err.contains("sefy:NAME"), "{err}");
        let err = With::parse("MY-VAR=sefy:github").unwrap_err().to_string();
        assert!(err.contains("MY-VAR"), "{err}");
    }

    /// A record without a variable is its lines; one that is a bare secret
    /// says how to give it a name.
    #[test]
    fn a_record_read_whole_is_its_lines() {
        let source = With::parse("sefy:myapp-dev").unwrap();
        assert_eq!(
            source.lines("# the dev stand\nDATABASE_URL=postgres://dev\nAPI_KEY='k $1'\n").unwrap(),
            [
                ("DATABASE_URL".to_string(), "postgres://dev".to_string()),
                ("API_KEY".to_string(), "k $1".to_string())
            ]
        );
        for bare in ["hunter2 with spaces", "", "# only a comment", "hunter-2=x"] {
            let err = format!("{:#}", source.lines(bare).unwrap_err());
            assert!(err.contains("--with VAR=sefy:myapp-dev"), "{bare:?}: {err}");
            assert!(!err.contains("hunter"), "the refusal quotes the secret: {err}");
        }
    }
}
