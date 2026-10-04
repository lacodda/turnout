//! What the gateway notices about the stands it talks to.
//!
//! The gateway is the one place that sees every answer a stand gives, and
//! until v0.22 it passed them all on without a word. Two of them are worth
//! saying out loud, because the app in the browser cannot tell them from its
//! own bugs:
//!
//! * **The session expired.** The stand refused the cookies the gateway holds
//!   for it (401). The app shows a login page, or an empty screen, or retries
//!   forever; the fix is to log in again, or `turnout cookie set`.
//! * **The stand is down.** It answered 502, 503 or 504, or did not answer at
//!   all. Nothing the app does locally will help.
//!
//! A signal goes up on the first answer that shows it and stays up until an
//! answer shows the opposite: a successful login for a session, any answer
//! below 500 for a stand. `status` shows the signals that are up, and each one
//! raises a desktop notification once when it goes up - not once per failed
//! request, and not again for the same thing within [`RETOAST`], so a stand
//! that flaps does not bury the desktop.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// What the stand answers when the session the gateway holds is no longer
/// good: 401, and IIS's 440 "login time-out".
const SESSION_GONE: [u16; 2] = [401, 440];

/// What the stand's own proxy answers when the application behind it is not
/// there.
const STAND_DOWN: [u16; 3] = [502, 503, 504];

/// How long a signal that went up and down again stays quiet when it comes
/// back.
pub const RETOAST: u64 = 10 * 60;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    SessionExpired,
    StandDown,
}

/// One thing the gateway has noticed and is still seeing.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Signal {
    pub kind: Kind,
    /// The app whose request showed it. A stand that is down is down for
    /// every app; this is the first one that ran into it.
    pub app: String,
    pub server: String,
    /// The status the stand answered with; `None` when it did not answer.
    pub status: Option<u16>,
    /// Seconds since the epoch, when it was first seen.
    pub since: u64,
}

impl Signal {
    /// What one signal is about: a stand is down for everybody, a session
    /// belongs to one app.
    fn key(&self) -> Key {
        key(self.kind, &self.app, &self.server)
    }
}

type Key = (Kind, String, String);

fn key(kind: Kind, app: &str, server: &str) -> Key {
    match kind {
        Kind::StandDown => (kind, String::new(), server.to_string()),
        Kind::SessionExpired => (kind, app.to_string(), server.to_string()),
    }
}

/// What one exchange with a stand showed.
#[derive(Clone, Copy, Debug)]
pub enum Seen {
    Answered {
        status: u16,
        /// The request carried cookies from the jar.
        sent_cookies: bool,
        /// The answer set cookies of its own.
        got_cookies: bool,
    },
    /// The stand did not answer at all.
    Unreachable,
}

/// What an observation changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Change {
    /// The signals that went up, as they were raised.
    pub raised: Vec<Signal>,
    /// Those of them due a notification.
    pub toast: Vec<Signal>,
    /// The signals that went down.
    pub cleared: Vec<Signal>,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.raised.is_empty() && self.cleared.is_empty()
    }
}

/// The signals that are up, and when each last made a notification.
#[derive(Default)]
pub struct Board {
    active: Vec<Signal>,
    toasted: HashMap<Key, u64>,
}

impl Board {
    pub fn active(&self) -> &[Signal] {
        &self.active
    }

    /// Take in what one exchange between `app` and `server` showed.
    pub fn observe(&mut self, app: &str, server: &str, seen: Seen, now: u64) -> Change {
        let mut change = Change::default();
        let down = match seen {
            Seen::Unreachable => Some(None),
            Seen::Answered { status, .. } if STAND_DOWN.contains(&status) => Some(Some(status)),
            Seen::Answered { .. } => None,
        };
        match down {
            Some(status) => self.raise(Kind::StandDown, app, server, status, now, &mut change),
            None => self.clear(Kind::StandDown, app, server, &mut change),
        }
        if let Seen::Answered {
            status,
            sent_cookies,
            got_cookies,
        } = seen
        {
            // A stand that ends a session often says so with a Set-Cookie that
            // deletes it, on the very 401 - so only cookies that came with a
            // success count as a fresh login.
            if got_cookies && status < 400 {
                self.clear(Kind::SessionExpired, app, server, &mut change);
            } else if sent_cookies && SESSION_GONE.contains(&status) {
                self.raise(Kind::SessionExpired, app, server, Some(status), now, &mut change);
            }
        }
        change
    }

    /// The session of `app` on `server` was handed fresh cookies by hand.
    pub fn renewed(&mut self, app: &str, server: &str) -> Change {
        let mut change = Change::default();
        self.clear(Kind::SessionExpired, app, server, &mut change);
        change
    }

    fn raise(&mut self, kind: Kind, app: &str, server: &str, status: Option<u16>, now: u64, change: &mut Change) {
        let key = key(kind, app, server);
        if self.active.iter().any(|signal| signal.key() == key) {
            return;
        }
        let signal = Signal {
            kind,
            app: app.to_string(),
            server: server.to_string(),
            status,
            since: now,
        };
        let due = self.toasted.get(&key).is_none_or(|at| now.saturating_sub(*at) >= RETOAST);
        if due {
            self.toasted.insert(key, now);
            change.toast.push(signal.clone());
        }
        self.active.push(signal.clone());
        change.raised.push(signal);
    }

    fn clear(&mut self, kind: Kind, app: &str, server: &str, change: &mut Change) {
        let key = key(kind, app, server);
        if let Some(index) = self.active.iter().position(|signal| signal.key() == key) {
            change.cleared.push(self.active.remove(index));
        }
    }
}

