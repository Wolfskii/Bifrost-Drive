use reqwest::{header, RequestBuilder, Response, StatusCode};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MAX_RETRIES: u32 = 5;
const MAX_BACKOFF: Duration = Duration::from_secs(16);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum DispatchError {
    Network(reqwest::Error),
    Forbidden { rate_limited: bool, body: String },
}

/// Sends a Google API request, retrying rate limits, 5xx responses, and connection
/// failures with exponential backoff. Streaming bodies cannot be cloned and are sent once.
pub async fn dispatch(mut request: RequestBuilder) -> Result<Response, DispatchError> {
    let mut attempt = 0;
    loop {
        let retry = if attempt < MAX_RETRIES {
            request.try_clone()
        } else {
            None
        };
        let (delay, next) = match (request.send().await, retry) {
            (Ok(response), retry) if response.status() == StatusCode::FORBIDDEN => {
                let body = response.text().await.unwrap_or_default();
                let rate_limited = is_rate_limited(&body);
                match retry {
                    Some(next) if rate_limited => (backoff(attempt), next),
                    _ => return Err(DispatchError::Forbidden { rate_limited, body }),
                }
            }
            (Ok(response), Some(next)) if is_transient(response.status()) => (
                retry_after(&response).unwrap_or_else(|| backoff(attempt)),
                next,
            ),
            (Ok(response), _) => return Ok(response),
            (Err(error), Some(next)) if error.is_connect() || error.is_timeout() => {
                (backoff(attempt), next)
            }
            (Err(error), _) => return Err(DispatchError::Network(error)),
        };
        tokio::time::sleep(delay).await;
        request = next;
        attempt += 1;
    }
}

fn is_rate_limited(body: &str) -> bool {
    body.contains("rateLimitExceeded") || body.contains("userRateLimitExceeded")
}

fn is_transient(status: StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504 | 509)
}

fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(|seconds| Duration::from_secs(seconds).min(MAX_RETRY_AFTER))
}

fn backoff(attempt: u32) -> Duration {
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_millis());
    Duration::from_secs(1u64 << attempt.min(4)).min(MAX_BACKOFF)
        + Duration::from_millis(u64::from(jitter))
}

#[cfg(test)]
mod tests {
    use super::{backoff, is_rate_limited, is_transient, MAX_BACKOFF};
    use reqwest::StatusCode;
    use std::time::Duration;

    #[test]
    fn detects_google_rate_limit_reasons() {
        assert!(is_rate_limited(
            r#"{"error":{"errors":[{"reason":"userRateLimitExceeded"}]}}"#
        ));
        assert!(is_rate_limited(
            r#"{"errors":[{"reason":"rateLimitExceeded"}]}"#
        ));
        assert!(!is_rate_limited(
            r#"{"errors":[{"reason":"insufficientPermissions"}]}"#
        ));
    }

    #[test]
    fn retries_only_transient_statuses() {
        assert!(is_transient(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_transient(StatusCode::SERVICE_UNAVAILABLE));
        assert!(!is_transient(StatusCode::NOT_FOUND));
        assert!(!is_transient(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn backoff_grows_and_stays_bounded() {
        assert!(backoff(0) >= Duration::from_secs(1));
        assert!(backoff(0) < Duration::from_secs(2));
        assert!(backoff(3) >= Duration::from_secs(8));
        assert!(backoff(20) < MAX_BACKOFF + Duration::from_secs(1));
    }
}
