//! The Markdown renderer behind the reference (plan step 7.2), shared by `build.rs` (which renders the pages when the admin
//! is built) and the tests in `src/reference.rs` (which render hostile documents through the very same code).
//! What it allows is described in `build.rs`.
#![allow(dead_code)]

use pulldown_cmark::{html, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};
use std::collections::{BTreeMap, HashMap};

/// The marker the runtime replaces with `{SOURCE_URL}/blob/{SOURCE_BRANCH}` at the start of an `href`.
pub const SOURCE_MARK: &str = "@SOURCE@";

/// The pages under this folder of docs/reference may show live examples.
pub const LIVE_DIR: &str = "admin-ui/";
/// How a hole for a live example starts in the rendered HTML: `<div data-live="button"></div>`.
pub const LIVE_HOLE: &str = "data-live=";

/// The example name of a line `<!-- live: name -->` (lower case letters, digits and `_`), or None for any other HTML.
pub fn live_marker(html: &str) -> Option<String> {
    let name = html.trim().strip_prefix("<!--")?.strip_suffix("-->")?.trim().strip_prefix("live:")?.trim();
    let ok = !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    ok.then(|| name.to_string())
}

pub struct Heading {
    pub level: u8,
    pub id: String,
    pub text: String,
}

/// The reference file a relative link from `from` points at (`settings/network.md`), when it stays under the root.
pub fn resolve_page(from: &str, dest: &str) -> Option<String> {
    if has_scheme(dest) || dest.starts_with('#') || dest.starts_with('/') {
        return None;
    }
    let path = dest.split(['#', '?']).next()?;
    let joined = join(from, path)?;
    joined.ends_with(".md").then_some(joined)
}

/// `rel` against the directory of `from`, both under the root; None if it climbs out of it.
pub fn join(from: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

pub fn has_scheme(dest: &str) -> bool {
    // Browsers drop tabs, newlines and leading control characters inside a scheme, so look at what is left.
    let clean: String = dest.chars().filter(|c| !c.is_control() && !c.is_whitespace()).collect();
    if clean.starts_with("//") {
        return true;
    }
    let head = clean.split(['/', '?', '#']).next().unwrap_or("");
    head.contains(':')
}

// ------------------------------------------------------------------ rendering

/// What a link written to the output needs at its end.
enum LinkKind {
    Same,
    NewTab,
    Dropped,
}

/// Every link that leaves the admin (a repository file, GitHub, any other site) opens in a new tab, and says so to
/// assistive technology with visually hidden text after the link's own words.
const NEW_TAB: &str = "target=\"_blank\" rel=\"noopener noreferrer\"";
const NEW_TAB_HINT: &str = "<span class=\"visually-hidden\"> (opens in a new tab)</span>";

enum Link {
    Admin(String),
    Source(String),
    External(String),
    None,
}

fn classify(file: &str, dest: &str, ids: &HashMap<String, usize>) -> Result<Link, String> {
    if dest.starts_with('#') {
        return Ok(Link::Admin(dest.to_string()));
    }
    if has_scheme(dest) {
        let lower = dest.trim().to_ascii_lowercase();
        return Ok(if lower.starts_with("https://") || lower.starts_with("http://") {
            Link::External(dest.trim().to_string())
        } else {
            Link::None
        });
    }
    if dest.starts_with('/') {
        return Ok(Link::None);
    }
    let (path, frag) = match dest.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (dest, None),
    };
    let frag = frag.filter(|f| !f.is_empty()).map(|f| format!("#{f}")).unwrap_or_default();
    // Inside the reference root: another page.
    if let Some(target) = join(file, path) {
        if path.is_empty() {
            return Ok(Link::Admin(frag));
        }
        if target.ends_with(".md") {
            return match ids.get(&target) {
                Some(_) => {
                    let id = &target[..target.len() - 3];
                    Ok(Link::Admin(if id == "index" {
                        format!("/admin/reference/{frag}")
                    } else {
                        format!("/admin/reference/{id}/{frag}")
                    }))
                }
                None => Err(format!("{}: the link '{dest}' names no page under docs/reference", file)),
            };
        }
        return Err(format!("{}: the link '{dest}' points at a non-page file under docs/reference", file));
    }
    // Outside: a file elsewhere in the repository, relative to docs/reference/<dir>.
    let from_repo = format!("docs/reference/{}", file);
    match join(&from_repo, path) {
        Some(p) if !p.is_empty() => Ok(Link::Source(format!("{p}{frag}"))),
        _ => Ok(Link::None),
    }
}

pub fn attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

pub fn slug(text: &str) -> String {
    let t: String = text.chars().filter(|c| !matches!(c, '`' | '*' | '_')).collect();
    let t = t.trim().to_lowercase();
    t.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == ' ' || *c == '_').map(|c| if c == ' ' { '-' } else { c }).collect()
}

