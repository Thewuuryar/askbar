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

/// How the renderer writes a hole for a live example, `<div data-live="name"></div>` (a test checks it matches
/// `build_render::LIVE_HOLE`, which only the build and the tests can use).
const LIVE_HOLE: &str = "data-live=";

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

/// One live example of the admin-ui pages (plan step 7.5): the real component macro, rendered by `ui/templates/live.html`.
#[derive(Template)]
#[template(path = "live.html")]
struct LiveExample {
    name: String,
}

/// The HTML of the example `name`, or an error when `live.html` has no branch for it. The macros write a few parts
/// hidden or closed (a menu, a side panel, a save bar, a row confirmation, a dialog) because on a real page they open
/// when used. Here they are shown in place, so their `hidden` is taken out and the dialog is opened, and the secret's
/// value, which a script fills on a real page, gets a plain placeholder.
fn live_example(name: &str) -> Result<String, String> {
    let html = LiveExample { name: name.to_string() }.render().map_err(|e| format!("live example {name}: {e}"))?;
    let html = html.trim();
    if html.is_empty() {
        return Err(format!("live example {name}: live.html has no example by that name"));
    }
    let shown = match name {
        "menu" | "side_panel" | "row_confirm" | "save_bar" => html.replacen(" hidden>", ">", 1),
        "dialog_box" => html.replacen("<dialog ", "<dialog open ", 1),
        "secret" => html.replacen("></code>", ">Example value</code>", 1),
        _ => html.to_string(),
    };
    Ok(shown)
}

/// A piece of a page: its HTML, then the live example (`name` of `live.html`) that follows it, `""` for none.
struct Part {
    html: &'static str,
    live: &'static str,
    example: String,
}

