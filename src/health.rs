//! Is a stand up? One probe of its `/health`, before a switch.
//!
//! A stand is somebody's machine, and it is off more often than anyone would
//! like. Switching an app to it and finding out from a blank page costs a
//! minute of looking at the wrong thing; `use` asks first. The server picker
//! probes every stand it offers at once, so the answer is beside each name
//! when the choice is made - and the one that was picked is not asked again.
//!
//! The probe is `GET {url}/health` - the path most backends answer on when
//! they have one. A stand without it answers 404, which still says the stand
//! is there; only 5xx and silence mean it is not.

use std::time::{Duration, Instant};

use crate::model::Server;

/// How long a probe waits. Long enough for a stand across a VPN, short enough
/// that a picker over five dead stands opens in a few seconds.
pub const TIMEOUT: Duration = Duration::from_secs(3);

/// What one probe found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Health {
    /// It answered, below 500.
    Up { status: u16, millis: u128 },
    /// It answered 5xx: the stand's proxy is there, the application is not.
    Down { status: u16 },
    /// It did not answer: off, out of reach, or the name does not resolve.
    Unreachable,
    /// It answered with a certificate this machine does not trust, and the
    /// server is not marked to accept one. The stand is there; the gateway
    /// will not talk to it until it is told to.
    Untrusted,
}

impl Health {
    /// The few words the picker puts beside a stand.
    pub fn label(&self) -> String {
        match self {
            Health::Up { millis, .. } => format!("up, {millis} ms"),
            Health::Down { status } => format!("down, {status}"),
            Health::Unreachable => "unreachable".to_string(),
            Health::Untrusted => "untrusted certificate".to_string(),
        }
    }

    /// The sentence `use` says after a switch.
    pub fn sentence(&self, server: &Server) -> String {
        let url = health_url(&server.url);
        match self {
            Health::Up { status, millis } => format!("'{}' is up: {url} answered {status} in {millis} ms.", server.name),
            Health::Down { status } => format!(
                "warning: '{}' is down - {url} answered {status}; requests through the gateway will fail until it is back.",
                server.name
            ),
            Health::Unreachable => format!(
                "warning: '{}' did not answer within {}s - the stand is off or out of reach.",
                server.name,
                TIMEOUT.as_secs()
            ),
            Health::Untrusted => format!(
                "warning: '{0}' answers with a certificate this machine does not trust - accept it with `turnout server edit {0} --insecure`.",
                server.name
            ),
        }
    }
}

/// Where the probe goes: the stand's own URL, plus `/health`.
pub fn health_url(url: &str) -> String {
    format!("{}/health", url.trim_end_matches('/'))
}

/// Probe every server at once; the answers come back in the same order.
pub fn probe_all(servers: &[&Server]) -> Vec<Health> {
    let probes = servers.iter().map(|server| probe((*server).clone()));
    crate::utils::run_blocking(async move { Ok(futures_util::future::join_all(probes).await) }).unwrap_or_else(|_| vec![Health::Unreachable; servers.len()])
}

/// Probe one server.
pub fn probe_one(server: &Server) -> Health {
    probe_all(&[server]).pop().unwrap_or(Health::Unreachable)
}

async fn probe(server: Server) -> Health {
    // No redirects: a stand that sends /health to its login page answered,
    // and the redirect is the answer.
    let client = match reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .danger_accept_invalid_certs(server.accept_invalid_certs)
        .timeout(TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(_) => return Health::Unreachable,
    };
    let started = Instant::now();
    match client.get(health_url(&server.url)).send().await {
        Ok(response) => classify(response.status().as_u16(), started.elapsed()),
        Err(err) if !server.accept_invalid_certs && is_certificate_error(&err) => Health::Untrusted,
        Err(_) => Health::Unreachable,
    }
}

/// Whether a failed request failed on the stand's certificate. rustls says so
/// somewhere down the chain of sources - "invalid peer certificate:
/// UnknownIssuer" - and reqwest only wraps it as a connect error.
fn is_certificate_error(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(err) = current {
        if err.to_string().to_ascii_lowercase().contains("certificate") {
            return true;
        }
        current = err.source();
    }
    false
}

fn classify(status: u16, elapsed: Duration) -> Health {
    if status >= 500 {
        Health::Down { status }
    } else {
        Health::Up {
            status,
            millis: elapsed.as_millis(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_goes_to_health_under_the_stand_url() {
        assert_eq!(health_url("https://staging.example.com"), "https://staging.example.com/health");
        assert_eq!(health_url("https://staging.example.com/app/"), "https://staging.example.com/app/health");
    }

    /// 404 is a stand without a health endpoint, and it is there; 5xx is a
    /// stand whose application is not.
    #[test]
    fn any_answer_below_500_is_a_stand_that_is_up() {
        let ms = Duration::from_millis(120);
        assert_eq!(classify(200, ms), Health::Up { status: 200, millis: 120 });
        assert_eq!(classify(404, ms), Health::Up { status: 404, millis: 120 });
        assert_eq!(classify(302, ms), Health::Up { status: 302, millis: 120 });
        assert_eq!(classify(502, ms), Health::Down { status: 502 });
        assert_eq!(classify(503, ms).label(), "down, 503");
        assert_eq!(classify(200, ms).label(), "up, 120 ms");
    }

    /// A stand with a self-signed certificate is there, and says so: the
    /// probe names the certificate rather than calling the stand unreachable.
    #[test]
    fn a_certificate_this_machine_does_not_trust_is_named() {
        #[derive(Debug)]
        struct Wrapped(Box<dyn std::error::Error + 'static>);
        impl std::fmt::Display for Wrapped {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("error sending request")
            }
        }
        impl std::error::Error for Wrapped {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(self.0.as_ref())
            }
        }
        let tls = Wrapped("invalid peer certificate: UnknownIssuer".into());
        assert!(is_certificate_error(&tls));
        let refused = Wrapped("connection refused".into());
        assert!(!is_certificate_error(&refused));
        assert_eq!(Health::Untrusted.label(), "untrusted certificate");
    }
}
