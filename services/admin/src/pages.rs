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

/// The menu group a page sits in (rule 1). Top-level pages have no parent; the others sit under
/// their group's heading in the side menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Top,
    Audit,
    Settings,
    Reference,
}

impl Group {
    /// The group's name, as the menu and the breadcrumb show it.
    pub fn label(self) -> &'static str {
        match self {
            Group::Top => "",
            Group::Audit => "Audit",
            Group::Settings => "Settings",
            Group::Reference => "Reference",
        }
    }
}

/// One admin page: its path (`/admin/<slug>/`, where a slug may have one `/`), its menu label and
/// heading, its group and icon (an icon macro's name), and how to render it.
pub struct Page {
    pub slug: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub icon: &'static str,
    render: fn() -> askama::Result<String>,
}

macro_rules! page {
    ($ty:ident, $render:ident, $slug:literal, $file:literal, $label:literal) => {
        #[derive(Template)]
        #[template(path = $file)]
        struct $ty {
            current: &'static str,
            label: &'static str,
            /// The group's name for the breadcrumb; empty for a page that is its own group.
            group: &'static str,
            /// The page's section id: the slug with `/` as `-` (`tab-audit-calls`).
            section: String,
            /// The side menu (rule 1), built from `PAGES`.
            menu: &'static [MenuGroup],
            /// The tab bar's pages (plan step 6.2): the page's group when it has one, else none.
            tabs: Vec<&'static Page>,
        }
        fn $render() -> askama::Result<String> {
            $ty {
                current: $slug,
                label: $label,
                group: crumb_group($slug),
                section: section_id($slug),
                menu: menu(),
                tabs: tab_pages($slug),
            }
            .render()
        }
    };
}

/// The breadcrumb's first item (rule 3): the page's group label, or empty when the page is top-level or
/// is named like its group (Settings, Reference), where the breadcrumb shows just the page.
pub fn crumb_group(slug: &str) -> &'static str {
    match PAGES.iter().find(|p| p.slug == slug) {
        Some(p) if p.group.label() != p.label => p.group.label(),
        _ => "",
    }
}

/// The tab bar of a page (plan step 6.2): every page of its group, in menu order, when the group has a tab bar
/// (Audit). Labels and paths come from `PAGES`, so the tabs can only match the menu.
pub fn tab_pages(slug: &str) -> Vec<&'static Page> {
    match PAGES.iter().find(|p| p.slug == slug) {
        Some(p) if p.group == Group::Audit => PAGES.iter().filter(|q| q.group == Group::Audit).collect(),
        _ => Vec::new(),
    }
}

/// The id of a page's `<section>`: `tab-` and the slug, with `/` as `-`.
pub fn section_id(slug: &str) -> String {
    format!("tab-{}", slug.replace('/', "-"))
}

page!(Home, home, "home", "home.html", "Home");
page!(Alerts, alerts, "alerts", "alerts.html", "Alerts");
page!(AuditCalls, audit_calls, "audit/calls", "audit_calls.html", "Model calls");
page!(AuditAdmin, audit_admin, "audit/admin", "audit_admin.html", "Admin actions");
page!(AuditSealed, audit_sealed, "audit/sealed", "audit_sealed.html", "Sealed access");
page!(AuditRouting, audit_routing, "audit/routing", "audit_routing.html", "Routing");
page!(Settings, settings, "settings", "settings.html", "Settings");
page!(Providers, providers, "providers", "providers.html", "Providers");
page!(People, people, "people", "people.html", "People & clients");
page!(Services, services, "services", "services.html", "Services");
page!(Network, network, "network", "network.html", "Network");
page!(Security, security, "security", "security.html", "Security");
page!(Backup, backup, "backup", "backup.html", "Backup");
page!(Notifications, notifications, "notifications", "notifications.html", "Notifications");
page!(General, general, "general", "general.html", "General");
page!(Reference, reference, "reference", "reference.html", "Reference");

