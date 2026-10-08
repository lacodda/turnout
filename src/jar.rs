//! The gateway's cookie jar: what a stand set, kept the way a browser keeps it.
//!
//! ADR 0002 promised that the gateway honours cookie semantics, and up to
//! v0.22 it kept only `name=value`. A stand that ended a session with
//! `Set-Cookie: sid=deleted; Max-Age=0` got `sid=deleted` back on every
//! request after; a cookie past its `Expires` was still sent; a cookie set
//! for `Path=/api` went to every path. The storage and matching rules are
//! RFC 6265's, and `cookie_store` implements them whole - the lenient date
//! formats, Max-Age over Expires, the default path, Secure, domains that are
//! IP addresses - so the jar is that store plus what the gateway itself
//! remembers about the session.
//!
//! What it remembers is the evidence `turnout doctor` needs to say whether a
//! session is alive *now*: the stand's last answer to a request that carried
//! the jar's cookies, and the last request that proved the session - the one
//! the gateway can send again to ask (see [`proves`]).

use std::time::Duration;

use cookie_store::{CookieExpiration, CookieStore, RawCookie, StoreAction};
use reqwest::Url;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

/// The longest a proving request may have taken to answer. A request that
/// waited longer is a long poll or a slow report, and sending it again from
/// `doctor` would make the check wait as long - or longer than its patience.
pub const QUICK: Duration = Duration::from_secs(2);

/// One app's cookies for one stand, and what the stand last said about them.
#[derive(Default)]
pub struct Jar {
    store: CookieStore,
    proof: Option<Proof>,
    last: Option<Answer>,
}

/// The last request that proved the session: see [`proves`].
#[derive(Clone, Debug)]
pub struct Proof {
    /// The path and query, under the stand's URL.
    pub path: String,
    /// The request's own headers as the app sent them, without `Cookie` -
    /// the jar's current cookies go in its place when it is sent again. An app
    /// that also authenticates with a header (a bearer token, an API key)
    /// needs it as much as the cookies, and the check is only honest when it
    /// asks what the app asked.
    pub headers: HeaderMap,
    /// Seconds since the epoch.
    pub at: u64,
}

/// What the stand answered, and when.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    /// Seconds since the epoch.
    pub at: u64,
}

/// What one answer's `Set-Cookie` headers did to the jar.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Taken {
    /// Cookies added or replaced.
    pub stored: usize,
    /// Cookies the stand deleted - an expiry in the past, `Max-Age=0`.
    pub deleted: usize,
}

impl Jar {
    /// The `Cookie` header for a request to `url`, when any cookie matches it.
    ///
    /// `url` is always an `http(s)` address, a WebSocket's included: the
    /// upgrade is an HTTP request, and a browser sends its HttpOnly and Secure
    /// cookies with it - matched against `ws://` they would be left out.
    pub fn header(&self, url: &Url) -> Option<String> {
        let pairs: Vec<String> = self.store.get_request_values(url).map(|(name, value)| format!("{name}={value}")).collect();
        (!pairs.is_empty()).then(|| pairs.join("; "))
    }

