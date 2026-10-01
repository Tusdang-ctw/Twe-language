//! web3d-M5: one blocking HTTPS POST with retries, for the API
//! providers. Built only with the `http` feature.

use std::time::Duration;

use crate::Error;

/// Attempts after the first for rate limits (429), overload (529) and
/// other server errors (5xx), and for dropped connections.
const RETRIES: u32 = 4;
/// A reply can take minutes: a thinking model on a hard task.
const TIMEOUT: Duration = Duration::from_secs(600);

/// POST `body` (JSON) to `url` with `headers`; the response body on a
/// 2xx status. Retryable failures back off 2, 4, 8, 16 s, or as long
/// as the service's `retry-after` asks (up to a minute).
pub(crate) fn post_json(url: &str, headers: &[(&str, String)], body: &str) -> Result<String, Error> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let mut attempt = 0;
    loop {
        let mut request = agent.post(url).header("content-type", "application/json");
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        let wait = match request.send(body.as_bytes()) {
            Ok(mut response) => {
                let status = response.status().as_u16();
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok());
                let text = response
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| Error::Transport(format!("reading the response failed: {e}")))?;
                if (200..300).contains(&status) {
                    return Ok(text);
                }
                let retryable = status == 429 || status >= 500;
                if !retryable || attempt == RETRIES {
                    return Err(Error::Http { status, message: error_message(&text) });
                }
                retry_after.map(|s| Duration::from_secs(s.min(60)))
            }
            Err(e) => {
                if attempt == RETRIES {
                    return Err(Error::Transport(format!("request to {url} failed: {e}")));
                }
                None
            }
        };
        let backoff = Duration::from_secs(2u64 << attempt);
        std::thread::sleep(wait.unwrap_or(backoff));
        attempt += 1;
    }
}

/// The message in a JSON error body (`{"error": {"message": ...}}` in
/// both APIs), or the body itself.
fn error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| body.chars().take(500).collect())
}
