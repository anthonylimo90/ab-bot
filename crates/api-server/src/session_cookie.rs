//! The dashboard's session cookie.
//!
//! A browser (a request carrying [`BROWSER_HEADER`]) gets its session token
//! in an `HttpOnly` cookie that scripts on the page cannot read, and never in
//! a response body. API clients keep sending `Authorization: Bearer`.
//!
//! The cookie is `SameSite=Strict`, so another site's page cannot send it;
//! and a state-changing request authenticated by it must also carry
//! [`BROWSER_HEADER`], which a cross-site form cannot set and a cross-site
//! script cannot send without the API's CORS consent.

use axum::http::{header, HeaderMap, HeaderValue, Method};

/// The session cookie's name.
pub const SESSION_COOKIE: &str = "abbot_session";
/// The header the dashboard sends on every request.
pub const BROWSER_HEADER: &str = "x-requested-with";
/// Its value.
pub const BROWSER_HEADER_VALUE: &str = "ab-bot-dashboard";

/// Where a request's session token came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Bearer,
    Cookie,
}

/// Whether the request comes from the dashboard.
pub fn is_browser(headers: &HeaderMap) -> bool {
    headers
        .get(BROWSER_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == BROWSER_HEADER_VALUE)
}

/// The request's session token: the `Authorization: Bearer` header when
/// present, else the session cookie. `Err` for a malformed header.
pub fn request_token(headers: &HeaderMap) -> Result<Option<(String, TokenSource)>, &'static str> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        let value = value
            .to_str()
            .map_err(|_| "Invalid authorization header encoding")?;
        let token = value
            .strip_prefix("Bearer ")
            .ok_or("Invalid authorization format, expected 'Bearer <token>'")?;
        return Ok(Some((token.to_string(), TokenSource::Bearer)));
    }
    Ok(cookie_value(headers, SESSION_COOKIE).map(|token| (token, TokenSource::Cookie)))
}

/// Whether a request authenticated from `source` may go ahead: one made with
/// the cookie that changes state must come from the dashboard, from one of
/// `origins` (its `Origin` header; `None` takes any). The origin check stops
/// a page on a sibling subdomain, which counts as the same site and so
/// sends the `SameSite=Strict` cookie.
pub fn csrf_ok(
    method: &Method,
    source: TokenSource,
    headers: &HeaderMap,
    origins: Option<&[String]>,
) -> bool {
    if source == TokenSource::Bearer
        || matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
    {
        return true;
    }
    if !is_browser(headers) {
        return false;
    }
    let Some(origins) = origins else {
        return true;
    };
    headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| origins.iter().any(|allowed| allowed == origin))
}

/// `Set-Cookie` for a new session token that lasts `max_age_secs`.
pub fn set_cookie(token: &str, max_age_secs: i64, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{secure}",
        max_age_secs.max(0)
    ))
    .expect("a JWT is a valid header value")
}

/// `Set-Cookie` that removes the session cookie.
pub fn clear_cookie(secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}"
    ))
    .expect("a valid header value")
}

/// Hand a new session token to the client that asked for it: to the
/// dashboard in the cookie, leaving `token` empty for the response body; to
/// an API client in the body. Returns the headers to send with it.
pub fn deliver(
    state: &crate::state::AppState,
    request: &HeaderMap,
    token: &mut String,
) -> HeaderMap {
    let mut response = HeaderMap::new();
    if !is_browser(request) {
        return response;
    }
    let max_age = state
        .jwt_auth
        .validate_token(token)
        .map(|claims| claims.exp - chrono::Utc::now().timestamp())
        .unwrap_or(0);
    response.insert(
        header::SET_COOKIE,
        set_cookie(token, max_age, state.secure_cookies),
    );
    token.clear();
    response
}

/// The value of cookie `name`, if the request carries it.
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn the_bearer_header_wins_over_the_cookie() {
        let both = headers(&[
            ("authorization", "Bearer from-header"),
            ("cookie", "theme=dark; abbot_session=from-cookie"),
        ]);
        assert_eq!(
            request_token(&both).unwrap(),
            Some(("from-header".into(), TokenSource::Bearer))
        );
        let cookie = headers(&[("cookie", "theme=dark; abbot_session=from-cookie; x=y")]);
        assert_eq!(
            request_token(&cookie).unwrap(),
            Some(("from-cookie".into(), TokenSource::Cookie))
        );
        assert_eq!(
            request_token(&headers(&[("cookie", "abbot_session=")])).unwrap(),
            None
        );
        assert!(request_token(&headers(&[("authorization", "Basic x")])).is_err());
        // Another cookie that merely ends in the name does not count.
        assert_eq!(
            request_token(&headers(&[("cookie", "not_abbot_session=x")])).unwrap(),
            None
        );
    }

    #[test]
    fn a_cookie_session_changes_state_only_from_the_dashboard() {
        let plain = HeaderMap::new();
        let dashboard = headers(&[
            (BROWSER_HEADER, BROWSER_HEADER_VALUE),
            ("origin", "https://app.example.com"),
        ]);
        let allowed = vec!["https://app.example.com".to_string()];
        let only = Some(allowed.as_slice());
        assert!(csrf_ok(&Method::GET, TokenSource::Cookie, &plain, only));
        assert!(!csrf_ok(&Method::POST, TokenSource::Cookie, &plain, only));
        assert!(!csrf_ok(&Method::DELETE, TokenSource::Cookie, &plain, None));
        assert!(csrf_ok(
            &Method::POST,
            TokenSource::Cookie,
            &dashboard,
            only
        ));
        assert!(csrf_ok(&Method::POST, TokenSource::Bearer, &plain, only));
        let wrong = headers(&[(BROWSER_HEADER, "XMLHttpRequest")]);
        assert!(!csrf_ok(&Method::PUT, TokenSource::Cookie, &wrong, None));
        // A sibling subdomain sends the cookie and can set the header, but
        // not its Origin.
        let sibling = headers(&[
            (BROWSER_HEADER, BROWSER_HEADER_VALUE),
            ("origin", "https://evil.example.com"),
        ]);
        assert!(!csrf_ok(&Method::POST, TokenSource::Cookie, &sibling, only));
        let no_origin = headers(&[(BROWSER_HEADER, BROWSER_HEADER_VALUE)]);
        assert!(!csrf_ok(
            &Method::POST,
            TokenSource::Cookie,
            &no_origin,
            only
        ));
        // Development without a list takes any origin.
        assert!(csrf_ok(&Method::POST, TokenSource::Cookie, &sibling, None));
    }

    #[test]
    fn the_cookie_is_http_only_and_strict() {
        let set = set_cookie("tok", 3600, true);
        let set = set.to_str().unwrap();
        assert!(set.starts_with("abbot_session=tok;"));
        for part in [
            "HttpOnly",
            "SameSite=Strict",
            "Max-Age=3600",
            "Secure",
            "Path=/",
        ] {
            assert!(set.contains(part), "{set}");
        }
        assert!(!set_cookie("tok", 10, false)
            .to_str()
            .unwrap()
            .contains("Secure"));
        assert!(clear_cookie(true).to_str().unwrap().contains("Max-Age=0"));
    }
}