    /// Take in the `Set-Cookie` headers of an answer to a request for `url`.
    ///
    /// A header the store refuses - a domain that is not the stand's, a date
    /// that does not parse - is dropped, as a browser drops it.
    pub fn take<'a>(&mut self, url: &Url, set_cookies: impl IntoIterator<Item = &'a str>) -> Taken {
        let mut taken = Taken::default();
        for text in set_cookies {
            match self.store.parse(text, url) {
                Ok(StoreAction::Inserted | StoreAction::UpdatedExisting) => taken.stored += 1,
                Ok(StoreAction::ExpiredExisting) => taken.deleted += 1,
                Err(_) => {}
            }
        }
        taken
    }

    /// Replace everything with cookies copied from a browser, as `cookie set`
    /// hands them over.
    ///
    /// A `Cookie` header carries names and values only. The cookies are kept
    /// for the stand's own host, for every path on it and for this session -
    /// the widest reading, because the browser sent them all to the request
    /// they were copied from. The evidence about the old session goes with it.
    pub fn replace(&mut self, url: &Url, pairs: Vec<(String, String)>) {
        *self = Jar::default();
        for (name, value) in pairs {
            let cookie = RawCookie::build((name, value)).path("/").build();
            let _ = self.store.insert_raw(&cookie, url);
        }
    }

    /// The names of the cookies that are good, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.store.iter_unexpired().map(|cookie| cookie.name().to_string()).collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn is_empty(&self) -> bool {
        self.store.iter_unexpired().next().is_none()
    }

    /// When the first of the good cookies expires, in seconds since the epoch;
    /// `None` when they all last for the session.
    pub fn expires(&self) -> Option<u64> {
        self.store
            .iter_unexpired()
            .filter_map(|cookie| match &cookie.expires {
                CookieExpiration::AtUtc(at) => u64::try_from(at.unix_timestamp()).ok(),
                CookieExpiration::SessionEnd => None,
            })
            .min()
    }

    /// The stand answered a request that carried this jar's cookies.
    pub fn answered(&mut self, answer: Answer) {
        self.last = Some(answer);
    }

    /// That request proved the session; it is the one to send again.
    pub fn proved(&mut self, proof: Proof) {
        self.proof = Some(proof);
    }

    pub fn last(&self) -> Option<Answer> {
        self.last
    }

    pub fn proof(&self) -> Option<&Proof> {
        self.proof.as_ref()
    }
}