pub fn render(file: &str, body: &str, ids: &HashMap<String, usize>) -> Result<(String, Vec<Heading>), String> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let events: Vec<Event> = Parser::new_ext(body, opts).collect();
    let mut out: Vec<Event> = Vec::new();
    let mut headings = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut first_h1_done = false;
    let mut skipping_h1 = false;
    let mut links: Vec<LinkKind> = Vec::new();
    let mut i = 0;
    while i < events.len() {
        let ev = events[i].clone();
        i += 1;
        if skipping_h1 && !matches!(ev, Event::End(TagEnd::Heading(_))) {
            continue;
        }
        match ev {
            Event::Start(Tag::Heading { level, .. }) => {
                // Gather the heading's plain text up to its end.
                let mut text = String::new();
                let mut j = i;
                while j < events.len() {
                    match &events[j] {
                        Event::End(TagEnd::Heading(_)) => break,
                        Event::Text(t) | Event::Code(t) => text.push_str(t),
                        _ => {}
                    }
                    j += 1;
                }
                let base = slug(&text);
                let n = seen.entry(base.clone()).or_insert(0);
                let id = if *n == 0 { base.clone() } else { format!("{base}-{n}") };
                *n += 1;
                let lvl = level as u8;
                // The page's title is the admin page's heading, so the first `#` is left out of the article.
                if lvl == 1 && !first_h1_done {
                    first_h1_done = true;
                    skipping_h1 = true;
                    continue;
                }
                headings.push(Heading { level: lvl, id: id.clone(), text });
                // The article's headings sit one level down: the page title is the h2.
                let shown = (lvl + 1).min(6);
                out.push(Event::Html(CowStr::from(format!("<h{shown} id=\"{}\">", attr(&id)))));
            }
            Event::End(TagEnd::Heading(level)) => {
                if skipping_h1 {
                    skipping_h1 = false;
                    continue;
                }
                let shown = (level as u8 + 1).min(6);
                out.push(Event::Html(CowStr::from(format!("</h{shown}>\n"))));
            }
            Event::Start(Tag::Link { dest_url, link_type, .. }) => {
                // A kept link is written as an `<a>` here; an unusable one is dropped and its text stays.
                let open = if matches!(link_type, LinkType::Email) {
                    None
                } else {
                    match classify(file, &dest_url, ids)? {
                        Link::Admin(h) => Some((format!("<a href=\"{}\">", attr(&h)), LinkKind::Same)),
                        Link::Source(p) => Some((format!("<a href=\"{SOURCE_MARK}/{}\" {NEW_TAB}>", attr(&p)), LinkKind::NewTab)),
                        Link::External(u) => Some((format!("<a href=\"{}\" {NEW_TAB}>", attr(&u)), LinkKind::NewTab)),
                        Link::None => None,
                    }
                };
                match open {
                    Some((tag, kind)) => {
                        links.push(kind);
                        out.push(Event::Html(CowStr::from(tag)));
                    }
                    None => links.push(LinkKind::Dropped),
                }
            }
            Event::End(TagEnd::Link) => match links.pop().unwrap_or(LinkKind::Dropped) {
                LinkKind::Same => out.push(Event::Html(CowStr::from("</a>"))),
                LinkKind::NewTab => out.push(Event::Html(CowStr::from(format!("{NEW_TAB_HINT}</a>")))),
                LinkKind::Dropped => {}
            },
            // Images are dropped; their alt text stays as plain text.
            Event::Start(Tag::Image { .. }) | Event::End(TagEnd::Image) => {}
            // A line `<!-- live: name -->` marks where the admin shows a live example (plan step 7.5): GitHub hides it, and
            // here it becomes an empty hole the server fills with the real component. Only the admin-ui pages may use it.
            Event::Html(t) if live_marker(&t).is_some() => {
                let name = live_marker(&t).unwrap_or_default();
                if !file.starts_with(LIVE_DIR) {
                    return Err(format!("{file}: a live example marker ('{name}') is only allowed under {LIVE_DIR}"));
                }
                out.push(Event::Html(CowStr::from(format!("<div {LIVE_HOLE}\"{name}\"></div>\n"))));
            }
            // Other raw HTML is shown as text, never passed on.
            Event::Html(t) | Event::InlineHtml(t) => out.push(Event::Text(t)),
            other => out.push(other),
        }
    }
    let mut html = String::new();
    html::push_html(&mut html, out.into_iter());
    Ok((html, headings))
}

/// The HTML of each section of a rendered page: `("", intro)` for what comes before the first heading, then
/// `(heading id, html)` for each heading, from after its closing tag up to the next heading of the same or a higher
/// level (deeper headings stay inside). The heading itself is left out: the help panel has its own title.
/// Headings are found by the `<hN id="...">` the renderer writes; raw HTML never reaches the output as markup, so
/// nothing else can look like one.
pub fn sections(html: &str) -> Vec<(String, String)> {
    // (start of the open tag, end of the close tag, level, id)
    let mut heads: Vec<(usize, usize, u8, String)> = Vec::new();
    let mut at = 0;
    while let Some(p) = html[at..].find("<h") {
        let start = at + p;
        at = start + 2;
        let rest = &html[start + 2..];
        let Some(level) = rest.bytes().next().filter(|b| (b'1'..=b'6').contains(b)).map(|b| b - b'0') else { continue };
        let Some(id_rest) = rest[1..].strip_prefix(" id=\"") else { continue };
        let Some(id_end) = id_rest.find('"') else { continue };
        let close = format!("</h{level}>\n");
        let Some(c) = html[start..].find(&close) else { continue };
        heads.push((start, start + c + close.len(), level, id_rest[..id_end].to_string()));
    }
    let mut out = vec![(String::new(), html[..heads.first().map_or(html.len(), |h| h.0)].to_string())];
    for (i, (_, body_start, level, id)) in heads.iter().enumerate() {
        let end = heads[i + 1..].iter().find(|h| h.2 <= *level).map_or(html.len(), |h| h.0);
        out.push((id.clone(), html[*body_start..end].to_string()));
    }
    out
}
