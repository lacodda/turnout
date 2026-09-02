//! Talking to a running SSH agent.
//!
//! An agent holds unlocked private keys and signs on their behalf, so a key
//! with a passphrase is unlocked once per login session instead of once per
//! command. That is the whole point of the third auth kind: `key` auth reads
//! the key file itself and must have the passphrase every time turnout runs;
//! `agent` auth never sees the key at all.
//!
//! Support rode on libssh2's `userauth_agent` until v0.10.0, where the move to
//! russh dropped it rather than pretend it still worked. russh offers the
//! agent as a `Signer` the client authenticates *with*, which is what this
//! module wires up.
//!
//! Where the agent lives is the platform's business:
//!
//! - Unix: a Unix-domain socket named by `SSH_AUTH_SOCK`.
//! - Windows: Pageant (PuTTY, and what Git for Windows ships), or the named
//!   pipe that the OpenSSH agent service listens on. Both are tried, because
//!   which one is running is a property of the machine, not of turnout.

use anyhow::{Context, Result};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};

/// The agent connection, with the platform's stream boxed away.
///
/// `dynamic()` erases the stream type so Unix sockets, Pageant and named pipes
/// all reach [`crate::ssh`] as one type - the auth path has no business
/// knowing which door the agent answered.
pub type Agent = AgentClient<Box<dyn AgentStream + Send + Unpin + 'static>>;

/// The named pipe the Windows OpenSSH agent service listens on. Fixed by
/// OpenSSH, not configurable, so it is a constant rather than a setting.
#[cfg(windows)]
const OPENSSH_PIPE: &str = r"\.\pipe\openssh-ssh-agent";

/// Connect to the running agent.
///
/// The error is the one the user acts on: there is no point reporting "the
/// agent said no" when what happened is that no agent is running at all, so
/// the message names how one is started on *this* platform.
pub async fn connect() -> Result<Agent> {
    #[cfg(unix)]
    {
        // An unset SSH_AUTH_SOCK and a socket that is not there are different
        // mistakes: one means no agent was ever started in this shell, the
        // other that the variable outlived the agent it pointed at.
        let socket = std::env::var("SSH_AUTH_SOCK")
            .map_err(|_| anyhow::anyhow!("no SSH agent: SSH_AUTH_SOCK is not set - start one with `eval $(ssh-agent)`, then add the key with `ssh-add`"))?;
        return AgentClient::connect_uds(&socket)
            .await
            .map(AgentClient::dynamic)
            .with_context(|| format!("cannot reach the SSH agent at {socket}"));
    }

    #[cfg(windows)]
    {
        // Pageant first: it is the agent Git for Windows and PuTTY users
        // actually have running. The OpenSSH service is the fallback, and its
        // failure is the one reported - if neither answered, naming the
        // service is the more actionable of the two.
        if let Ok(client) = AgentClient::connect_pageant().await {
            return Ok(client.dynamic());
        }
        return AgentClient::connect_named_pipe(OPENSSH_PIPE)
            .await
            .map(AgentClient::dynamic)
            .context(
                "no SSH agent: neither Pageant nor the OpenSSH agent service answered - start the service with `Start-Service ssh-agent`, then add the key with `ssh-add`",
            );
    }

    #[cfg(not(any(unix, windows)))]
    anyhow::bail!("SSH agents are not supported on this platform")
}

/// The identities the agent is holding, in the order it offers them.
pub async fn identities(agent: &mut Agent) -> Result<Vec<AgentIdentity>> {
    agent.request_identities().await.context("cannot list the SSH agent's keys")
}

/// A one-line description of an identity, for `credential show` and for the
/// message that says which keys an agent does hold.
///
/// The comment is what a person recognises a key by (`ssh-add -l` prints it
/// too); a key added without one still has to be nameable, so the algorithm
/// stands in for it.
pub fn describe(identity: &AgentIdentity) -> String {
    let key = identity.public_key();
    let algorithm = key.algorithm().as_str().to_string();
    let comment = identity.comment().trim();
    if comment.is_empty() { algorithm } else { format!("{algorithm} {comment}") }
}

/// The error for an agent that is running but holds nothing.
///
/// Its own case on purpose: "the server rejected the credential" would send
/// the user off checking the server, when what is wrong is on this machine and
/// one `ssh-add` away.
pub fn no_identities() -> anyhow::Error {
    anyhow::anyhow!("the SSH agent is running but holds no keys - add one with `ssh-add PATH`")
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
    use russh::keys::{PrivateKey, PublicKey};

    fn key(seed: u8, comment: &str) -> PrivateKey {
        let pair = Ed25519Keypair::from_seed(&[seed; 32]);
        PrivateKey::new(KeypairData::Ed25519(pair), comment).expect("an ed25519 key from a fixed seed")
    }

    fn identity(seed: u8, comment: &str) -> AgentIdentity {
        let public: PublicKey = key(seed, comment).public_key().clone();
        AgentIdentity::PublicKey {
            key: public,
            comment: comment.to_string(),
        }
    }

    #[test]
    fn an_identity_is_described_by_its_comment() {
        assert_eq!(describe(&identity(1, "me@laptop")), "ssh-ed25519 me@laptop");
    }

    /// A key can be added to an agent with no comment at all; describing it as
    /// an empty string would print a blank line where a name belongs.
    #[test]
    fn an_identity_without_a_comment_falls_back_to_its_algorithm() {
        assert_eq!(describe(&identity(2, "")), "ssh-ed25519");
        assert_eq!(describe(&identity(3, "   ")), "ssh-ed25519", "whitespace is not a comment");
    }

    /// The empty agent is a distinct situation from a rejected key, and the
    /// message has to point at this machine rather than at the server.
    #[test]
    fn an_empty_agent_names_the_fix_on_this_machine() {
        let message = no_identities().to_string();
        assert!(message.contains("holds no keys"), "{message}");
        assert!(message.contains("ssh-add"), "{message}");
    }
}
