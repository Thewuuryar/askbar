//! The reference in the admin (plan step 7.2): the Markdown pages under docs/reference, rendered to HTML when the admin is
//! built (`build.rs`) and compiled in, so nothing is read at run time and the admin image holds no docs. `/admin/reference/`
//! is the index page and `/admin/reference/<id>/` a page, in the article layout (`ui/templates/reference.html`).
//!
//! The HTML has no inline script or style and no raw HTML from the Markdown (the renderer shows it as text), so the admin's
//! CSP is the same as for every other page. Links to the repository (outside docs/reference) carry a marker that is filled
//! here from the constants, so the one place that knows the repository's address stays `constants.rs`.

use crate::constants::{HELP_TOPICS, REFERENCE_ROOT, SOURCE_BRANCH, SOURCE_URL};
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

/// One help topic (plan step 7.3): what the ? button shows in the side panel. Its text is the rendered section of the
/// reference page, ready when the admin starts, so opening help fetches nothing.
pub struct HelpTopic {
    /// The topic as `constants::HELP_TOPICS` lists it: `settings/network#host-name`, or a page id alone.
    pub topic: &'static str,
    /// The heading's text, or the page's title for a page topic. The ? button's name is "Help: <title>".
    pub title: String,
    /// The page's title, shown above the heading in the panel.
    pub kicker: String,
    /// "Open full page": the article, at the heading.
    pub href: String,
    /// The section's HTML, without its heading.
    pub html: String,
}

/// Every help topic, in `HELP_TOPICS` order. Panics (so the admin does not start) on a topic that names no page or
/// heading, or whose section is empty: the topic list, the reference and the buttons cannot drift apart unnoticed.
pub fn help_topics() -> &'static [HelpTopic] {
    static TOPICS: OnceLock<Vec<HelpTopic>> = OnceLock::new();
    TOPICS.get_or_init(|| {
        let marker = "href=\"@SOURCE@/";
        let source = format!("href=\"{SOURCE_URL}/blob/{SOURCE_BRANCH}/");
        HELP_TOPICS
            .iter()
            .map(|&topic| {
                let (id, anchor) = topic.split_once('#').unwrap_or((topic, ""));
                let page = find(id).unwrap_or_else(|| panic!("help topic {topic}: no reference page {id}"));
                let html = SECTION_HTML
                    .iter()
                    .find(|(k, _)| *k == topic)
                    .map(|(_, h)| h.replace(marker, &source))
                    .unwrap_or_else(|| panic!("help topic {topic}: the page has no such section"));
                assert!(!html.trim().is_empty(), "help topic {topic}: its section is empty");
                let title = if anchor.is_empty() {
                    page.title.clone()
                } else {
                    page.headings
                        .iter()
                        .find(|h| h.id == anchor)
                        .unwrap_or_else(|| panic!("help topic {topic}: no heading {anchor}"))
                        .text
                        .clone()
                };
                let href = if anchor.is_empty() { page.path() } else { format!("{}#{anchor}", page.path()) };
                HelpTopic { topic, title, kicker: page.title.clone(), href, html }
            })
            .collect()
    })
}

/// The topic's record.
pub fn help_topic(topic: &str) -> Option<&'static HelpTopic> {
    help_topics().iter().find(|t| t.topic == topic)
}

/// The topics a page carries in its HTML: those of `settings/<slug>` (the people page's reference page is
/// `settings/people`, and so on). A page with none has no ? buttons and no help panel.
pub fn topics_for(slug: &str) -> Vec<&'static HelpTopic> {
    let id = format!("settings/{slug}");
    help_topics().iter().filter(|t| t.topic == id || t.topic.strip_prefix(&id).is_some_and(|r| r.starts_with('#'))).collect()
}

/// The topic of the ? button after a page's title: the page's own, or "" when it has none.
pub fn page_topic(slug: &str) -> &'static str {
    let id = format!("settings/{slug}");
    help_topics().iter().find(|t| t.topic == id).map_or("", |t| t.topic)
}

