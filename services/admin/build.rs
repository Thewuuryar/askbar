//! Renders the reference pages (docs/reference/**/*.md, plan step 7.2) to HTML at build time, so the admin serves the
//! same Markdown GitHub shows and reads no file at run time. Writes into OUT_DIR:
//! - `reference/<id with / as __>.html`: each page's article HTML;
//! - `reference.json`: the index (id, title, summary, keywords, settings, source, group, order, headings with ids);
//! - `reference_pages.rs`: `PAGE_HTML`, the (id, html) pairs `src/reference.rs` includes.
//!
//! What the renderer allows (the admin's CSP is strict and the Markdown is not trusted blindly):
//! - raw HTML in the Markdown is shown as text, never passed on;
//! - a link is kept only when it is relative, an `#anchor`, or http(s); any other scheme (`javascript:`, `data:`,
//!   `mailto:`...) leaves the link text without a link;
//! - a link to another reference page becomes an admin link; a link to another file in the repository becomes a
//!   source link (the `@SOURCE@` marker, which `src/reference.rs` fills from the constants);
//! - images are dropped (their alt text stays);
//! - every heading gets an id, made the way `scripts/check-reference.sh` makes it, so `page#heading` topics resolve.
//!
//! A page whose front matter is wrong, or a relative link to a page that does not exist, fails the build.

use build_render::{render, resolve_page};
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{env, fs};

#[path = "build_render.rs"]
mod build_render;

const ROOT: &str = "../../docs/reference";

struct Source {
    id: String,
    title: String,
    summary: String,
    keywords: Vec<String>,
    settings: Vec<String>,
    /// Path under docs/reference with `.md`, as `settings/network.md`.
    file: String,
    body: String,
}