/// Whether one exchange proves the session - makes it the request to send
/// again when `doctor` asks whether the session is still alive.
///
/// All of it has to hold. A GET, because only a GET is safe to repeat
/// unasked. The jar's cookies went with it, or it proves nothing about them.
/// A 2xx: a redirect is as often the way to the login page as anywhere else.
/// Nothing deleted: a GET that ends a session (a logout link) answers by
/// deleting the cookie, and sending it again would end the next one. And
/// [`QUICK`]: a long poll that answers when it has news would hold the check
/// up for as long.
pub fn proves(get: bool, sent_cookies: bool, status: u16, deleted: usize, took: Duration) -> bool {
    get && sent_cookies && (200..300).contains(&status) && deleted == 0 && took <= QUICK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    /// The defect the wish named: a `Set-Cookie` that deletes the session
    /// left `sid=deleted` in the jar, and the stand got it back for ever.
    #[test]
    fn a_deleting_set_cookie_takes_the_cookie_out() {
        let stand = url("https://staging.example.com/api/login");
        let mut jar = Jar::default();
        assert_eq!(jar.take(&stand, ["sid=abc; Path=/", "lang=en; Path=/"]), Taken { stored: 2, deleted: 0 });
        assert_eq!(
            jar.header(&url("https://staging.example.com/")).as_deref().map(sorted),
            Some(sorted("lang=en; sid=abc"))
        );

        let taken = jar.take(&stand, ["sid=deleted; Path=/; Max-Age=0"]);
        assert_eq!(taken, Taken { stored: 0, deleted: 1 });
        assert_eq!(jar.header(&url("https://staging.example.com/")).as_deref(), Some("lang=en"));
        assert_eq!(jar.names(), vec!["lang"]);

        // An expiry in the past deletes the same way.
        jar.take(&stand, ["lang=gone; Path=/; Expires=Thu, 01 Jan 1970 00:00:01 GMT"]);
        assert!(jar.is_empty(), "the jar still holds {:?}", jar.names());
        assert_eq!(jar.header(&url("https://staging.example.com/")), None);
    }

    /// Path and Secure are honoured: a cookie for `/api` stays off other
    /// paths, a Secure one stays off plain HTTP.
    #[test]
    fn a_cookie_goes_only_where_it_belongs() {
        let mut jar = Jar::default();
        jar.take(
            &url("https://staging.example.com/api/login"),
            ["token=t; Path=/api", "secret=s; Path=/; Secure"],
        );
        assert_eq!(
            jar.header(&url("https://staging.example.com/api/user")).as_deref().map(sorted),
            Some(sorted("secret=s; token=t"))
        );
        assert_eq!(jar.header(&url("https://staging.example.com/static/app.js")).as_deref(), Some("secret=s"));
        assert_eq!(jar.header(&url("http://staging.example.com/api/user")).as_deref(), Some("token=t"));
    }

    /// A cookie set for another domain is refused, as a browser refuses it.
    #[test]
    fn a_cookie_for_another_domain_is_not_kept() {
        let mut jar = Jar::default();
        let taken = jar.take(&url("https://staging.example.com/"), ["sid=x; Domain=other.example.org"]);
        assert_eq!(taken, Taken::default());
        assert!(jar.is_empty());
        // The stand's own parent domain is fine, and reaches its siblings.
        jar.take(&url("https://staging.example.com/"), ["sid=y; Domain=example.com; Path=/"]);
        assert_eq!(jar.header(&url("https://api.example.com/")).as_deref(), Some("sid=y"));
    }

    /// Max-Age is a lifetime from now, and it wins over Expires; a session
    /// cookie has no expiry at all.
    #[test]
    fn the_jar_knows_when_its_first_cookie_expires() {
        let mut jar = Jar::default();
        let stand = url("https://staging.example.com/");
        jar.take(&stand, ["session=s; Path=/"]);
        assert_eq!(jar.expires(), None, "a session cookie lasts as long as the gateway");
        let now = crate::registry::now();
        jar.take(&stand, ["short=x; Path=/; Max-Age=600; Expires=Fri, 01 Jan 2100 00:00:00 GMT"]);
        let first = jar.expires().expect("a cookie with a lifetime");
        assert!((now + 590..=now + 610).contains(&first), "Max-Age=600 from {now} gave {first}");
        jar.take(&stand, ["long=y; Path=/; Expires=Fri, 01 Jan 2100 00:00:00 GMT"]);
        assert_eq!(jar.expires(), Some(first), "the first to go is the one reported");
    }

    /// Cookies from a browser cover the whole stand, and replace the jar -
    /// the old session's evidence included.
    #[test]
    fn cookies_from_a_browser_replace_the_jar() {
        let stand = url("http://127.0.0.1:8080");
        let mut jar = Jar::default();
        jar.take(&stand, ["old=1; Path=/"]);
        jar.answered(Answer { status: 200, at: 1 });
        jar.replace(&stand, vec![("sid".into(), "abc".into()), ("lang".into(), "en".into())]);
        assert_eq!(jar.names(), vec!["lang", "sid"]);
        assert_eq!(
            jar.header(&url("http://127.0.0.1:8080/deep/path?q=1")).as_deref().map(sorted),
            Some(sorted("lang=en; sid=abc"))
        );
        assert_eq!(jar.last(), None, "an answer about the old session says nothing about this one");
    }

    #[test]
    fn only_a_quick_successful_get_with_cookies_that_deletes_nothing_proves_the_session() {
        let quick = Duration::from_millis(80);
        assert!(proves(true, true, 200, 0, quick));
        assert!(proves(true, true, 204, 0, QUICK));
        assert!(!proves(false, true, 200, 0, quick), "a POST is not repeated unasked");
        assert!(!proves(true, false, 200, 0, quick), "no cookies, no proof about them");
        assert!(!proves(true, true, 302, 0, quick), "a redirect may be the way to the login page");
        assert!(!proves(true, true, 401, 0, quick));
        assert!(!proves(true, true, 200, 1, quick), "a logout link deletes the session");
        assert!(!proves(true, true, 200, 0, QUICK + Duration::from_millis(1)), "a long poll");
    }

    /// Cookie order in a header is the store's; tests compare the set.
    fn sorted(header: &str) -> String {
        let mut pairs: Vec<&str> = header.split("; ").collect();
        pairs.sort_unstable();
        pairs.join("; ")
    }
}
