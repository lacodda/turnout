//! Desktop notifications: how a job nobody is watching says how it went.
//!
//! A job in a terminal speaks there. A job in the background has no terminal,
//! and until v0.21 its result waited in `ps` for somebody to think of looking.
//! Now it says so once, on the desktop - the server is up, the build finished,
//! the deploy went out, or it failed and here is its output.
//!
//! A toast is only worth its interruption if it leads somewhere, so every one
//! carries a place to open: the page the server answers on, the stand a deploy
//! went to, the log a failure left. How far a click can go differs by
//! platform, and the docs keep an honest table of it:
//!
//! * Windows: WinRT directly, with protocol activation. The shell handles the
//!   click and the buttons, so they work long after turnout has exited - from
//!   the notification centre too. turnout registers itself as the sender, so
//!   the toast carries its name and icon and can be switched off on its own.
//! * Linux: D-Bus notifications with actions. A click reaches the process that
//!   showed the toast, so a supervisor stays around for a while after its job
//!   ended, to answer it ([`settle`]).
//! * macOS: shown, not clickable. NSUserNotificationCenter reports a click
//!   only on an application's main run loop, which a command-line tool does not
//!   run. The text names the command that shows the log.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::progress::human_duration;

/// Sends toasts to a file instead of the desktop: one JSON object per line.
///
/// For tests. A test run must not pop real toasts on the developer's desktop,
/// and a CI runner has no desktop to pop them on - yet which toast a job sends
/// and where it leads is exactly the behaviour under test. Undocumented in the
/// CLI, like `TURNOUT_CONSOLE`.
pub const SINK_ENV: &str = "TURNOUT_TOASTS";

/// One notification.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub title: String,
    pub body: String,
    /// A smaller line under the body: how it ended, and the command that
    /// shows more where a click cannot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// What a click on the toast itself opens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub buttons: Vec<Button>,
    /// A failure stays on screen longer than news that all went well.
    pub failure: bool,
}

/// A button: a label and the address it opens.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Button {
    pub label: String,
    pub open: String,
}

impl Button {
    fn new(label: &str, open: impl Into<String>) -> Self {
        Self {
            label: label.to_string(),
            open: open.into(),
        }
    }
}

/// Show a toast - on the desktop, or in the file [`SINK_ENV`] names.
pub fn show(toast: &Toast) -> Result<()> {
    if let Some(path) = std::env::var_os(SINK_ENV).filter(|path| !path.is_empty()) {
        return record(Path::new(&path), toast);
    }
    imp::show(toast)
}

/// Stay until the toasts this process showed no longer need it, within bounds.
///
/// Only Linux has anything to wait for: there the click comes back to this
/// process over D-Bus, and a process that has exited cannot open anything.
pub fn settle() {
    if std::env::var_os(SINK_ENV).is_some_and(|path| !path.is_empty()) {
        return;
    }
    imp::settle();
}

fn record(path: &Path, toast: &Toast) -> Result<()> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("cannot write {}", path.display()))?;
    // One write per toast: two supervisors appending at once must not
    // interleave halves of their lines.
    let line = format!("{}\n", serde_json::to_string(toast)?);
    file.write_all(line.as_bytes()).with_context(|| format!("cannot write {}", path.display()))
}

// --- what a job says -----------------------------------------------------------

/// Where a job's toast can lead: its log, its project folder.
pub struct Places<'a> {
    /// The log to open - for a failure, the copy kept aside.
    pub log: Option<&'a Path>,
    pub dir: &'a Path,
}

impl Places<'_> {
    fn log_url(&self) -> Option<String> {
        self.log.map(file_url)
    }

    fn log_button(&self) -> Option<Button> {
        self.log_url().map(|url| Button::new("Show log", url))
    }

    fn folder_button(&self) -> Button {
        Button::new("Show folder", file_url(self.dir))
    }
}

/// What the toasts call a job: `myapp` for its dev server, `myapp build` for
/// the rest - the same words `ps` uses, minus the `dev` nobody says.
fn subject(app: &str, command: &str) -> String {
    if command == "dev" { app.to_string() } else { format!("{app} {command}") }
}