/// The page's HTML cut at its holes (`<div data-live="name"></div>`, written by the renderer at each marker), each hole
/// paired with the real component. Done on the server, so the browser never builds markup from text.
fn parts(page: &'static RefPage) -> Result<Vec<Part>, String> {
    let hole = format!("<div {}\"", LIVE_HOLE);
    let mut out = Vec::new();
    let mut rest = page.html.as_str();
    while let Some(at) = rest.find(&hole) {
        let after = &rest[at + hole.len()..];
        let end = after.find("\"></div>\n").ok_or_else(|| format!("{}: a live example hole is not closed", page.id))?;
        let name = &after[..end];
        out.push(Part { html: &rest[..at], live: name, example: live_example(name)? });
        rest = &after[end + "\"></div>\n".len()..];
    }
    out.push(Part { html: rest, live: "", example: String::new() });
    Ok(out)
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
    parts: Vec<Part>,
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
        parts: parts(page).map_err(|e| askama::Error::Custom(e.into()))?,
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

    // ---------------------------------------------------------------- live examples (plan step 7.5)

    fn markdown(id: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference").join(format!("{id}.md"));
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// The names of the `<!-- live: name -->` lines of a Markdown page.
    fn markers(md: &str) -> Vec<String> {
        md.lines().filter_map(build_render::live_marker).collect()
    }

    fn served(id: &str) -> &'static str {
        &rendered()[pages().iter().position(|p| p.id == id).unwrap()]
    }

    /// The macros the page frame writes once. They are named on the Components page without an example of their own.
    const FRAME_MACROS: &[&str] = &["skip_link", "announcer", "toast_region", "banner_region"];

    #[test]
    fn the_hole_the_renderer_writes_is_the_one_the_server_looks_for() {
        assert_eq!(LIVE_HOLE, build_render::LIVE_HOLE);
        let (html, _) = build_render::render("admin-ui/x.md", "# T\n\ntext\n\n<!-- live: button -->\n\nmore\n", &HashMap::new()).unwrap();
        assert!(html.contains("<div data-live=\"button\"></div>\n"), "{html}");
        // Any other comment or HTML is still shown as text; a marker with a bad name is not a marker.
        for md in ["<!-- note -->", "<!-- live: Bad Name -->", "<!-- live: -->", "<div data-live=\"x\"></div>"] {
            let (html, _) = build_render::render("admin-ui/x.md", &format!("# T\n\n{md}\n"), &HashMap::new()).unwrap();
            assert!(!html.contains("<div data-live"), "{md}: {html}");
        }
        // Only the admin-ui pages may show live examples.
        assert!(build_render::render("settings/x.md", "# T\n\n<!-- live: button -->\n", &HashMap::new()).is_err());
    }

    #[test]
    fn every_marker_has_a_renderer_and_none_is_left_in_the_served_html() {
        let mut total = 0;
        for id in ["admin-ui/components", "admin-ui/page-layouts"] {
            let names = markers(&markdown(id));
            assert!(!names.is_empty(), "{id}: no live examples");
            let html = served(id);
            assert!(!html.contains("data-live=\""), "{id}: a hole is left in the served HTML");
            assert!(!html.contains("<!-- live") && !html.contains("{{live"), "{id}: a marker is left in the served HTML");
            let shown: Vec<&str> = html.split("data-live-example=\"").skip(1).map(|r| r.split('"').next().unwrap()).collect();
            assert_eq!(shown, names, "{id}: the examples shown are the markers, in order");
            for name in &names {
                live_example(name).unwrap_or_else(|e| panic!("{id}: {e}"));
            }
            let mut sorted = names.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), names.len(), "{id}: a marker is used twice");
            total += names.len();
        }
        assert!(total >= 25, "{total}");
        assert!(live_example("no_such_example").is_err(), "a name with no branch in live.html is an error");
    }

    #[test]
    fn the_components_page_names_every_macro_and_shows_all_but_the_frame_ones() {
        let templates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui/templates/components.html");
        let source = std::fs::read_to_string(templates).unwrap();
        let macros: Vec<&str> = source.split("{% macro ").skip(1).map(|r| r.split('(').next().unwrap()).collect();
        assert!(macros.len() >= 25, "{macros:?}");
        let md = markdown("admin-ui/components");
        let shown = markers(&md);
        for m in &macros {
            assert!(md.contains(&format!("#### `{m}`")), "components.md does not name the macro {m}");
            assert_eq!(shown.iter().any(|s| s == m), !FRAME_MACROS.contains(m), "{m}: a live example iff it is not a frame macro");
        }
        for s in &shown {
            assert!(macros.contains(&s.as_str()), "{s} is a marker but not a macro");
        }
    }

    #[test]
    fn the_admin_ui_markdown_holds_no_html_and_only_these_pages_show_examples() {
        for id in ["admin-ui/components", "admin-ui/page-layouts"] {
            let md = markdown(id);
            let body: String = md.lines().filter(|l| build_render::live_marker(l).is_none()).collect::<Vec<_>>().join("\n");
            // Code is allowed to hold anything; what is left must not open a tag.
            let mut plain = String::new();
            let mut in_fence = false;
            for line in body.lines() {
                if line.trim_start().starts_with("```") {
                    in_fence = !in_fence;
                } else if !in_fence {
                    plain.push_str(&line.split('`').step_by(2).collect::<String>());
                    plain.push('\n');
                }
            }
            for (i, _) in plain.match_indices('<') {
                let next = plain[i + 1..].chars().next().unwrap_or(' ');
                assert!(
                    !(next.is_ascii_alphabetic() || next == '/' || next == '!'),
                    "{id}: HTML in the Markdown near {:?}",
                    &plain[i..plain.len().min(i + 30)]
                );
            }
        }
        for p in pages() {
            if !p.id.starts_with("admin-ui/") {
                assert!(!served(&p.id).contains("data-live-example"), "{}: shows a live example", p.id);
                assert!(!p.html.contains("data-live="), "{}: has a hole", p.id);
            }
        }
    }

    #[test]
    fn the_live_examples_have_no_duplicate_ids_and_use_the_ex_prefix_for_their_own() {
        for id in ["admin-ui/components", "admin-ui/page-layouts"] {
            let html = served(id);
            let mut seen = std::collections::HashSet::new();
            for part in html.split(" id=\"").skip(1) {
                let value = part.split('"').next().unwrap();
                assert!(seen.insert(value), "{id}: the id {value} is used twice");
            }
            let live: String =
                html.split("data-live-example=\"").skip(1).map(|r| r.split("</div></article>").next().unwrap_or(r)).collect();
            assert!(
                !live.contains("<script") && !live.contains(" style=") && !live.contains(" disabled"),
                "{id}: unsafe or disabled markup in an example"
            );
        }
        let c = served("admin-ui/components");
        for id in ["ex-menu", "ex-panel", "ex-box", "ex-row-confirm", "ex-form-save-bar", "ex-field", "ex-password", "ex-secret-value"] {
            assert!(c.contains(&format!(" id=\"{id}\"")), "the example {id} is missing");
        }
        // The parts the macros write hidden or closed are shown.
        assert!(c.contains("<dialog open "), "the dialog example is open");
        for id in ["ex-menu", "ex-panel", "ex-row-confirm", "ex-form-save-bar"] {
            let tag = c.split(&format!(" id=\"{id}\"")).nth(1).unwrap().split('>').next().unwrap();
            assert!(!tag.contains("hidden"), "{id} is shown: {tag}");
        }
    }
}
