//! One-shot URL fetch for `view-source:`.
//!
//! Everything else Beacon loads goes through the engine, which owns networking, cookies,
//! the cache and the user agent. `view-source:` is the exception: it needs the raw bytes
//! in the shell so [`crate::window::source_page`] can mark them up, and the engine has no
//! embedder-facing "fetch me this URL" API — its `net` fetcher is internal, reachable only
//! through a navigation.
//!
//! So this is a deliberate stopgap, and it has the limitation you would expect from one:
//! the request carries no cookies and shares no cache with the engine, so viewing the
//! source of a page behind a login shows the logged-out HTML. The fix is an engine-side
//! fetch API (or a `TabCommand` that hands back the last response body), at which point
//! this module goes away.

use url::Url;

/// Fetch `url` and return its body. `user_agent` should be the engine's configured UA, so
/// servers at least see the same client they saw for the page itself.
pub async fn url_body(url: Url, user_agent: String) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .user_agent(user_agent)
        .build()
        .map_err(|e| e.to_string())?;

    let response = client.get(url).send().await.map_err(|e| e.to_string())?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("server returned {status}"));
    }

    response.bytes().await.map(|b| b.to_vec()).map_err(|e| e.to_string())
}