/// The line the gateway's log and `status` say about a signal.
pub fn describe(signal: &Signal) -> String {
    match signal.kind {
        Kind::StandDown => match signal.status {
            Some(status) => format!("'{}' answers {status} - the stand is down, not the app", signal.server),
            None => format!("'{}' does not answer - the stand is down or out of reach", signal.server),
        },
        Kind::SessionExpired => format!(
            "the '{}' session of '{}' expired - log in again, or hand it a cookie with `turnout cookie set {}`",
            signal.server, signal.app, signal.app
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answered(status: u16) -> Seen {
        Seen::Answered {
            status,
            sent_cookies: true,
            got_cookies: false,
        }
    }

    /// One outage, however many requests run into it, is one signal and one
    /// notification; the first good answer ends it.
    #[test]
    fn a_stand_that_is_down_raises_one_signal_until_it_answers() {
        let mut board = Board::default();
        let change = board.observe("web", "prod", answered(502), 100);
        assert_eq!(change.raised.len(), 1);
        assert_eq!(change.toast.len(), 1);
        assert_eq!(change.raised[0].status, Some(502));
        for _ in 0..5 {
            assert!(board.observe("web", "prod", answered(503), 101).is_empty(), "a burst is still one outage");
        }
        // Another app on the same stand: the same outage.
        assert!(board.observe("admin", "prod", Seen::Unreachable, 102).is_empty());
        assert_eq!(board.active().len(), 1);
        let change = board.observe("admin", "prod", answered(404), 103);
        assert_eq!(change.cleared.len(), 1, "any answer below 500 means the stand is back");
        assert!(board.active().is_empty());
    }

    /// A stand that flaps is reported in `status` each time, but the desktop
    /// hears about it once per [`RETOAST`].
    #[test]
    fn a_flapping_stand_does_not_toast_again_too_soon() {
        let mut board = Board::default();
        assert_eq!(board.observe("web", "prod", answered(502), 1_000).toast.len(), 1);
        board.observe("web", "prod", answered(200), 1_010);
        let again = board.observe("web", "prod", answered(502), 1_020);
        assert_eq!(again.raised.len(), 1, "status shows it");
        assert!(again.toast.is_empty(), "the desktop does not hear it twice in ten minutes");
        board.observe("web", "prod", answered(200), 1_030);
        let later = board.observe("web", "prod", answered(502), 1_000 + RETOAST);
        assert_eq!(later.toast.len(), 1);
    }

    /// Two stands down are two signals.
    #[test]
    fn stands_are_told_apart() {
        let mut board = Board::default();
        board.observe("web", "prod", answered(502), 1);
        board.observe("web", "dev", Seen::Unreachable, 2);
        assert_eq!(board.active().len(), 2);
        assert_eq!(board.active()[1].status, None);
    }

    /// A 401 is a lapsed session only when there was a session to lapse: the
    /// request carried the jar's cookies. A first visit that is not logged in
    /// yet is the app's business.
    #[test]
    fn a_session_expires_only_when_cookies_were_sent() {
        let mut board = Board::default();
        let first_visit = Seen::Answered {
            status: 401,
            sent_cookies: false,
            got_cookies: false,
        };
        assert!(board.observe("web", "prod", first_visit, 1).is_empty());
        let change = board.observe("web", "prod", answered(401), 2);
        assert_eq!(change.raised[0].kind, Kind::SessionExpired);
        assert_eq!(change.toast.len(), 1);
        assert!(
            board.observe("web", "prod", answered(440), 3).is_empty(),
            "IIS's code for it is the same signal"
        );
        // Each app keeps its own jar, and so its own session.
        assert_eq!(board.observe("admin", "prod", answered(401), 4).raised.len(), 1);
    }

    /// The 401 that ends a session usually deletes the cookie too; that
    /// Set-Cookie is not a login. A success that sets cookies is.
    #[test]
    fn only_a_successful_answer_with_cookies_renews_a_session() {
        let mut board = Board::default();
        board.observe("web", "prod", answered(401), 1);
        let deleting = Seen::Answered {
            status: 401,
            sent_cookies: true,
            got_cookies: true,
        };
        assert!(board.observe("web", "prod", deleting, 2).is_empty());
        assert_eq!(board.active().len(), 1);
        assert!(
            board.observe("web", "prod", answered(200), 3).is_empty(),
            "a page that sets nothing proves nothing"
        );
        let login = Seen::Answered {
            status: 302,
            sent_cookies: true,
            got_cookies: true,
        };
        assert_eq!(board.observe("web", "prod", login, 4).cleared.len(), 1);
        assert!(board.active().is_empty());
    }

    #[test]
    fn cookies_set_by_hand_renew_the_session() {
        let mut board = Board::default();
        board.observe("web", "prod", answered(401), 1);
        assert_eq!(board.renewed("web", "prod").cleared.len(), 1);
        assert!(board.renewed("web", "prod").is_empty());
    }

    /// The registry keeps the signals; they must read back as written.
    #[test]
    fn signals_read_back_as_written() {
        let signal = Signal {
            kind: Kind::SessionExpired,
            app: "web".into(),
            server: "prod".into(),
            status: Some(401),
            since: 9,
        };
        let text = serde_json::to_string(&signal).unwrap();
        assert!(text.contains(r#""kind":"session_expired""#), "{text}");
        assert_eq!(serde_json::from_str::<Signal>(&text).unwrap(), signal);
    }
}