fn main() {
    println!("cargo:rerun-if-changed={ROOT}");
    println!("cargo:rerun-if-changed=build.rs");
    if let Err(e) = run() {
        eprintln!("error: the reference under docs/reference cannot be built: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut files = Vec::new();
    walk(Path::new(ROOT), &mut files).map_err(|e| format!("reading {ROOT}: {e}"))?;
    files.sort();
    let mut sources = Vec::new();
    for f in &files {
        let rel = f.strip_prefix(ROOT).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
        let text = fs::read_to_string(f).map_err(|e| format!("{rel}: {e}"))?;
        sources.push(parse(&rel, &text)?);
    }
    if sources.is_empty() {
        return Err(format!("{ROOT} holds no pages"));
    }
    let ids: HashMap<String, usize> = sources.iter().enumerate().map(|(i, s)| (s.file.clone(), i)).collect();

    // The index page lists the pages by area: its `##` headings are the areas, its links to pages the members.
    let (group_of, order_of) = outline(&sources, &ids);

    let out = PathBuf::from(env::var("OUT_DIR").map_err(|e| e.to_string())?);
    fs::create_dir_all(out.join("reference")).map_err(|e| e.to_string())?;
    let mut json = String::from("[");
    let mut pages_rs = String::from("pub static PAGE_HTML: &[(&str, &str)] = &[\n");
    for (i, s) in sources.iter().enumerate() {
        let (html, headings) = render(&s.file, &s.body, &ids)?;
        let name = format!("reference/{}.html", s.id.replace('/', "__"));
        fs::write(out.join(&name), &html).map_err(|e| e.to_string())?;
        let _ = writeln!(pages_rs, "    ({:?}, include_str!(concat!(env!(\"OUT_DIR\"), \"/{name}\"))),", s.id);
        if i > 0 {
            json.push(',');
        }
        let heads: Vec<String> =
            headings.iter().map(|h| format!("{{\"level\":{},\"id\":{},\"text\":{}}}", h.level, jstr(&h.id), jstr(&h.text))).collect();
        let list = |v: &[String]| format!("[{}]", v.iter().map(|x| jstr(x)).collect::<Vec<_>>().join(","));
        let _ = write!(
            json,
            "{{\"id\":{},\"title\":{},\"summary\":{},\"keywords\":{},\"settings\":{},\"source\":{},\"group\":{},\"order\":{},\"headings\":[{}]}}",
            jstr(&s.id),
            jstr(&s.title),
            jstr(&s.summary),
            list(&s.keywords),
            list(&s.settings),
            jstr(&format!("docs/reference/{}", s.file)),
            jstr(&group_of[i]),
            order_of[i],
            heads.join(",")
        );
    }
    json.push(']');
    pages_rs.push_str("];\n");
    fs::write(out.join("reference.json"), json).map_err(|e| e.to_string())?;
    fs::write(out.join("reference_pages.rs"), pages_rs).map_err(|e| e.to_string())?;
    Ok(())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for e in fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            walk(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
    Ok(())
}

fn jstr(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

// ------------------------------------------------------------------ front matter

fn parse(file: &str, text: &str) -> Result<Source, String> {
    let text = text.replace("\r\n", "\n");
    let rest = text.strip_prefix("---\n").ok_or_else(|| format!("{file}: the page must start with front matter (a --- line)"))?;
    let end = rest.find("\n---\n").ok_or_else(|| format!("{file}: the front matter has no closing --- line"))?;
    let (front, body) = (&rest[..end], &rest[end + 5..]);
    let mut fields: HashMap<&str, &str> = HashMap::new();
    for line in front.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let (k, v) = line.split_once(':').ok_or_else(|| format!("{file}: front matter line '{line}' is not 'key: value'"))?;
        fields.insert(k.trim(), v.trim());
    }
    let get = |k: &str| fields.get(k).copied().ok_or_else(|| format!("{file}: front matter has no '{k}'"));
    let text_of = |k: &str| -> Result<String, String> {
        let v = get(k)?.trim_matches(|c| c == '"' || c == '\'').to_string();
        if v.is_empty() {
            return Err(format!("{file}: front matter '{k}' is empty"));
        }
        Ok(v)
    };
    let list_of = |k: &str| -> Result<Vec<String>, String> {
        let v = get(k)?;
        let inner = v
            .strip_prefix('[')
            .and_then(|v| v.strip_suffix(']'))
            .ok_or_else(|| format!("{file}: front matter '{k}' must be a list like [a, b]"))?;
        Ok(inner.split(',').map(|x| x.trim().trim_matches(|c| c == '"' || c == '\'').to_string()).filter(|x| !x.is_empty()).collect())
    };
    let id = text_of("id")?;
    let want = file.strip_suffix(".md").unwrap_or(file);
    if id != want {
        return Err(format!("{file}: front matter id '{id}' is not the page's path '{want}'"));
    }
    let keywords = list_of("keywords")?;
    if keywords.is_empty() {
        return Err(format!("{file}: front matter 'keywords' is empty"));
    }
    Ok(Source {
        id,
        title: text_of("title")?,
        summary: text_of("summary")?,
        keywords,
        settings: list_of("settings")?,
        file: file.to_string(),
        body: body.to_string(),
    })
}

// ------------------------------------------------------------------ outline

/// For each page: the area it is listed under in the index page (empty for the index itself and for a page the index
/// does not list, which is put under "Other"), and its place in the index's order.
fn outline(sources: &[Source], ids: &HashMap<String, usize>) -> (Vec<String>, Vec<usize>) {
    let mut group = vec![String::from("Other"); sources.len()];
    let mut order: Vec<Option<usize>> = vec![None; sources.len()];
    let mut n = 0;
    if let Some(&ix) = ids.get("index.md") {
        group[ix] = String::new();
        order[ix] = Some(n);
        n += 1;
        let mut area = String::from("Other");
        let mut in_heading = false;
        for ev in Parser::new(&sources[ix].body) {
            match ev {
                Event::Start(Tag::Heading { level: HeadingLevel::H2, .. }) => {
                    in_heading = true;
                    area.clear();
                }
                Event::End(TagEnd::Heading(_)) => in_heading = false,
                Event::Text(t) | Event::Code(t) if in_heading => area.push_str(&t),
                Event::Start(Tag::Link { dest_url, .. }) => {
                    if let Some(target) = resolve_page(&sources[ix].file, &dest_url) {
                        if let Some(&j) = ids.get(&target) {
                            if order[j].is_none() {
                                order[j] = Some(n);
                                n += 1;
                                group[j] = area.clone();
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut rest: Vec<usize> = (0..sources.len()).filter(|&i| order[i].is_none()).collect();
    rest.sort_by_key(|&i| sources[i].file.clone());
    for i in rest {
        order[i] = Some(n);
        n += 1;
    }
    (group, order.into_iter().map(|o| o.unwrap_or(0)).collect())
}