/// Every admin page, in menu order (rule 1). Its slugs equal `constants::PAGE_SLUGS`, which a test checks.
/// `/admin` and `/admin/` go to `constants::LANDING_PAGE`.
pub static PAGES: &[Page] = &[
    Page { slug: "home", label: "Home", group: Group::Top, icon: "home", render: home },
    Page { slug: "alerts", label: "Alerts", group: Group::Top, icon: "bell", render: alerts },
    Page { slug: "audit/calls", label: "Model calls", group: Group::Audit, icon: "calls", render: audit_calls },
    Page { slug: "audit/admin", label: "Admin actions", group: Group::Audit, icon: "clipboard", render: audit_admin },
    Page { slug: "audit/sealed", label: "Sealed access", group: Group::Audit, icon: "lock", render: audit_sealed },
    Page { slug: "audit/routing", label: "Routing", group: Group::Audit, icon: "route", render: audit_routing },
    Page { slug: "settings", label: "Settings", group: Group::Settings, icon: "settings", render: settings },
    Page { slug: "providers", label: "Providers", group: Group::Settings, icon: "cube", render: providers },
    Page { slug: "people", label: "People & clients", group: Group::Settings, icon: "people", render: people },
    Page { slug: "services", label: "Services", group: Group::Settings, icon: "server", render: services },
    Page { slug: "network", label: "Network", group: Group::Settings, icon: "globe", render: network },
    Page { slug: "security", label: "Security", group: Group::Settings, icon: "shield", render: security },
    Page { slug: "backup", label: "Backup", group: Group::Settings, icon: "archive", render: backup },
    Page { slug: "notifications", label: "Notifications", group: Group::Settings, icon: "send", render: notifications },
    Page { slug: "general", label: "General", group: Group::Settings, icon: "sliders", render: general },
    Page { slug: "reference", label: "Reference", group: Group::Reference, icon: "reference", render: reference },
];

/// A page is its group's overview when its slug is the group's name in lower case (`settings` in Settings).
/// Its heading links to it and it is the heading itself in the menu, not a sub-item (plan step 5.4).
pub fn is_overview(p: &Page) -> bool {
    p.slug == p.group.label().to_lowercase()
}

/// One entry of the side menu: a top-level page (Home, Alerts, Reference), or a group heading (Audit,
/// Settings) with its pages under it. A group's heading links to its first page; when that is the group's
/// overview, the heading is the overview's link and the other pages are its sub-items.
pub struct MenuGroup {
    pub label: &'static str,
    pub icon: &'static str,
    /// True for Audit and Settings, whose pages are listed under the heading.
    pub nested: bool,
    /// True when the first page is the group's overview (see `is_overview`).
    pub overview: bool,
    pub pages: Vec<&'static Page>,
}

impl MenuGroup {
    /// Where the heading links: its first page.
    pub fn href(&self) -> String {
        path(self.pages[0])
    }
    /// The overview's slug, or empty when the group has none. The heading carries it as its `data-tab`.
    pub fn overview_slug(&self) -> &'static str {
        if self.overview {
            self.pages[0].slug
        } else {
            ""
        }
    }
    /// The pages listed under the heading: all of them, except an overview, which the heading already is.
    pub fn subs(&self) -> &[&'static Page] {
        if self.overview {
            &self.pages[1..]
        } else {
            &self.pages
        }
    }
    /// Whether `slug` is one of this group's pages (its heading is then bold, rule 1).
    pub fn holds(&self, slug: &str) -> bool {
        self.pages.iter().any(|p| p.slug == slug)
    }
}

/// The pages of the Settings group that are sections (not its overview), in menu order: one row each on the
/// Settings overview.
pub fn settings_sections() -> Vec<&'static Page> {
    PAGES.iter().filter(|p| p.group == Group::Settings && !is_overview(p)).collect()
}