/// The ? button's accessible name and tooltip.
pub fn help_label(topic: &str) -> String {
    format!("Help: {}", help_topic(topic).map_or("this page", |t| t.title.as_str()))
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
    lede: &'static str,
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
        lede: page.summary.as_str(),
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
    fn every_help_topic_names_a_page_or_heading_and_resolves_to_a_rendered_section() {
        for topic in crate::constants::HELP_TOPICS {
            let (page, anchor) = topic.split_once('#').unwrap_or((topic, ""));
            let p = find(page).unwrap_or_else(|| panic!("{topic}: no page {page}"));
            assert!(anchor.is_empty() || p.headings.iter().any(|h| h.id == anchor), "{topic}: no heading {anchor}");
            // The section is in the compiled-in HTML, not empty, and still safe (it is shown with no escaping).
            let t = help_topic(topic).unwrap_or_else(|| panic!("{topic}: not in the help index"));
            assert!(!t.html.trim().is_empty(), "{topic}: empty section");
            assert!(!t.html.contains("@SOURCE@"), "{topic}: a source link was not filled in");
            assert!(!t.html.contains("Placeholder"), "{topic}: the text is not written yet");
            no_active_content(&t.html, topic);
            assert!(t.href.starts_with(&p.path()) && t.href.ends_with(&format!("#{anchor}")) == !anchor.is_empty(), "{}", t.href);
        }
        assert_eq!(help_topics().len(), crate::constants::HELP_TOPICS.len());
    }

    #[test]
    fn a_section_stops_at_the_next_heading_of_the_same_or_a_higher_level() {
        let md = "# T\n\nintro\n\n## A\n\na text\n\n### A1\n\ndeep\n\n## B\n\nb text\n";
        let (html, _) = build_render::render("x/y.md", md, &HashMap::new()).unwrap();
        let s: HashMap<String, String> = build_render::sections(&html).into_iter().collect();
        assert!(s[""].contains("intro") && !s[""].contains("a text"));
        assert!(s["a"].contains("a text") && s["a"].contains("deep") && !s["a"].contains("b text") && !s["a"].contains("<h2"));
        assert!(s["a1"].contains("deep") && !s["a1"].contains("b text"));
        assert!(s["b"].contains("b text") && !s["b"].contains("a text"));
    }

    /// Every `help(...)` call in the templates, and every ? button on a rendered page, names a topic in the list, and
    /// the page carries that topic's text, so a missing topic fails here and not on a user's click.
    #[test]
    fn every_help_button_names_a_topic_and_its_page_carries_the_text() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/templates");
        let mut calls = 0;
        for e in std::fs::read_dir(dir).unwrap() {
            let path = e.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            if path.file_name().unwrap() == "components.html" {
                continue; // the macros' own documentation uses example topics
            }
            // A topic is written as a string literal that starts with `settings/` (a help call's or a macro argument's).
            for part in text.split("\"settings/").skip(1) {
                let topic = format!("settings/{}", part.split('"').next().unwrap());
                assert!(HELP_TOPICS.contains(&topic.as_str()), "{}: help topic '{topic}' is not in HELP_TOPICS", path.display());
                calls += 1;
            }
        }
        assert!(calls >= 30, "{calls}");
        let mut buttons = 0;
        for html in crate::pages::rendered() {
            for part in html.split("data-help=\"").skip(1) {
                let topic = part.split('"').next().unwrap();
                assert!(HELP_TOPICS.contains(&topic), "a rendered page has a help button for '{topic}'");
                let want = format!("data-help-topic=\"{topic}\"");
                assert!(html.contains(&want), "the page with a help button for '{topic}' does not carry its text");
                buttons += 1;
            }
        }
        assert!(buttons >= 40, "{buttons}");
    }
}
