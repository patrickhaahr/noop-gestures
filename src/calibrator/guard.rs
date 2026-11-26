use axum::extract::{Request, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse as _, Response};

pub async fn loopback_only(State(port): State<u16>, req: Request, next: Next) -> Response {
    if is_allowed(req.method(), req.headers(), port) {
        next.run(req).await
    } else {
        StatusCode::FORBIDDEN.into_response()
    }
}

fn is_allowed(method: &Method, headers: &HeaderMap, port: u16) -> bool {
    let header = |name| headers.get(name).and_then(|v| v.to_str().ok());
    let loopback_hosts = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host_ok = header(HOST).is_some_and(|h| loopback_hosts.iter().any(|l| l == h));
    let origin_ok = method == Method::GET
        || header(ORIGIN).is_some_and(|o| {
            loopback_hosts
                .iter()
                .any(|l| o.strip_prefix("http://") == Some(l.as_str()))
        });
    host_ok && origin_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(host: &'static str, origin: Option<&'static str>) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(HOST, HeaderValue::from_static(host));
        if let Some(origin) = origin {
            map.insert(ORIGIN, HeaderValue::from_static(origin));
        }
        map
    }

    #[test]
    fn allows_same_origin_post() {
        let h = headers("127.0.0.1:7878", Some("http://127.0.0.1:7878"));
        assert!(is_allowed(&Method::POST, &h, 7878));
    }

    #[test]
    fn rejects_rebound_host() {
        let h = headers("evil.example:7878", None);
        assert!(!is_allowed(&Method::GET, &h, 7878));
    }

    #[test]
    fn rejects_cross_site_post() {
        let h = headers("localhost:7878", Some("https://evil.example"));
        assert!(!is_allowed(&Method::POST, &h, 7878));
    }

    #[test]
    fn rejects_post_without_origin() {
        let h = headers("localhost:7878", None);
        assert!(!is_allowed(&Method::DELETE, &h, 7878));
    }
}
