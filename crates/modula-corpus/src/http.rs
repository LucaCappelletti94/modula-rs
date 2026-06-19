//! Minimal HTTP downloads (db-dump and crate tarballs) over a native-TLS agent.
//!
//! native-tls (system OpenSSL) is used deliberately instead of the rustls/ring
//! stack so the dependency tree stays inside the workspace's permissive license
//! allow-list, with no `ring` license-file clarification.

use std::io::Read as _;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};

const UA: &str = "modula-rs-corpus (https://github.com/LucaCappelletti94/modula-rs)";

/// Builds a ureq agent backed by the system native-TLS implementation.
pub fn agent() -> Result<ureq::Agent> {
    let connector = native_tls::TlsConnector::new().context("building native-tls connector")?;
    Ok(ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .tls_connector(Arc::new(connector))
        .user_agent(UA)
        .build())
}

/// GETs `url` and returns the full response body.
pub fn get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    let resp = agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    let mut buf = Vec::new();
    resp.into_reader()
        .read_to_end(&mut buf)
        .with_context(|| format!("reading body of {url}"))?;
    Ok(buf)
}
