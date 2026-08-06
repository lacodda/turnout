use anyhow::{Context, Result};

use crate::model::Server;

/// Best-effort reachability probe used by `use` and `status`.
pub fn check_reachable(server: &Server) -> Result<reqwest::StatusCode> {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(server.accept_invalid_certs)
            .timeout(std::time::Duration::from_secs(4))
            .build()?;
        let response = client.get(&server.url).send().await.with_context(|| format!("{} is unreachable", server.url))?;
        Ok(response.status())
    })
}
