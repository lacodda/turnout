//! The system clipboard, and the file that stands in for it under test.
//!
//! Three commands touch the clipboard: `pass copy` and `ssh` put a secret on
//! it, `cookie set` reads a header copied from a browser. A test of any of
//! them used to write to the real clipboard of whoever ran the suite - a
//! password over whatever they had copied - and could only run where a
//! clipboard exists at all, which left Linux runners untested.

use anyhow::{Context, Result};

/// Undocumented in the CLI, like `TURNOUT_TOASTS` and `TURNOUT_OPENED`: a file
/// that is the clipboard. `set` writes it, `get` reads it.
pub const ENV: &str = "TURNOUT_CLIPBOARD";

fn sink() -> Option<std::path::PathBuf> {
    std::env::var_os(ENV).filter(|path| !path.is_empty()).map(Into::into)
}

/// Put `text` on the clipboard.
pub fn set(text: &str) -> Result<()> {
    if let Some(path) = sink() {
        return std::fs::write(&path, text).with_context(|| format!("cannot write {}", path.display()));
    }
    arboard::Clipboard::new().and_then(|mut clipboard| clipboard.set_text(text))?;
    Ok(())
}

/// What is on the clipboard now, as text.
pub fn get() -> Result<String> {
    if let Some(path) = sink() {
        return std::fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()));
    }
    Ok(arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text())?)
}