/// The side menu, built once from `PAGES`: each Top or Reference page on its own, and each Audit or
/// Settings run of pages under its group's heading.
pub fn menu() -> &'static [MenuGroup] {
    static MENU: OnceLock<Vec<MenuGroup>> = OnceLock::new();
    MENU.get_or_init(|| {
        let mut out: Vec<MenuGroup> = Vec::new();
        for p in PAGES {
            let nested = matches!(p.group, Group::Audit | Group::Settings);
            match out.last_mut() {
                Some(g) if nested && g.nested && g.label == p.group.label() => g.pages.push(p),
                _ if nested => out.push(MenuGroup {
                    label: p.group.label(),
                    icon: match p.group {
                        Group::Audit => "audit",
                        _ => "settings",
                    },
                    nested,
                    overview: is_overview(p),
                    pages: vec![p],
                }),
                _ => out.push(MenuGroup { label: p.label, icon: p.icon, nested, overview: false, pages: vec![p] }),
            }
        }
        out
    })
}

/// The first letter of a name, upper case, for the menu's avatar.
pub fn initial(name: &str) -> String {
    name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default()
}

/// The page `/admin` opens.
pub fn landing() -> &'static Page {
    PAGES.iter().find(|p| p.slug == crate::constants::LANDING_PAGE).expect("the landing page is in PAGES")
}

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
            assert!(html.contains(&format!(r#"<section id="{}" data-tab="{}">"#, section_id(p.slug), p.slug)), "{}", p.slug);
            // The side menu (plan step 2.2, rule 1): one link per page, the current one marked, its group's
            // heading bold, and no other heading.
            let nav = &html[html.find(r#"<nav id="nav""#).expect("nav")..];
            let nav = &nav[..nav.find("</nav>").unwrap()];
            assert_eq!(nav.matches(r#"aria-current="page""#).count(), 1, "{}: one current page", p.slug);
            // The breadcrumb (plan step 3.1) holds the one other `aria-current="page"`: the page's own label. An Audit
            // page's tab bar (plan step 6.2) holds one more, its own tab.
            let tab_bar = usize::from(!tab_pages(p.slug).is_empty());
            assert_eq!(
                html.matches(r#"aria-current="page""#).count(),
                2 + tab_bar,
                "{}: the menu's link, the breadcrumb and its tab, nothing else",
                p.slug
            );
            let crumb = &html[html.find(r#"<nav class="crumb" aria-label="Breadcrumb">"#).expect("breadcrumb")..];
            let crumb = &crumb[..crumb.find("</nav>").unwrap()];
            // Labels are HTML-escaped in the page ("People & clients" is written `People &#38; clients`).
            let label = p.label.replace('&', "&#38;");
            let expect = match crumb_group(p.slug) {
                "" => format!(r#"<ol><li aria-current="page">{label}</li></ol>"#),
                g => format!(r#"<ol><li>{}</li><li aria-current="page">{label}</li></ol>"#, g),
            };
            assert!(crumb.contains(&expect), "{}: breadcrumb is {}", p.slug, expect);
            assert_eq!(
                crumb_group(p.slug).is_empty(),
                matches!(p.group, Group::Top | Group::Reference) || p.group.label() == p.label,
                "{}",
                p.slug
            );
            // One status bar, holding the budget as a greyed item and the kill switch as a word with a symbol, once each.
            assert_eq!(html.matches(r#"id="status-bar""#).count(), 1, "{}", p.slug);
            assert_eq!(
                html.matches("the server has no family budget setting").count(),
                2,
                "{}: the budget, once (tooltip and text)",
                p.slug
            );
            // The budget reads "Budget not set": the space is a no-break one, since the greyed item is a flex box,
            // which drops a plain space at its end.
            assert_eq!(html.matches("Budget&nbsp;<b>not set</b>").count(), 1, "{}: the budget text keeps its space", p.slug);
            assert_eq!(html.matches(r#"id="sb-kill""#).count(), 1, "{}", p.slug);
            // The seal toggle (step 3.3) sits in the facts: a status word and one named button, and the unseal box exists once.
            let facts = &html[html.find(r#"id="status-facts""#).unwrap()..];
            let facts = &facts[..facts.find(r#"id="banners""#).unwrap()];
            assert_eq!(facts.matches(r#"id="sb-seal-status""#).count(), 1, "{}", p.slug);
            assert_eq!(facts.matches(r#"id="seal-button""#).count(), 1, "{}", p.slug);
            assert_eq!(facts.matches(r#"id="bell-button""#).count(), 1, "{}: one bell", p.slug);
            assert!(
                facts.contains(r#"aria-label="Alerts: none""#) && facts.contains(r#"id="bell-menu""#),
                "{}: the bell has a name and a menu",
                p.slug
            );
            assert_eq!(html.matches(r#"id="alert-pops""#).count(), 1, "{}: one pop-up region", p.slug);
            // Global search (step 3.5): one combobox with its listbox, after the breadcrumb and before the facts, with a name.
            let bar = &html[html.find(r#"id="status-bar""#).unwrap()..html.find(r#"id="status-facts""#).unwrap()];
            assert!(bar.find("Breadcrumb").unwrap() < bar.find(r#"id="gs-input""#).unwrap(), "{}: search follows the breadcrumb", p.slug);
            assert_eq!(html.matches(r#"id="gs-input""#).count(), 1, "{}: one search field", p.slug);
            assert!(
                bar.contains(r#"role="combobox" aria-expanded="false" aria-controls="gs-list" aria-autocomplete="list""#)
                    && bar.contains(r#"<label class="gs-sr" for="gs-input">Search the admin</label>"#)
                    && bar.contains(r#"id="gs-list" role="listbox""#),
                "{}: combobox, name and listbox",
                p.slug
            );
            assert_eq!(html.matches(r#"id="unseal-box""#).count(), 1, "{}", p.slug);
            assert_eq!(html.matches(r#"id="unseal-pass-error""#).count(), 1, "{}", p.slug);
            let current = format!(r#"href="/admin/{}/" data-tab="{}" data-tip="{}" aria-current="page">"#, p.slug, p.slug, label);
            assert!(nav.contains(&current), "{}: its link is current", p.slug);
            for other in PAGES {
                assert_eq!(nav.matches(&format!(r#"data-tab="{}""#, other.slug)).count(), 1, "{} links {} once", p.slug, other.slug);
            }
            let parents = nav.matches("sm-item sm-parent").count();
            match p.group {
                Group::Audit | Group::Settings => {
                    assert_eq!(parents, 1, "{}: its group's heading is bold", p.slug);
                    let heading = &nav[nav.find("sm-item sm-parent").unwrap()..];
                    let heading = &heading[..heading.find("</a>").unwrap()];
                    assert!(heading.contains(&format!(">{}</span>", p.group.label())), "{}: the heading is {}", p.slug, p.group.label());
                    // Plan step 5.4: a group whose first page is its overview (Settings) links its heading to that page,
                    // which is then the current item, and no sub-item lists it again. Other groups' headings are never current.
                    assert_eq!(
                        heading.contains(r#"aria-current="page""#),
                        is_overview(p),
                        "{}: the heading is current only on the overview",
                        p.slug
                    );
                    if is_overview(p) {
                        assert!(
                            heading.contains(&format!(
                                r#"href="/admin/{}/" data-tab="{}" data-tip="Settings" aria-current="page">"#,
                                p.slug, p.slug
                            )),
                            "{}: the overview's heading is its link, current",
                            p.slug
                        );
                        // Every nested page that is not an overview is a sub-item: the Audit pages and the Settings sections.
                        let subs = PAGES.iter().filter(|q| matches!(q.group, Group::Audit | Group::Settings) && !is_overview(q)).count();
                        assert_eq!(nav.matches("sm-item sm-sub").count(), subs, "{}: every page but the overview is a sub-item", p.slug);
                        assert_eq!(
                            nav.matches(&format!(r#"href="/admin/{}/" data-tab="{}" data-tip="Settings""#, p.slug, p.slug)).count(),
                            1,
                            "{}: listed once",
                            p.slug
                        );
                        assert!(!nav.contains(&format!(r#"sm-sub" href="/admin/{}/""#, p.slug)), "{}: not a sub-item too", p.slug);
                    }
                }
                _ => assert_eq!(parents, 0, "{}: no heading is bold", p.slug),
            }
            assert!(!html.contains(r#"class="top""#), "{}: the old top nav is gone", p.slug);
            // The licence and source link come from the constants; the icon buttons have names and tooltips.
            assert!(html.contains(&format!(
                r#"href="{}" rel="noreferrer">{}</a>"#,
                crate::constants::LICENSE_URL,
                crate::constants::LICENSE
            )));
            assert!(html.contains(&format!(
                r#"href="{}" rel="noreferrer" aria-label="Source code" data-tip="Source code""#,
                crate::constants::SOURCE_URL
            )));
            for name in ["Collapse menu", "Sign out"] {
                assert!(html.contains(&format!(r#"aria-label="{name}" data-tip="{name}""#)), "{}: {name}", p.slug);
            }
            // The same frame on every page: sign-in, first run, the banner area, the page-level error line, sign-out.
            for id in ["login", "setup", "banners", "page-error", "logout", "app"] {
                assert!(html.contains(&format!(r#"id="{id}""#)), "{}: #{id}", p.slug);
            }
            // The old flash banners are gone: banners and toasts replace them.
            for id in ["offline", "rejected", "chain-alert", "alerts-banner", "unsealed-banner", "sealed-alerts", "flash"] {
                assert!(!html.contains(&format!(r#"id="{id}""#)), "{}: old #{id}", p.slug);
            }
            assert_eq!(html.matches(r#"id="banners""#).count(), 1, "{}: one banner area", p.slug);
            // The component builders (which load the constants) come before app.js, which waits for them (plan step 1.9).
            let constants = html.find(r#"<script src="/admin/components.js" defer></script>"#).expect("components.js");
            let app = html.find(r#"<script src="/admin/app.js" defer></script>"#).expect("app.js");
            assert!(constants < app, "{}: components.js before app.js", p.slug);
            assert_eq!(html.matches(r#"id="announcer""#).count(), 1, "{}: one announcer", p.slug);
            assert_eq!(html.matches(r#"id="toasts""#).count(), 1, "{}: one toast region", p.slug);
            // Model calls has its own script (plan step 6.1); Sealed access and Routing share audit.js (plan step 6.2).
            assert_eq!(
                html.contains("/admin/audit.js"),
                ["audit/sealed", "audit/routing"].contains(&p.slug),
                "{}: audit.js only on Sealed access and Routing",
                p.slug
            );
            assert!(!html.contains("subnav") && !html.contains("audit-nav"), "{}: no in-page sub-nav (plan step 6.2)", p.slug);
            assert_eq!(html.contains("/admin/audit_calls.js"), p.slug == "audit/calls", "{}: audit_calls.js only on Model calls", p.slug);
            assert!(!html.contains("{%") && !html.contains("{{"), "{}: no template syntax left", p.slug);
        }
    }

    #[test]
    fn slugs_are_unique_plain_path_segments() {
        let mut seen = std::collections::HashSet::new();
        for p in PAGES {
            let parts: Vec<&str> = p.slug.split('/').collect();
            assert!(parts.len() <= 2, "{}: at most one /", p.slug);
            for part in parts {
                assert!(!part.is_empty() && part.bytes().all(|b| b.is_ascii_lowercase()), "{}", p.slug);
            }
            assert!(seen.insert(p.slug), "{} twice", p.slug);
        }
    }

    #[test]
    fn pages_match_the_constants_and_sit_in_their_groups() {
        let slugs: Vec<&str> = PAGES.iter().map(|p| p.slug).collect();
        assert_eq!(slugs, crate::constants::PAGE_SLUGS, "PAGES and constants::PAGE_SLUGS list the same pages in the same order");
        assert_eq!(landing().slug, crate::constants::LANDING_PAGE);
        // Home is the first page and the landing page: it sits alone in the Top group (plan step 4.1).
        assert_eq!(PAGES[0].slug, "home");
        assert_eq!(PAGES[0].group, Group::Top);
        // Alerts follows Home, also on its own in the Top group (plan step 4.4).
        assert_eq!((PAGES[1].slug, PAGES[1].group, PAGES[1].icon), ("alerts", Group::Top, "bell"));
        for p in PAGES {
            // An Audit page lives under /admin/audit/; nothing else does.
            assert_eq!(p.group == Group::Audit, p.slug.starts_with("audit/"), "{}", p.slug);
            assert!(ICONS.contains(&p.icon), "{}: icon {} is an icon macro", p.slug, p.icon);
        }
        // Rule 1: the groups come in menu order, each in one run.
        let order = [Group::Top, Group::Audit, Group::Settings, Group::Reference];
        let ranks: Vec<usize> = PAGES.iter().map(|p| order.iter().position(|g| *g == p.group).unwrap()).collect();
        assert!(ranks.windows(2).all(|w| w[0] <= w[1]), "groups in menu order: {ranks:?}");
        let audit: Vec<&str> = PAGES.iter().filter(|p| p.group == Group::Audit).map(|p| p.label).collect();
        assert_eq!(audit, ["Model calls", "Admin actions", "Sealed access", "Routing"]);
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
        "logo",
        "calls",
        "clipboard",
        "lock",
        "route",
        "cube",
        "people",
        "server",
        "globe",
        "shield",
        "archive",
        "send",
        "sliders",
    ];

    #[derive(Template)]
    #[template(
        source = r#"{% import "icons.html" as i %}{% call i::home() %}{% endcall %}{% call i::bell() %}{% endcall %}{% call i::audit() %}{% endcall %}{% call i::settings() %}{% endcall %}{% call i::reference() %}{% endcall %}{% call i::collapse() %}{% endcall %}{% call i::close() %}{% endcall %}{% call i::copy() %}{% endcall %}{% call i::help() %}{% endcall %}{% call i::sign_out() %}{% endcall %}{% call i::search() %}{% endcall %}{% call i::menu() %}{% endcall %}{% call i::check() %}{% endcall %}{% call i::warning() %}{% endcall %}{% call i::critical() %}{% endcall %}{% call i::info() %}{% endcall %}{% call i::code() %}{% endcall %}{% call i::more() %}{% endcall %}{% call i::logo() %}{% endcall %}{% call i::calls() %}{% endcall %}{% call i::clipboard() %}{% endcall %}{% call i::lock() %}{% endcall %}{% call i::route() %}{% endcall %}{% call i::cube() %}{% endcall %}{% call i::people() %}{% endcall %}{% call i::server() %}{% endcall %}{% call i::globe() %}{% endcall %}{% call i::shield() %}{% endcall %}{% call i::archive() %}{% endcall %}{% call i::send() %}{% endcall %}{% call i::sliders() %}{% endcall %}"#,
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
        source = r#"{% import "components.html" as c %}<!--skip_link-->{% call c::skip_link() %}{% endcall %}<!--announcer-->{% call c::announcer() %}{% endcall %}<!--toast_region-->{% call c::toast_region() %}{% endcall %}<!--banner_region-->{% call c::banner_region() %}{% endcall %}<!--help-->{% call c::help("audit") %}{% endcall %}<!--page_header-->{% call c::page_header("Clients", "Phones and computers.", "clients", "") %}<a class="btn btn-primary">Pair</a>{% endcall %}<!--button-->{% call c::button("Save", "submit", "", "") %}{% endcall %}{% call c::button("Edit", "secondary", "s", "copy") %}{% endcall %}<!--icon_button-->{% call c::icon_button("close", "Close it") %}{% endcall %}<!--icon-->{% call c::icon("bell") %}{% endcall %}<!--tabs-->{% call c::tabs(&crate::pages::tab_pages("audit/calls"), "audit/calls", "Audit") %}{% endcall %}<!--menu-->{% call c::menu("bell-menu") %}<button class="menu-item" role="menuitem">One</button>{% endcall %}<!--row_menu-->{% call c::row_menu(["edit:Edit", "!delete:Delete"]) %}{% endcall %}<!--field-->{% call c::field("Host name", "host", "Such as askbar.example.com.", "text", true) %}{% endcall %}{% call c::field("Pick", "pick", "", "slot", false) %}<select id="pick"></select>{% endcall %}{% call c::field("Note", "note", "", "textarea", false) %}{% endcall %}<!--new_secret-->{% call c::new_secret("New password", "pw", 10) %}{% endcall %}<!--save_bar-->{% call c::save_bar("general-form") %}{% endcall %}<!--notice-->{% call c::notice("error", "No route.", "Check the address.") %}{% endcall %}<!--status-->{% call c::status("ok") %}Running{% endcall %}{% call c::status("paused") %}Paused{% endcall %}<!--side_panel-->{% call c::side_panel("add-panel", "Add provider", "providers") %}<p>Body</p>{% endcall %}<!--dialog_box-->{% call c::dialog_box("revoke-box", "Revoke?") %}<p>Sure?</p>{% endcall %}<!--row_confirm-->{% call c::row_confirm("restart-confirm", "Restart the server?", "Restart") %}{% endcall %}<!--secret-->{% call c::secret("pair-key") %}{% endcall %}<!--table_head-->{% call c::table_head(["Name", ">Cost", ""]) %}{% endcall %}<!--empty-->{% call c::empty("Nothing yet.", "Add one") %}{% endcall %}<!--skeleton-->{% call c::skeleton(3) %}{% endcall %}<!--na-->{% call c::na("the server cannot do it") %}<span class="btn btn-secondary">Export</span>{% endcall %}<!--na_tag-->{% call c::na_tag("later") %}{% endcall %}"#,
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
        assert!(tabs.starts_with(r#"<nav class="tabs" aria-label="Audit">"#), "a named nav of links: {tabs}");
        assert!(tabs.contains(r#"<a class="tab" href="/admin/audit/calls/" aria-current="page">Model calls</a>"#));
        assert!(tabs.contains(r#"<a class="tab" href="/admin/audit/admin/">Admin actions</a>"#));
        assert_eq!(tabs.matches(r#"aria-current="page""#).count(), 1, "one current tab");

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

    /// Plan step 6.2: every Audit page shows the same four tabs as the menu's Audit links, in the same order, and
    /// marks only its own tab as the current one. The tab markup is built from `PAGES`, so this checks the frame.
    #[test]
    fn audit_pages_show_the_menu_links_as_tabs() {
        let audit: Vec<&Page> = PAGES.iter().filter(|p| p.group == Group::Audit).collect();
        assert_eq!(audit.len(), 4);
        for p in &audit {
            let html = (p.render)().expect("page renders");
            let mut expected = String::from(r#"<nav class="tabs" aria-label="Audit">"#);
            for q in &audit {
                let current = if q.slug == p.slug { r#" aria-current="page""# } else { "" };
                expected.push_str(&format!(r#"<a class="tab" href="/admin/{}/"{current}>{}</a>"#, q.slug, q.label));
            }
            expected.push_str("</nav>");
            assert!(html.contains(&expected), "{}: the tab bar is the four menu links, in order, with its own current", p.slug);
            // Each tab is the same page as its menu link: the side menu links the same path and label.
            for q in &audit {
                let link = format!(r#"href="/admin/{}/" data-tab="{}" data-tip="{}""#, q.slug, q.slug, q.label);
                assert!(html.contains(&link), "{}: the menu links {}", p.slug, q.slug);
            }
            assert!(!html.contains(r#"role="tablist""#), "{}: links, not a tablist", p.slug);
        }
    }
}
