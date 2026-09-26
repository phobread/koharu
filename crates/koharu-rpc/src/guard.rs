//! Request guard for the local HTTP server.
//!
//! The API is unauthenticated, so it must only answer the Koharu UI (served
//! same-origin) and non-browser clients. Otherwise any web page open in the
//! user's browser could drive it. Two checks:
//!
//! - `Origin` — browsers attach it to cross-origin requests. Only same-origin
//!   and loopback origins (e.g. the Next dev server) are accepted.
//! - `Host` — when bound to loopback, only loopback hosts are accepted. This
//!   blocks DNS rebinding, where an attacker's domain resolves to 127.0.0.1
//!   and would otherwise count as "same origin". Skipped for non-loopback
//!   binds (`--host 0.0.0.0`), where the server is reached under names we
//!   can't know in advance.
//!
//! Requests without these headers (curl, SDKs, MCP clients) pass through.

use std::net::IpAddr;
use std::str::FromStr;

use axum::extract::{Request, State};
use axum::http::uri::Authority;
use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;

const X_FORWARDED_HOST: &str = "x-forwarded-host";

#[derive(Debug, Clone, Copy)]
pub struct RequestGuard {
    enforce_loopback_host: bool,
}

impl RequestGuard {
    /// Guard for a server listening on `addr`.
    pub fn for_bind_addr(addr: IpAddr) -> Self {
        Self {
            enforce_loopback_host: addr.is_loopback(),
        }
    }

    pub fn check(&self, headers: &HeaderMap) -> Result<(), &'static str> {
        if self.enforce_loopback_host
            && let Some(host) = headers.get(header::HOST)
            && !parse_authority(host).is_some_and(|a| is_loopback_host(a.host()))
        {
            return Err("host not allowed");
        }

        if let Some(origin) = headers.get(header::ORIGIN)
            && !origin_allowed(origin, headers)
        {
            return Err("origin not allowed");
        }

        Ok(())
    }
}

pub async fn middleware(
    State(guard): State<RequestGuard>,
    request: Request,
    next: Next,
) -> Response {
    if let Err(reason) = guard.check(request.headers()) {
        tracing::warn!(
            reason,
            method = %request.method(),
            path = request.uri().path(),
            "rejected request"
        );
        return ApiError::new(StatusCode::FORBIDDEN, reason).into_response();
    }
    next.run(request).await
}

fn origin_allowed(origin: &HeaderValue, headers: &HeaderMap) -> bool {
    let Some(origin) = origin.to_str().ok().and_then(origin_authority) else {
        return false;
    };
    if is_loopback_host(origin.host()) {
        return true;
    }
    // Same-origin: the UI served over LAN or behind a reverse proxy. A
    // cross-origin page can't set `X-Forwarded-Host` without a preflight,
    // which fails because no CORS headers are served.
    let forwarded = headers
        .get(X_FORWARDED_HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|v| Authority::from_str(v.trim()).ok());
    let host = headers.get(header::HOST).and_then(parse_authority);
    host.as_ref() == Some(&origin) || forwarded.as_ref() == Some(&origin)
}

/// `http(s)://authority` → `authority`. Rejects `null` and other schemes.
fn origin_authority(origin: &str) -> Option<Authority> {
    let uri = Uri::from_str(origin).ok()?;
    match uri.scheme_str() {
        Some("http" | "https") => uri.authority().cloned(),
        _ => None,
    }
}

fn parse_authority(value: &HeaderValue) -> Option<Authority> {
    Authority::from_str(value.to_str().ok()?).ok()
}

fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    fn loopback() -> RequestGuard {
        RequestGuard::for_bind_addr(IpAddr::V4(Ipv4Addr::LOCALHOST))
    }

    fn any_addr() -> RequestGuard {
        RequestGuard::for_bind_addr(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn allows_requests_without_browser_headers() {
        assert!(loopback().check(&headers(&[])).is_ok());
        assert!(
            loopback()
                .check(&headers(&[("host", "127.0.0.1:4000")]))
                .is_ok()
        );
    }

    #[test]
    fn allows_same_origin_ui() {
        let h = headers(&[
            ("host", "127.0.0.1:4000"),
            ("origin", "http://127.0.0.1:4000"),
        ]);
        assert!(loopback().check(&h).is_ok());
    }

    #[test]
    fn allows_loopback_hosts() {
        for host in [
            "localhost:4000",
            "LOCALHOST",
            "127.0.0.1",
            "127.8.9.10:1",
            "[::1]:4000",
        ] {
            let h = HeaderMap::from_iter([(header::HOST, HeaderValue::from_static(host))]);
            assert!(loopback().check(&h).is_ok(), "{host}");
        }
    }

    #[test]
    fn allows_dev_server_origin() {
        let h = headers(&[
            ("host", "127.0.0.1:4000"),
            ("origin", "http://localhost:3000"),
        ]);
        assert!(loopback().check(&h).is_ok());
    }

    #[test]
    fn rejects_cross_origin_pages() {
        for origin in [
            "https://evil.example",
            "http://evil.example:4000",
            "null",
            "file://",
            "chrome-extension://abc",
        ] {
            let h = HeaderMap::from_iter([
                (header::HOST, HeaderValue::from_static("127.0.0.1:4000")),
                (header::ORIGIN, HeaderValue::from_static(origin)),
            ]);
            assert_eq!(loopback().check(&h), Err("origin not allowed"), "{origin}");
            assert_eq!(any_addr().check(&h), Err("origin not allowed"), "{origin}");
        }
    }

    #[test]
    fn rejects_dns_rebinding_on_loopback_bind() {
        let h = headers(&[
            ("host", "rebind.evil.example:4000"),
            ("origin", "http://rebind.evil.example:4000"),
        ]);
        assert_eq!(loopback().check(&h), Err("host not allowed"));

        let h = headers(&[("host", "rebind.evil.example:4000")]);
        assert_eq!(loopback().check(&h), Err("host not allowed"));
    }

    #[test]
    fn ipv6_loopback_bind_enforces_host() {
        let guard = RequestGuard::for_bind_addr(IpAddr::V6(Ipv6Addr::LOCALHOST));
        let h = headers(&[("host", "evil.example")]);
        assert_eq!(guard.check(&h), Err("host not allowed"));
    }

    #[test]
    fn non_loopback_bind_allows_lan_and_proxied_ui() {
        let h = headers(&[
            ("host", "192.168.1.5:4000"),
            ("origin", "http://192.168.1.5:4000"),
        ]);
        assert!(any_addr().check(&h).is_ok());

        let h = headers(&[
            ("host", "127.0.0.1:4000"),
            ("x-forwarded-host", "koharu.example.com"),
            ("origin", "https://koharu.example.com"),
        ]);
        assert!(any_addr().check(&h).is_ok());
    }
}
