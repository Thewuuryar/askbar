//! The reference in the admin (plan step 7.2): the Markdown pages under docs/reference, rendered to HTML when the admin is
//! built (`build.rs`) and compiled in, so nothing is read at run time and the admin image holds no docs. `/admin/reference/`
//! is the index page and `/admin/reference/<id>/` a page, in the article layout (`ui/templates/reference.html`).
//!
//! The HTML has no inline script or style and no raw HTML from the Markdown (the renderer shows it as text), so the admin's
//! CSP is the same as for every other page. Links to the repository (outside docs/reference) carry a marker that is filled
//! here from the constants, so the one place that knows the repository's address stays `constants.rs`.

use crate::constants::{REFERENCE_ROOT, SOURCE_BRANCH, SOURCE_URL};
use crate::pages::{menu, section_id, MenuGroup, Page};
use askama::Template;
use serde::Deserialize;
use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/reference_pages.rs"));
static INDEX_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/reference.json"));

// The renderer `build.rs` uses, so the tests feed it hostile Markdown directly.
#[cfg(test)]
#[path = "../build_render.rs"]
mod build_render;

/// The slug of the menu item the reference pages belong to.
const MENU_SLUG: &str = "reference";

/// One heading of a page: its level in the Markdown (`##` is 2) and the id it has in the page.
#[derive(Deserialize, Debug)]
pub struct Heading {
    pub level: u8,
    pub id: String,
    pub text: String,
}

/// One reference page: its front matter, the headings it has, and its HTML.
#[derive(Deserialize, Debug)]
pub struct RefPage {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub keywords: Vec<String>,
    pub settings: Vec<String>,
    /// The page's file in the repository, `docs/reference/settings/network.md`.
    pub source: String,
    /// The area the index lists it under (empty for the index itself).
    pub group: String,
    pub order: usize,
    pub headings: Vec<Heading>,
    #[serde(skip)]
    pub html: String,
}

impl RefPage {
    pub fn is_index(&self) -> bool {
        self.id == "index"
    }
    /// `/admin/reference/` for the index, `/admin/reference/settings/network/` for a page.
    pub fn path(&self) -> String {
        if self.is_index() {
            "/admin/reference/".to_string()
        } else {
            format!("/admin/reference/{}/", self.id)
        }
    }
    /// The page's file on GitHub, in the editor.
    pub fn edit_url(&self) -> String {
        format!("{SOURCE_URL}/edit/{SOURCE_BRANCH}/{}", self.source)
    }
}

/// Every reference page, in the order the index lists them (the index first).
pub fn pages() -> &'static [RefPage] {
    static PAGES: OnceLock<Vec<RefPage>> = OnceLock::new();
    PAGES.get_or_init(|| {
        let mut v: Vec<RefPage> = serde_json::from_str(INDEX_JSON).expect("the reference index built by build.rs is JSON");
        let marker = "href=\"@SOURCE@/";
        let source = format!("href=\"{SOURCE_URL}/blob/{SOURCE_BRANCH}/");
        for p in &mut v {
            let html = PAGE_HTML.iter().find(|(id, _)| *id == p.id).map(|(_, h)| *h).expect("every indexed page has HTML");
            p.html = html.replace(marker, &source);
            debug_assert!(p.source.starts_with(REFERENCE_ROOT));
        }
        v.sort_by_key(|p| p.order);
        v
    })
}

/// The page with this id (`settings/network`).
pub fn find(id: &str) -> Option<&'static RefPage> {
    pages().iter().find(|p| p.id == id)
}

/// The index of the reference as JSON: the front matter and headings of every page, for the search and for tests.
pub fn index_json() -> &'static str {
    INDEX_JSON
}

struct OutlineGroup {
    label: &'static str,
    pages: Vec<&'static RefPage>,
}

/// The pages by area, in index order: each area is an `##` heading of the index page.
fn outline() -> Vec<OutlineGroup> {
    let mut out: Vec<OutlineGroup> = Vec::new();
    for p in pages().iter().filter(|p| !p.is_index()) {
        match out.iter_mut().find(|g| g.label == p.group) {
            Some(g) => g.pages.push(p),
            None => out.push(OutlineGroup { label: p.group.as_str(), pages: vec![p] }),
        }
    }
    out
}

#[derive(Template)]
#[template(path = "reference.html")]
struct ReferenceView {
    current: &'static str,
    label: &'static str,
    group: &'static str,
    section: String,
    menu: &'static [MenuGroup],
    tabs: Vec<&'static Page>,
    summary: &'static str,
    html: &'static str,
    page: &'static RefPage,
    outline: Vec<OutlineGroup>,
    edit_url: String,
}

/// The page's HTML in the admin's frame.
pub fn render(page: &'static RefPage) -> askama::Result<String> {
    ReferenceView {
        current: MENU_SLUG,
        label: if page.is_index() { "Reference" } else { page.title.as_str() },
        group: if page.is_index() { "" } else { "Reference" },
        section: section_id(MENU_SLUG),
        menu: menu(),
        tabs: Vec::new(),
        summary: page.summary.as_str(),
        html: page.html.as_str(),
        page,
        outline: outline(),
        edit_url: page.edit_url(),
    }
    .render()
}

/// The index page, which is the `reference` entry of `PAGES`.
pub fn index_html() -> askama::Result<String> {
    render(find("index").expect("docs/reference/index.md exists"))
}

/// Each page's frame, rendered once (as `pages::rendered` does), so a template error stops the admin at start.
pub fn rendered() -> &'static [String] {
    static HTML: OnceLock<Vec<String>> = OnceLock::new();
    HTML.get_or_init(|| pages().iter().map(|p| render(p).unwrap_or_else(|e| panic!("reference page {}: {e}", p.id))).collect())
}