/// A server came up.
pub fn ready(app: &str, command: &str, address: Option<&str>, elapsed: Duration, places: &Places<'_>) -> Toast {
    let started = format!("started in {}", human_duration(elapsed));
    let mut buttons = Vec::new();
    let (body, open) = match address {
        Some(url) => {
            buttons.push(Button::new("Open in browser", url));
            (format!("{url} - {started}"), Some(url.to_string()))
        }
        None => (started, places.log_url()),
    };
    buttons.extend(places.log_button());
    Toast {
        title: format!("{} is ready", subject(app, command)),
        body,
        note: None,
        open,
        buttons,
        failure: false,
    }
}

/// A job ended well. `link` is where its result can be seen, when it has one:
/// the stand a deploy went to.
pub fn finished(app: &str, command: &str, elapsed: Duration, link: Option<&str>, places: &Places<'_>) -> Toast {
    let took = format!("in {}", human_duration(elapsed));
    let title = if command == "deploy" {
        format!("{app} deployed")
    } else {
        format!("{} finished", subject(app, command))
    };
    match link {
        Some(url) => Toast {
            title,
            body: format!("{url} - {took}"),
            note: None,
            open: Some(url.to_string()),
            buttons: [Some(Button::new("Open in browser", url)), places.log_button()].into_iter().flatten().collect(),
            failure: false,
        },
        None => Toast {
            title,
            body: took,
            note: None,
            open: places.log_url(),
            buttons: places.log_button().into_iter().chain([places.folder_button()]).collect(),
            failure: false,
        },
    }
}

/// A server that had come up ended by itself, cleanly. The news is that it no
/// longer answers; `span` is how long it did.
pub fn stopped(app: &str, command: &str, span: &str, places: &Places<'_>) -> Toast {
    Toast {
        title: format!("{} stopped", subject(app, command)),
        body: format!("after {span}"),
        note: None,
        open: places.log_url(),
        buttons: places.log_button().into_iter().collect(),
        failure: false,
    }
}

/// A job failed - to start, or by its own exit code.
///
/// The body is the line most likely to say why; the note says how it ended and
/// names the command that shows the rest, because on macOS the toast cannot be
/// clicked and the command is the way to the log.
pub fn failed(app: &str, command: &str, how: &str, reason: Option<&str>, places: &Places<'_>) -> Toast {
    let logs = if command == "dev" {
        format!("turnout logs {app} dev --failed")
    } else {
        format!("turnout logs {app} {command} --failed")
    };
    Toast {
        title: format!("{} failed", subject(app, command)),
        body: reason.map_or_else(|| how.to_string(), str::to_string),
        note: Some(format!("{how} - {logs}")),
        open: places.log_url(),
        buttons: places.log_button().into_iter().chain([places.folder_button()]).collect(),
        failure: true,
    }
}

/// How long a toast line may run before it is cut.
const REASON_CHARS: usize = 160;

/// The line of a failed job's output most likely to say why it failed.
///
/// The last line that mentions an error, else the last line with anything on
/// it - leaving out what the package manager adds around a failed script.
/// Compilers and test runners put their verdict last, and a toast has room for
/// one line; the log keeps the rest.
pub fn reason(tail: &[String]) -> Option<String> {
    let lines: Vec<String> = tail
        .iter()
        .map(|line| crate::job::strip_ansi(line).trim().to_string())
        .filter(|line| !line.is_empty())
        .collect();
    let own: Vec<&String> = lines.iter().filter(|line| !is_wrapper(line)).collect();
    let line = own
        .iter()
        .rev()
        .find(|line| line.to_ascii_lowercase().contains("error"))
        .or_else(|| own.last())
        .copied()
        .or_else(|| lines.last())?;
    Some(cut(line, REASON_CHARS))
}

/// Whether a line is the package manager speaking about the script it ran:
/// the exit code, the script's name, where its own log went. True, and never
/// the reason - but npm since v10 starts every such line with `npm error`,
/// which would otherwise outbid the compiler's own error above it.
fn is_wrapper(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "npm err",
        "npm warn",
        "elifecycle",
        "error command failed",
        "info visit",
        "yarn run v",
        "done in",
    ]
    .iter()
    .any(|prefix| lower.starts_with(prefix))
}

