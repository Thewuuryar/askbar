//! The admin site's pages (plan step 1.16): one URL per section, each rendered from
//! `ui/templates/<slug>.html`, which extends `ui/templates/base.html`. `PAGES` is the one list of
//! them: the routes, the nav and the route-table test all come from it. A new page is one entry
//! here, one template, and one `page!` line below (Askama wants the template's file name as a literal).
//!
//! The templates hold only static text and this list (Askama escapes by default). They compile into
//! the binary and are rendered once, at start, so serving a page costs what serving a file does.
//! The static files (`app.js`, `audit.js`, the CSS) are served with an `ETag`, so a browser asks
//! once and then gets 304s.

use askama::Template;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// One admin page: its path segment (`/admin/<slug>/`), its nav label and heading, and how to render it.
pub struct Page {
    pub slug: &'static str,
    pub label: &'static str,
    render: fn() -> askama::Result<String>,
}

macro_rules! page {
    ($ty:ident, $render:ident, $slug:literal, $file:literal, $label:literal) => {
        #[derive(Template)]
        #[template(path = $file)]
        struct $ty {
            pages: &'static [Page],
            current: &'static str,
            label: &'static str,
        }
        fn $render() -> askama::Result<String> {
            $ty { pages: PAGES, current: $slug, label: $label }.render()
        }
    };
}

page!(Providers, providers, "providers", "providers.html", "Providers");
page!(Services, services, "services", "services.html", "Services");
page!(Clients, clients, "clients", "clients.html", "Clients");
page!(Audit, audit, "audit", "audit.html", "Audit");
page!(Backup, backup, "backup", "backup.html", "Backup");
page!(Restore, restore, "restore", "restore.html", "Restore");
page!(Network, network, "network", "network.html", "Network");
page!(Security, security, "security", "security.html", "Security");
page!(Settings, settings, "settings", "settings.html", "Settings");

/// Every admin page, in nav order. `/admin` and `/admin/` go to the first.
pub static PAGES: &[Page] = &[
    Page { slug: "providers", label: "Providers", render: providers },
    Page { slug: "services", label: "Services", render: services },
    Page { slug: "clients", label: "Clients", render: clients },
    Page { slug: "audit", label: "Audit", render: audit },
    Page { slug: "backup", label: "Backup", render: backup },
    Page { slug: "restore", label: "Restore", render: restore },
    Page { slug: "network", label: "Network", render: network },
    Page { slug: "security", label: "Security", render: security },
    Page { slug: "settings", label: "Settings", render: settings },
];

/// The path of a page: `/admin/<slug>/`.
pub fn path(page: &Page) -> String {
    format!("/admin/{}/", page.slug)
}

/// Each page's HTML, rendered once: `routes` asks for it as the router is built, so a template error
/// stops the admin at start rather than on a request.
pub fn rendered() -> &'static [String] {
    static HTML: OnceLock<Vec<String>> = OnceLock::new();
    HTML.get_or_init(|| PAGES.iter().map(|p| (p.render)().unwrap_or_else(|e| panic!("admin page {}: {e}", p.slug))).collect())
}

/// The response for page number `i` of `PAGES`. Pages are never cached (`no-store` from
/// `web::security_headers`): they decide nothing, but they are what a person reloads to see changes.
pub fn page_response(i: usize) -> Response {
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], rendered()[i].as_str()).into_response()
}

/// A static file, with an `ETag` (the start of its SHA-256) and `Cache-Control: no-cache`: the
/// browser keeps it and asks each time, and an unchanged file costs a 304.
pub fn static_file(headers: &HeaderMap, content_type: &'static str, body: &'static str) -> Response {
    let etag = etag_of(body);
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == etag || t.trim() == "*"));
    let mut res =
        if fresh { StatusCode::NOT_MODIFIED.into_response() } else { ([(header::CONTENT_TYPE, content_type)], body).into_response() };
    let h = res.headers_mut();
    h.insert(header::ETAG, HeaderValue::from_str(&etag).expect("hex is a valid header"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    res
}

fn etag_of(body: &str) -> String {
    let digest = Sha256::digest(body.as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_page_renders_with_the_nav_marking_it_current() {
        for (i, p) in PAGES.iter().enumerate() {
            let html = &rendered()[i];
            assert!(html.contains(&format!(r#"<body data-page="{}">"#, p.slug)), "{}", p.slug);
            assert!(html.contains(&format!(r#"<section id="tab-{}" data-tab="{}">"#, p.slug, p.slug)), "{}", p.slug);
            assert!(html.contains(&format!(r#"<a href="/admin/{}/" data-tab="{}" aria-current="page">{}</a>"#, p.slug, p.slug, p.label)));
            assert_eq!(html.matches(r#"aria-current="page""#).count(), 1, "{}: one current page", p.slug);
            for other in PAGES {
                assert!(html.contains(&format!(r#"<a href="/admin/{}/""#, other.slug)), "{} links {}", p.slug, other.slug);
            }
            // The same frame on every page: sign-in, first run, banners, sign-out.
            for id in ["login", "setup", "offline", "flash", "logout", "app"] {
                assert!(html.contains(&format!(r#"id="{id}""#)), "{}: #{id}", p.slug);
            }
            assert!(html.contains(r#"<script src="/admin/app.js" defer></script>"#));
            assert_eq!(html.contains("/admin/audit.js"), p.slug == "audit", "{}: audit.js only on the Audit page", p.slug);
            assert!(!html.contains("{%") && !html.contains("{{"), "{}: no template syntax left", p.slug);
        }
    }

    #[test]
    fn slugs_are_unique_plain_path_segments() {
        let mut seen = std::collections::HashSet::new();
        for p in PAGES {
            assert!(!p.slug.is_empty() && p.slug.bytes().all(|b| b.is_ascii_lowercase()), "{}", p.slug);
            assert!(seen.insert(p.slug), "{} twice", p.slug);
        }
    }

    #[test]
    fn a_static_file_answers_304_when_unchanged() {
        let first = static_file(&HeaderMap::new(), "text/css", "body {}");
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first.headers()[header::ETAG].clone();
        assert_eq!(first.headers()[header::CACHE_CONTROL], "no-cache");
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, etag.clone());
        assert_eq!(static_file(&h, "text/css", "body {}").status(), StatusCode::NOT_MODIFIED);
        assert_eq!(static_file(&h, "text/css", "body { color: red }").status(), StatusCode::OK, "a changed file is sent again");
    }
}