/// The response for page number `i` of `pages()`.
pub fn response(i: usize) -> axum::response::Response {
    use axum::response::IntoResponse;
    ([(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")], rendered()[i].as_str()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use super::build_render;

    /// Every `href="..."` value of an HTML string.
    fn hrefs(html: &str) -> Vec<&str> {
        html.split("href=\"").skip(1).map(|r| &r[..r.find('"').unwrap()]).collect()
    }

    fn allowed_href(h: &str) -> bool {
        h.starts_with("/admin/reference/")
            || h.starts_with('#')
            || h.starts_with("https://")
            || h.starts_with("http://")
            || h.starts_with("@SOURCE@/")
    }

    fn no_active_content(html: &str, what: &str) {
        let lower = html.to_ascii_lowercase();
        for bad in ["<script", "<img", "<iframe", "<style", "<object", "<form", " style=", "javascript:"] {
            assert!(!lower.contains(bad), "{what}: found {bad} in {html}");
        }
        // No tag carries an event handler: an `on...=` can only appear inside escaped text.
        for tag in html.split('<').skip(1) {
            let head = &tag[..tag.find('>').unwrap_or(tag.len())];
            assert!(
                !head.to_ascii_lowercase().split_whitespace().any(|w| w.starts_with("on") && w.contains('=')),
                "{what}: handler in <{head}>"
            );
        }
        assert!(hrefs(html).iter().all(|h| allowed_href(h)), "{what}: {:?}", hrefs(html));
    }

    #[test]
    fn raw_html_and_unsafe_links_do_not_pass_through() {
        let ids: HashMap<String, usize> = [("x/other.md".to_string(), 0)].into();
        let md = "# T\n\n<script>alert(1)</script>\n\ntext <img src=x onerror=alert(1)> more\n\n\
                  <div onclick=\"x()\">block</div>\n\n\
                  [a](javascript:alert(1)) [b](JaVaScRiPt:alert(1)) [c](data:text/html,x) [d](java\tscript:alert(1)) [e](//evil.example/x) \
                  [f](mailto:a@b.c) [g](/admin/api) <https://auto.example/x>\n\n\
                  [ok](https://example.com/a?b=1&c=2) [rel](other.md#h) [up](../../adr/0001-x.md#why) [anchor](#h) ![alt text](http://x/y.png)\n\n\
                  ## Same\n\n## Same\n\n### Third `code_x`\n\n```\n<b>not bold</b> a_long_line\n```\n\n| a | b |\n| - | - |\n| 1 | <i>2</i> |\n";
        let (html, heads) = build_render::render("x/y.md", md, &ids).unwrap();
        no_active_content(&html, "hostile document");
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"), "{html}");
        // The unsafe links keep their text but are not links.
        for text in [">a<", ">b<", ">c<", ">e<", ">f<", ">g<"] {
            assert!(!html.contains(&format!("{text}/a>")), "{text} is still a link: {html}");
        }
        assert!(html.contains("href=\"/admin/reference/x/other/#h\""), "{html}");
        assert!(html.contains("href=\"@SOURCE@/docs/adr/0001-x.md#why\" rel=\"noopener noreferrer\""), "{html}");
        assert!(html.contains("href=\"https://example.com/a?b=1&amp;c=2\" rel=\"noopener noreferrer\""), "{html}");
        assert!(html.contains("<table>") && html.contains("<pre><code>"), "{html}");
        assert!(!html.contains("<h2 id=\"t\">"), "the first # is the page title, left out");
        // Every heading has an id; repeated headings get distinct ones.
        let ids: Vec<&str> = heads.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["same", "same-1", "third-codex"]);
        assert_eq!(html.matches("<h").count(), html.matches(" id=\"").count());
        // A link to a page that does not exist stops the build.
        assert!(build_render::render("x/y.md", "[z](missing.md)", &ids_none()).is_err());
    }

    fn ids_none() -> HashMap<String, usize> {
        HashMap::new()
    }

    #[test]
    fn every_page_renders_safely_with_ids_on_every_heading() {
        assert!(pages().len() >= 10, "{}", pages().len());
        assert!(pages()[0].is_index(), "the index is first");
        for p in pages() {
            no_active_content(&p.html, &p.id);
            for tag in p.html.split("<h").skip(1).filter(|t| t.as_bytes()[0].is_ascii_digit()) {
                assert!(tag.split('>').next().unwrap().contains(" id=\""), "{}: a heading without an id: <h{tag:.40}", p.id);
            }
            for h in &p.headings {
                assert!(p.html.contains(&format!(" id=\"{}\"", h.id)), "{}: heading {} is not in the page", p.id, h.id);
            }
            let framed = &rendered()[pages().iter().position(|q| q.id == p.id).unwrap()];
            assert!(framed.contains(&p.edit_url()), "{}: no Edit on GitHub link", p.id);
            assert!(p.edit_url().starts_with(&format!("{SOURCE_URL}/edit/{SOURCE_BRANCH}/docs/reference/")), "{}", p.edit_url());
        }
        // Links to the other docs go to the source on GitHub.
        let net = &find("settings/network").unwrap().html;
        assert!(net.contains(&format!("href=\"{SOURCE_URL}/blob/{SOURCE_BRANCH}/docs/guide.md#")), "{net}");
        assert!(!net.contains("@SOURCE@"));
    }

    #[test]
    fn every_help_topic_names_a_heading_of_a_page() {
        for topic in crate::constants::HELP_TOPICS {
            let (page, anchor) = topic.split_once('#').expect("page#heading");
            let p = find(page).unwrap_or_else(|| panic!("{topic}: no page {page}"));
            assert!(p.headings.iter().any(|h| h.id == anchor), "{topic}: no heading {anchor}");
        }
    }
}