fn cut(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(chars.saturating_sub(3)).collect();
    format!("{}...", kept.trim_end())
}

/// A local path as a `file:` URL that an opener takes back to the same path.
///
/// Percent-encoded byte by byte outside the characters a path segment keeps
/// as they are: a log's name carries the `%` of an escaped command
/// (`myapp.test%3Ae2e.log`), and a home directory can hold spaces and
/// non-ASCII letters. Unencoded, the first would be decoded into a different
/// name and the others would make no URL at all.
pub fn file_url(path: &Path) -> String {
    let raw = path.to_string_lossy();
    // The verbatim prefix canonicalization leaves on Windows paths names the
    // same file, and no opener understands it inside a URL.
    let raw = raw
        .strip_prefix(r"\\?\UNC\")
        .map(|share| format!(r"\\{share}"))
        .unwrap_or_else(|| raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string());
    let text = raw.replace('\\', "/");
    let mut url = String::from("file:");
    if text.starts_with("//") {
        // A UNC share: the server is the URL's host.
    } else if text.starts_with('/') {
        url.push_str("//");
    } else {
        url.push_str("///");
    }
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~' | b':') {
            url.push(byte as char);
        } else {
            url.push_str(&format!("%{byte:02X}"));
        }
    }
    url
}

/// The icon a toast shows turnout under, written out of the binary.
///
/// A notification takes an image by path, so the tile compiled into turnout is
/// put in the data directory the first time a toast needs it - and again
/// whenever the embedded one changed, which is what a self-update does. Any
/// failure costs the icon and nothing else.
#[cfg(not(target_os = "macos"))]
fn icon() -> Option<std::path::PathBuf> {
    const ICON: &[u8] = include_bytes!("../assets/toast-icon.png");
    let path = crate::paths::data_dir().ok()?.join("toast-icon.png");
    if std::fs::metadata(&path).map(|meta| meta.len() != ICON.len() as u64).unwrap_or(true) {
        std::fs::write(&path, ICON).ok()?;
    }
    Some(path)
}

