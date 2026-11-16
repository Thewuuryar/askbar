//! The admin site's pages (plan step 1.16): one URL per section, each rendered from
//! `ui/templates/<slug>.html`, which extends `ui/templates/base.html`. `PAGES` is the one list of
//! them: the routes, the nav and the route-table test all come from it. A new page is one entry
//! here, one template, and one `page!` line below (Askama wants the template's file name as a literal).
//!
//! The templates hold only static text and this list (Askama escapes by default). They compile into
//! the binary and are rendered once, at start, so serving a page costs what serving a file does.
//! The static files (`components.js`, `app.js`, `audit.js`, the CSS) are served with an `ETag`, so a browser asks
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
            // The component builders (which load the constants) come before app.js, which waits for them (plan step 1.9).
            let constants = html.find(r#"<script src="/admin/components.js" defer></script>"#).expect("components.js");
            let app = html.find(r#"<script src="/admin/app.js" defer></script>"#).expect("app.js");
            assert!(constants < app, "{}: components.js before app.js", p.slug);
            assert_eq!(html.matches(r#"id="announcer""#).count(), 1, "{}: one announcer", p.slug);
            assert_eq!(html.matches(r#"id="toasts""#).count(), 1, "{}: one toast region", p.slug);
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

    /// Every icon macro in `ui/templates/icons.html`, called once each. The list below and the
    /// calls in the template are kept in step by the count check.
    const ICONS: &[&str] = &[
        "home",
        "bell",
        "audit",
        "settings",
        "reference",
        "collapse",
        "close",
        "copy",
        "help",
        "sign_out",
        "search",
        "menu",
        "check",
        "warning",
        "critical",
        "info",
        "code",
        "more",
    ];

    #[derive(Template)]
    #[template(
        source = r#"{% import "icons.html" as i %}{% call i::home() %}{% endcall %}{% call i::bell() %}{% endcall %}{% call i::audit() %}{% endcall %}{% call i::settings() %}{% endcall %}{% call i::reference() %}{% endcall %}{% call i::collapse() %}{% endcall %}{% call i::close() %}{% endcall %}{% call i::copy() %}{% endcall %}{% call i::help() %}{% endcall %}{% call i::sign_out() %}{% endcall %}{% call i::search() %}{% endcall %}{% call i::menu() %}{% endcall %}{% call i::check() %}{% endcall %}{% call i::warning() %}{% endcall %}{% call i::critical() %}{% endcall %}{% call i::info() %}{% endcall %}{% call i::code() %}{% endcall %}{% call i::more() %}{% endcall %}"#,
        ext = "html"
    )]
    struct AllIcons;

    #[test]
    fn every_icon_macro_renders_a_decorative_svg_under_csp() {
        let html = AllIcons.render().expect("icons render");
        assert_eq!(html.matches("<svg").count(), ICONS.len(), "one svg per icon in the list");
        for (name, svg) in ICONS.iter().zip(html.split_inclusive("</svg>")) {
            assert!(svg.starts_with("<svg"), "{name}: starts with an svg");
            assert!(svg.contains(r#"aria-hidden="true""#), "{name}: aria-hidden");
            assert!(svg.contains(r#"class="icon""#), "{name}: class icon");
            assert!(svg.contains(r#"stroke="currentColor""#), "{name}: currentColor");
            assert!(svg.len() > 60, "{name}: has a shape");
        }
        // CSP: no inline style attribute and no script anywhere in the icons.
        assert!(!html.contains("style="), "no inline style attribute");
        assert!(!html.contains("<script"), "no script");
    }

    /// Every macro in `ui/templates/components.html`. A macro added there without a name here fails
    /// `every_component_macro_is_listed_and_called`.
    const COMPONENTS: &[&str] = &[
        "icon",
        "skip_link",
        "announcer",
        "toast_region",
        "banner_region",
        "help",
        "page_header",
        "button",
        "icon_button",
        "tabs",
        "menu",
        "row_menu",
        "field",
        "new_secret",
        "save_bar",
        "notice",
        "status",
        "side_panel",
        "dialog_box",
        "row_confirm",
        "secret",
        "table_head",
        "empty",
        "skeleton",
        "na",
        "na_tag",
    ];

    #[derive(Template)]
    #[template(
        source = r#"{% import "components.html" as c %}<!--skip_link-->{% call c::skip_link() %}{% endcall %}<!--announcer-->{% call c::announcer() %}{% endcall %}<!--toast_region-->{% call c::toast_region() %}{% endcall %}<!--banner_region-->{% call c::banner_region() %}{% endcall %}<!--help-->{% call c::help("audit") %}{% endcall %}<!--page_header-->{% call c::page_header("Clients", "Phones and computers.", "clients", "") %}<a class="btn btn-primary">Pair</a>{% endcall %}<!--button-->{% call c::button("Save", "submit", "", "") %}{% endcall %}{% call c::button("Edit", "secondary", "s", "copy") %}{% endcall %}<!--icon_button-->{% call c::icon_button("close", "Close it") %}{% endcall %}<!--icon-->{% call c::icon("bell") %}{% endcall %}<!--tabs-->{% call c::tabs(["calls:Model calls", "admin:Admin actions"]) %}{% endcall %}<!--menu-->{% call c::menu("bell-menu") %}<button class="menu-item" role="menuitem">One</button>{% endcall %}<!--row_menu-->{% call c::row_menu(["edit:Edit", "!delete:Delete"]) %}{% endcall %}<!--field-->{% call c::field("Host name", "host", "Such as askbar.example.com.", "text", true) %}{% endcall %}{% call c::field("Pick", "pick", "", "slot", false) %}<select id="pick"></select>{% endcall %}{% call c::field("Note", "note", "", "textarea", false) %}{% endcall %}<!--new_secret-->{% call c::new_secret("New password", "pw", 10) %}{% endcall %}<!--save_bar-->{% call c::save_bar("general-form") %}{% endcall %}<!--notice-->{% call c::notice("error", "No route.", "Check the address.") %}{% endcall %}<!--status-->{% call c::status("ok") %}Running{% endcall %}{% call c::status("paused") %}Paused{% endcall %}<!--side_panel-->{% call c::side_panel("add-panel", "Add provider", "providers") %}<p>Body</p>{% endcall %}<!--dialog_box-->{% call c::dialog_box("revoke-box", "Revoke?") %}<p>Sure?</p>{% endcall %}<!--row_confirm-->{% call c::row_confirm("restart-confirm", "Restart the server?", "Restart") %}{% endcall %}<!--secret-->{% call c::secret("pair-key") %}{% endcall %}<!--table_head-->{% call c::table_head(["Name", ">Cost", ""]) %}{% endcall %}<!--empty-->{% call c::empty("Nothing yet.", "Add one") %}{% endcall %}<!--skeleton-->{% call c::skeleton(3) %}{% endcall %}<!--na-->{% call c::na("the server cannot do it") %}<span class="btn btn-secondary">Export</span>{% endcall %}<!--na_tag-->{% call c::na_tag("later") %}{% endcall %}"#,
        ext = "html"
    )]
    struct AllComponents;

    /// The part of the output for one macro: from its marker to the next.
    fn part<'a>(html: &'a str, name: &str) -> &'a str {
        let start = html.find(&format!("<!--{name}-->")).unwrap_or_else(|| panic!("{name}: no output")) + name.len() + 7;
        let rest = &html[start..];
        &rest[..rest.find("<!--").unwrap_or(rest.len())]
    }

    #[test]
    fn every_component_macro_is_listed_and_called() {
        let source = include_str!("ui/templates/components.html");
        let defined: Vec<&str> = source.split("{% macro ").skip(1).map(|m| m.split('(').next().unwrap()).collect();
        for name in &defined {
            assert!(COMPONENTS.contains(name), "macro {name} is in components.html but not in COMPONENTS (and the render test)");
        }
        for name in COMPONENTS {
            assert!(defined.contains(name), "{name} is listed but not defined in components.html");
        }
        let html = AllComponents.render().expect("components render");
        // Every macro but `icon` has its own marker in the render test; icon is exercised through the others.
        for name in COMPONENTS {
            assert!(html.contains(&format!("<!--{name}-->")), "{name}: called in the render test");
        }
    }

    #[test]
    fn components_render_what_the_plan_lists() {
        let html = AllComponents.render().expect("components render");
        assert!(!html.contains("{%") && !html.contains("{{"), "no template syntax left");
        // CSP: no inline style, script or event handler in any component.
        assert!(!html.contains("style="), "no inline style attribute");
        assert!(!html.contains("<script"), "no script");
        assert!(!html.contains(" onclick=") && !html.contains(" onload="), "no handler attributes");

        assert!(html.contains(r##"<a class="skip-link" href="#main">Skip to content</a>"##));
        let announcer = part(&html, "announcer");
        assert!(announcer.contains(r#"id="announcer" class="visually-hidden" aria-live="polite""#));
        assert!(announcer.contains(r#"id="announcer-urgent" class="visually-hidden" aria-live="assertive""#));
        assert!(part(&html, "toast_region").contains(r#"id="toasts""#));
        assert!(part(&html, "banner_region").contains(r#"id="banners""#));

        let help = part(&html, "help");
        assert!(help.contains(r#"class="btn btn-quiet btn-icon help""#) && help.contains(r#"data-help="audit""#));
        assert!(help.contains(r#"aria-label="Help""#) && help.contains(r#"data-tip="Help""#) && help.contains("<svg"));

        let header = part(&html, "page_header");
        assert!(header.contains(r#"<h1 class="page-title">Clients</h1>"#), "title");
        assert!(header.contains(r#"data-help="clients""#), "help button");
        assert!(header.contains(r#"<p class="page-lede">Phones and computers.</p>"#), "lede");
        assert!(header.contains(r#"<div class="page-actions"><a class="btn btn-primary">Pair</a></div>"#), "actions slot");

        let buttons = part(&html, "button");
        assert!(buttons.contains(r#"<button type="submit" class="btn btn-primary">Save</button>"#));
        assert!(buttons.contains(r#"class="btn btn-secondary btn-s""#) && buttons.contains("<svg"));
        let icon_button = part(&html, "icon_button");
        assert!(icon_button.contains(r#"aria-label="Close it""#) && icon_button.contains(r#"data-tip="Close it""#));

        let tabs = part(&html, "tabs");
        assert!(
            tabs.contains(r#"role="tablist""#) && tabs.contains(r#"id="tab-calls""#) && tabs.contains(r#"aria-controls="panel-admin""#)
        );
        assert_eq!(tabs.matches(r#"aria-selected="true""#).count(), 1, "one selected tab");

        let menu = part(&html, "menu");
        assert!(menu.contains(r#"id="bell-menu" class="menu" role="menu" hidden"#) && menu.contains("One"));
        let row_menu = part(&html, "row_menu");
        assert!(row_menu.contains(r#"aria-haspopup="menu""#) && row_menu.contains(r#"data-action="edit""#));
        assert!(row_menu.contains(r#"menu-item menu-item-danger" role="menuitem" data-action="delete""#), "! marks the red item");
        assert!(row_menu.find("Edit").unwrap() < row_menu.find("Delete").unwrap());

        let field = part(&html, "field");
        assert!(field.contains(r#"<label for="host">Host name <span class="optional">(optional)</span></label>"#));
        assert!(field.contains(r#"<input type="text" id="host""#) && field.contains(r#"id="host-hint">Such as askbar.example.com.</p>"#));
        assert!(field.contains(r#"id="host-error" hidden"#));
        assert!(field.contains(r#"<select id="pick"></select>"#), "slot control");
        assert!(field.contains(r#"<textarea id="note" name="note" rows="3" aria-describedby="note-error">"#), "no hint, no hint idref");
        assert!(field.contains(r#"aria-describedby="host-hint host-error""#), "the hint and the error describe the field");

        let secret = part(&html, "new_secret");
        assert_eq!(secret.matches(r#"type="password""#).count(), 2, "two fields");
        assert_eq!(secret.matches(r#"autocomplete="new-password""#).count(), 2);
        let (first, second, hint) =
            (secret.find(r#"id="pw""#).unwrap(), secret.find(r#"id="pw2""#).unwrap(), secret.find("data-length-hint").unwrap());
        assert!(first < hint && hint < second, "the length hint sits under the first field, before the second");
        assert!(secret.contains(r#"data-length-hint data-min="10">At least 10 characters</p>"#));
        assert_eq!(secret.matches(r#"minlength="10""#).count(), 2, "minlength comes from the argument");

        let bar = part(&html, "save_bar");
        assert!(bar.contains(r#"id="general-form-save-bar""#) && bar.contains("hidden") && bar.contains("Discard"));
        assert!(bar.contains(r#"type="submit" class="btn btn-primary" form="general-form""#) && bar.contains(">Save</button>"));

        let notice = part(&html, "notice");
        assert!(notice.contains(r#"notice-error" role="alert""#) && notice.contains("No route.") && notice.contains("Check the address."));
        assert!(notice.contains("<svg"), "a symbol, not colour alone");

        let status = part(&html, "status");
        assert!(status.contains("status-ok") && status.contains("<svg") && status.contains(r#"status-word">Running</span>"#));
        assert!(status.contains("status-paused") && status.contains("Paused"));

        let panel = part(&html, "side_panel");
        assert!(panel.starts_with("<aside") && panel.contains(r#"id="add-panel" class="side-panel""#) && panel.contains(" hidden>"));
        assert!(panel.contains("Add provider") && panel.contains(r#"data-help="providers""#));
        assert!(panel.contains(r#"aria-label="Close""#) && panel.contains("data-panel-close") && panel.contains("<p>Body</p>"));
        let boxed = part(&html, "dialog_box");
        assert!(boxed.starts_with("<dialog") && boxed.contains("Revoke?") && boxed.contains("<p>Sure?</p>"));
        assert!(!boxed.contains("data-help"), "no help button in a box");

        let confirm = part(&html, "row_confirm");
        assert!(
            confirm.contains("Restart the server?")
                && confirm.contains("data-row-cancel")
                && confirm.contains(r#"data-row-action>Restart</button>"#)
        );
        let secret_box = part(&html, "secret");
        assert!(secret_box.contains(r#"id="pair-key""#) && secret_box.contains(r#"data-copy="pair-key""#));

        let head = part(&html, "table_head");
        assert!(
            head.contains(r#"<th scope="col">Name</th>"#)
                && head.contains(r#"<th scope="col" class="num">Cost</th>"#)
                && head.contains("Actions")
        );
        let empty = part(&html, "empty");
        assert!(empty.contains("Nothing yet.") && empty.contains("data-empty-action>Add one</button>"));
        assert_eq!(part(&html, "skeleton").matches("skeleton-row").count(), 3);

        let na = part(&html, "na");
        assert!(na.contains(r#"aria-disabled="true""#) && na.contains(r#"tabindex="0""#), "focusable and aria-disabled");
        assert!(!na.contains(" disabled"), "never the disabled attribute");
        assert!(na.contains("Not available yet: the server cannot do it") && na.contains("Export"));
        let tag = part(&html, "na_tag");
        assert!(tag.contains("Not available yet") && tag.contains("later") && !tag.contains(" disabled"));
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