/// The toast as Windows reads it: ToastGeneric XML with protocol activation.
///
/// Built as text rather than through the DOM because it is small and fixed,
/// and so it can be checked on every platform the tests run on.
fn windows_xml(toast: &Toast) -> String {
    let mut xml = String::from("<toast");
    if let Some(open) = &toast.open {
        xml.push_str(&format!(r#" activationType="protocol" launch="{}""#, escape_xml(open)));
    }
    if toast.failure {
        xml.push_str(r#" duration="long""#);
    }
    xml.push_str(r#"><visual><binding template="ToastGeneric">"#);
    xml.push_str(&format!("<text>{}</text>", escape_xml(&toast.title)));
    if !toast.body.is_empty() {
        xml.push_str(&format!("<text>{}</text>", escape_xml(&toast.body)));
    }
    if let Some(note) = &toast.note {
        xml.push_str(&format!(r#"<text placement="attribution">{}</text>"#, escape_xml(note)));
    }
    xml.push_str("</binding></visual>");
    if !toast.buttons.is_empty() {
        xml.push_str("<actions>");
        for button in &toast.buttons {
            xml.push_str(&format!(
                r#"<action content="{}" activationType="protocol" arguments="{}"/>"#,
                escape_xml(&button.label),
                escape_xml(&button.open)
            ));
        }
        xml.push_str("</actions>");
    }
    xml.push_str("</toast>");
    xml
}

fn escape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 has no way to carry most control characters at all,
            // escaped or not; a stray one from a tool's output must not cost
            // the whole toast.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(windows)]
mod imp {
    use anyhow::{Context, Result, bail};
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    use windows::core::HSTRING;

    use super::Toast;

    /// Who the toasts come from, as Windows files them.
    const APP_ID: &str = "lacodda.turnout";

    pub fn show(toast: &Toast) -> Result<()> {
        // Before the toast, so the first one already carries turnout's name.
        // A failure here costs the name and the icon, not the toast: it is
        // reported after the toast went out.
        let registered = register();
        let xml = XmlDocument::new().context("cannot prepare the notification")?;
        xml.LoadXml(&HSTRING::from(super::windows_xml(toast)))
            .context("cannot prepare the notification")?;
        let notification = ToastNotification::CreateToastNotification(&xml).context("cannot prepare the notification")?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID)).context("cannot reach the notification centre")?;
        notifier.Show(&notification).context("cannot show the notification")?;
        registered.context("the notification went out without turnout's name and icon")
    }

    pub fn settle() {}

    /// Register turnout as a sender of notifications, for this user.
    ///
    /// An unpackaged program is known to the notification platform by its
    /// application id; `DisplayName` and `IconUri` under this key are what the
    /// toast header and the Windows settings show for it. Without them the toast
    /// appears under a bare id and cannot be told apart from anything else.
    /// Written every time: a few values, and it follows the icon wherever the
    /// data directory went.
    fn register() -> Result<()> {
        use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, RegCloseKey, RegCreateKeyExW};
        let subkey = wide(&format!(r"Software\Classes\AppUserModelId\{APP_ID}"));
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: every pointer is either null where the API allows it, or
        // points at a live, NUL-terminated buffer owned by this frame.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            bail!("cannot register turnout as a notification sender (error {status})");
        }
        let mut result = set_string(key, "DisplayName", "turnout");
        if result.is_ok()
            && let Some(icon) = super::icon()
        {
            result = set_string(key, "IconUri", &icon.to_string_lossy());
        }
        // SAFETY: `key` was opened above and is closed exactly once.
        unsafe { RegCloseKey(key) };
        result
    }

    fn set_string(key: windows_sys::Win32::System::Registry::HKEY, name: &str, value: &str) -> Result<()> {
        use windows_sys::Win32::System::Registry::{REG_SZ, RegSetValueExW};
        let name_w = wide(name);
        let data = wide(value);
        // SAFETY: `key` is open for setting values; both buffers outlive the
        // call and the length counts the terminating NUL, as REG_SZ wants.
        let status = unsafe { RegSetValueExW(key, name_w.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
        if status != 0 {
            bail!("cannot write {name} for the notification sender (error {status})");
        }
        Ok(())
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::sync::Mutex;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    use anyhow::{Context, Result};

    use super::Toast;

    /// How long a supervisor stays after its job, to answer a click.
    ///
    /// Long enough for somebody who stepped away for a coffee, bounded so a
    /// notification nobody touches does not keep a process around for a day.
    const LINGER: Duration = Duration::from_secs(15 * 60);

    /// The threads waiting for a click on a toast this process showed.
    static WAITING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

    pub fn show(toast: &Toast) -> Result<()> {
        let mut notification = notify_rust::Notification::new();
        notification.appname("turnout").summary(&toast.title).body(&body(toast));
        if let Some(icon) = super::icon() {
            notification.icon(&icon.to_string_lossy());
        }
        // Action ids are ours; "default" is the one a click on the body sends.
        let mut targets: Vec<(String, String)> = Vec::new();
        if let Some(open) = &toast.open {
            notification.action("default", "Open");
            targets.push(("default".to_string(), open.clone()));
        }
        for (index, button) in toast.buttons.iter().enumerate() {
            let id = format!("button-{index}");
            notification.action(&id, &button.label);
            targets.push((id, button.open.clone()));
        }
        let handle = notification.show().context("cannot show the notification")?;
        if targets.is_empty() {
            return Ok(());
        }
        let waiter = std::thread::spawn(move || {
            handle.wait_for_action(|action| {
                if let Some((_, url)) = targets.iter().find(|(id, _)| id == action) {
                    let _ = crate::front::open_in_browser(url);
                }
            });
        });
        WAITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).push(waiter);
        Ok(())
    }

    pub fn settle() {
        let deadline = Instant::now() + LINGER;
        loop {
            let done = WAITING
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .all(JoinHandle::is_finished);
            if done || Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    /// The body with the note under it, escaped for the markup most
    /// notification servers read the body as.
    fn body(toast: &Toast) -> String {
        let text = match &toast.note {
            Some(note) => format!("{}\n{note}", toast.body),
            None => toast.body.clone(),
        };
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::{Context, Result};

    use super::Toast;

    pub fn show(toast: &Toast) -> Result<()> {
        let body = match &toast.note {
            Some(note) => format!("{}\n{note}", toast.body),
            None => toast.body.clone(),
        };
        notify_rust::Notification::new()
            .summary(&toast.title)
            .body(&body)
            .show()
            .context("cannot show the notification")?;
        Ok(())
    }

    pub fn settle() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn places(log: Option<&Path>) -> Places<'_> {
        Places {
            log,
            dir: Path::new("/work/myapp"),
        }
    }

    /// A path survives the trip into a URL and back: the `%` of an escaped
    /// command, a space and a non-ASCII home directory all come out encoded,
    /// and a Windows drive keeps its colon.
    #[test]
    fn a_path_becomes_a_url_an_opener_reads_back() {
        assert_eq!(
            file_url(Path::new("/home/me/.local/share/lacodda/turnout/logs/myapp.test%3Ae2e.log")),
            "file:///home/me/.local/share/lacodda/turnout/logs/myapp.test%253Ae2e.log"
        );
        assert_eq!(
            file_url(Path::new(r"C:\Users\Анна Ли\x.log")),
            "file:///C:/Users/%D0%90%D0%BD%D0%BD%D0%B0%20%D0%9B%D0%B8/x.log"
        );
        assert_eq!(file_url(Path::new(r"\\?\C:\work\myapp")), "file:///C:/work/myapp");
        assert_eq!(file_url(Path::new(r"\\?\UNC\server\share\app")), "file://server/share/app");
        assert_eq!(file_url(Path::new("/tmp/a#b?c")), "file:///tmp/a%23b%3Fc");
    }

    /// A server's toast leads to the server; without an address, to its log.
    #[test]
    fn a_ready_toast_leads_to_the_server() {
        let log = PathBuf::from("/logs/myapp.dev.log");
        let toast = ready("myapp", "dev", Some("http://myapp.localhost"), Duration::from_millis(800), &places(Some(&log)));
        assert_eq!(toast.title, "myapp is ready");
        assert_eq!(toast.body, "http://myapp.localhost - started in 0.8s");
        assert_eq!(toast.open.as_deref(), Some("http://myapp.localhost"));
        assert_eq!(toast.buttons[0], Button::new("Open in browser", "http://myapp.localhost"));
        assert_eq!(toast.buttons[1], Button::new("Show log", "file:///logs/myapp.dev.log"));
        assert!(!toast.failure);

        let quiet = ready("myapp", "storybook", None, Duration::from_secs(3), &places(Some(&log)));
        assert_eq!(quiet.title, "myapp storybook is ready");
        assert_eq!(quiet.open.as_deref(), Some("file:///logs/myapp.dev.log"));
    }

    /// A deploy leads to the stand it went to; anything else to what it said.
    #[test]
    fn a_finished_toast_leads_to_the_result() {
        let log = PathBuf::from("/logs/myapp.deploy.log");
        let deployed = finished(
            "myapp",
            "deploy",
            Duration::from_secs(34),
            Some("https://staging.example.com"),
            &places(Some(&log)),
        );
        assert_eq!(deployed.title, "myapp deployed");
        assert_eq!(deployed.body, "https://staging.example.com - in 34s");
        assert_eq!(deployed.open.as_deref(), Some("https://staging.example.com"));

        let built = finished("myapp", "build", Duration::from_secs(72), None, &places(Some(&log)));
        assert_eq!(built.title, "myapp build finished");
        assert_eq!(built.body, "in 1m 12s");
        assert_eq!(built.open.as_deref(), Some("file:///logs/myapp.deploy.log"));
        assert_eq!(built.buttons.last().unwrap(), &Button::new("Show folder", "file:///work/myapp"));
    }

    /// A failure says why in one line, how it ended, and the command that
    /// shows the rest - and opens the log that was kept aside.
    #[test]
    fn a_failure_toast_says_why_and_where() {
        let kept = PathBuf::from("/logs/failed/myapp.build.log");
        let toast = failed("myapp", "build", "exit 2 after 34s", Some("error TS2322: nope"), &places(Some(&kept)));
        assert_eq!(toast.title, "myapp build failed");
        assert_eq!(toast.body, "error TS2322: nope");
        assert_eq!(toast.note.as_deref(), Some("exit 2 after 34s - turnout logs myapp build --failed"));
        assert_eq!(toast.open.as_deref(), Some("file:///logs/failed/myapp.build.log"));
        assert!(toast.failure);
        // Nothing to quote: the ending is the message.
        assert_eq!(failed("myapp", "dev", "exit 1 after 2s", None, &places(None)).body, "exit 1 after 2s");
    }

    /// The reason is the last line that names an error, not merely the last
    /// line - and not the package manager's closing words about the script,
    /// which every failure through npm, yarn or pnpm ends with.
    #[test]
    fn the_reason_is_the_last_line_that_names_an_error() {
        let tail = |lines: &[&str]| lines.iter().map(|line| line.to_string()).collect::<Vec<_>>();
        let compiler = "src/main.ts(3,7): \x1b[31merror\x1b[0m TS2322: Type 'string' is not assignable";
        let expected = Some("src/main.ts(3,7): error TS2322: Type 'string' is not assignable".to_string());
        // npm 10 and later.
        assert_eq!(
            reason(&tail(&[
                compiler,
                "npm error Lifecycle script `build` failed with error:",
                "npm error code 2",
                "npm error A complete log of this run can be found in: x"
            ])),
            expected
        );
        // npm before 10, yarn, pnpm.
        assert_eq!(reason(&tail(&[compiler, "npm ERR! code ELIFECYCLE", "npm ERR! errno 2"])), expected);
        assert_eq!(
            reason(&tail(&[
                "yarn run v1.22.22",
                "$ tsc",
                compiler,
                "error Command failed with exit code 2.",
                "info Visit https://yarnpkg.com/en/docs/cli/run"
            ])),
            expected
        );
        assert_eq!(reason(&tail(&[compiler, " ELIFECYCLE  Command failed with exit code 2."])), expected);
        // Nothing but the wrapper: better its words than none.
        assert_eq!(reason(&tail(&["npm error code 2"])), Some("npm error code 2".to_string()));
        assert_eq!(
            reason(&tail(&["error: expected `;`", "", "  1 problem", "Done in 2s"])),
            Some("error: expected `;`".to_string())
        );
        assert_eq!(reason(&tail(&["compiling", "exit"])), Some("exit".to_string()));
        assert_eq!(reason(&tail(&["", "  "])), None);
        let long = "x".repeat(400);
        assert_eq!(reason(&tail(&[&long])).unwrap().chars().count(), REASON_CHARS);
    }

    /// The toast XML carries the click and the buttons as protocol
    /// activations, and nothing in a job's output can break out of it.
    #[test]
    fn the_windows_toast_is_protocol_activated_and_escaped() {
        let toast = Toast {
            title: "myapp build failed".into(),
            body: "error: <T> & \"x\"\u{1b}[0m".into(),
            note: Some("exit 2".into()),
            open: Some("file:///C:/logs/a%253Ab.log".into()),
            buttons: vec![Button::new("Show folder", "file:///C:/work/myapp")],
            failure: true,
        };
        let xml = windows_xml(&toast);
        assert!(
            xml.starts_with(r#"<toast activationType="protocol" launch="file:///C:/logs/a%253Ab.log" duration="long">"#),
            "{xml}"
        );
        assert!(xml.contains("<text>error: &lt;T&gt; &amp; &quot;x&quot;[0m</text>"), "{xml}");
        assert!(xml.contains(r#"<text placement="attribution">exit 2</text>"#), "{xml}");
        assert!(
            xml.contains(r#"<action content="Show folder" activationType="protocol" arguments="file:///C:/work/myapp"/>"#),
            "{xml}"
        );
        // Good news does not linger, and a toast without buttons has no actions.
        let plain = windows_xml(&Toast {
            buttons: Vec::new(),
            failure: false,
            ..toast
        });
        assert!(!plain.contains("duration") && !plain.contains("<actions>"), "{plain}");
    }
}
