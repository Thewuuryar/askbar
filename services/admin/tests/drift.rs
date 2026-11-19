//! Drift checks for the admin site's scripts and templates (plan step 8.3). They read the source files, so
//! `cargo test -p askbar-admin` runs them. Each one fails on a new violation and on an exemption that no longer
//! matches anything (a stale exemption is a drift too).
//!
//! - Components: a page template does not hand-write markup that a component in `components.html` already covers.
//! - Literals: no script or template types in a number that `constants.json` defines as a limit, interval or size.
//! - Addresses: every `/admin/<slug>/` in the scripts and templates names a page in `PAGE_SLUGS`.
use askbar_admin::constants::{json, PAGE_SLUGS};
use serde_json::Value;
use std::path::PathBuf;

/// The scripts and templates the checks read: the four scripts, then every template. `icons.html` is left out of
/// every check: it is SVG geometry, the one place an icon is drawn, so its numbers are coordinates.
fn sources() -> Vec<(String, String)> {
    let dir = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui"));
    let mut names: Vec<String> = ["app.js", "audit.js", "audit_calls.js", "components.js"].map(String::from).to_vec();
    let mut templates: Vec<String> = std::fs::read_dir(dir.join("templates"))
        .expect("templates")
        .map(|e| e.expect("entry").file_name().into_string().expect("utf-8 name"))
        .filter(|n| n.ends_with(".html") && n != "icons.html")
        .collect();
    templates.sort();
    names.extend(templates.into_iter().map(|t| format!("templates/{t}")));
    names
        .into_iter()
        .map(|n| {
            let text = std::fs::read_to_string(dir.join(&n)).unwrap_or_else(|e| panic!("{n}: {e}"));
            (n, text)
        })
        .collect()
}

/// `text` with the comments blanked, newlines kept so line numbers hold: `{# #}` and `<!-- -->` in a template; `/* */`
/// and a `//` that starts a line or follows a space in a script (a `//` in a URL stays).
fn without_comments(name: &str, text: &str) -> String {
    if name.ends_with(".html") {
        blank_between(&blank_between(text, "{#", "#}"), "<!--", "-->")
    } else {
        blank_between(text, "/*", "*/").lines().map(line_comment).collect::<Vec<_>>().join("\n")
    }
}

fn blank_between(text: &str, open: &str, close: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(open) {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let end = after.find(close).map_or(after.len(), |j| j + close.len());
        out.extend(after[..end].chars().map(|c| if c == '\n' { '\n' } else { ' ' }));
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn line_comment(line: &str) -> &str {
    let b = line.as_bytes();
    (0..b.len().saturating_sub(1))
        .find(|&i| b[i] == b'/' && b[i + 1] == b'/' && (i == 0 || b[i - 1].is_ascii_whitespace()))
        .map_or(line, |i| &line[..i])
}

/// Each (file, marker, context, reason). A hand-written marker is accepted when its enclosing tag, or the text just
/// after it, holds `context`. An entry that matches nothing fails the test.
const MARKUP_EXEMPT: &[(&str, &str, &str, &str)] = &[
    (
        "templates/backup.html",
        "class=\"notice ",
        "id=\"sched-reminder\"",
        "the scheduled-backup reminder is shown and filled by id from app.js; notice() takes no id, so it keeps its own markup until its script moves",
    ),
    (
        "templates/providers.html",
        "<div class=\"field\"><label for=",
        "cpa-redirect",
        "the CLIProxyAPI paste box needs autocomplete=\"off\", which field() cannot set",
    ),
];

/// A marker that is a component's own markup written out by hand. `unless` names a part of the enclosing tag that
/// makes the hit legitimate: a status() cannot take an id, so a status a script fills (it has one) is not one.
struct Covered {
    markers: &'static [&'static str],
    component: &'static str,
    reason: &'static str,
    unless: &'static str,
}

const COVERED: &[Covered] = &[
    Covered {
        markers: &["class=\"empty-state\""],
        component: "empty()",
        reason: "the empty state is one block with one optional button",
        unless: "",
    },
    Covered {
        markers: &["class=\"new-secret\""],
        component: "new_secret()",
        reason: "a new password is two fields with a length hint",
        unless: "",
    },
    Covered {
        markers: &["class=\"save-bar\""],
        component: "save_bar()",
        reason: "the unsaved-changes bar has one text, Discard and Save",
        unless: "",
    },
    Covered {
        markers: &["class=\"secret\"", "class=\"secret-value\""],
        component: "secret()",
        reason: "a show-once key and its copy button",
        unless: "",
    },
    Covered {
        markers: &["class=\"side-panel\""],
        component: "side_panel()",
        reason: "the docked panel has a fixed head with a close button",
        unless: "",
    },
    Covered { markers: &["<dialog"], component: "dialog_box()", reason: "a modal box has a labelled title and body", unless: "" },
    Covered {
        markers: &["class=\"row-confirm\""],
        component: "row_confirm()",
        reason: "the in-row confirmation has Cancel and one danger action",
        unless: "",
    },
    Covered {
        markers: &["class=\"skeleton\""],
        component: "skeleton()",
        reason: "the loading placeholder holds a hidden Loading word",
        unless: "",
    },
    Covered {
        markers: &["class=\"page-header\"", "class=\"page-title\""],
        component: "page_header()",
        reason: "the page's one title, lede and actions",
        unless: "",
    },
    Covered {
        markers: &["class=\"tabs\""],
        component: "tabs()",
        reason: "the tab bar is built from PAGES, so it matches the menu",
        unless: "",
    },
    Covered { markers: &["class=\"row-menu\""], component: "row_menu()", reason: "the row's ... button and its menu", unless: "" },
    Covered { markers: &["data-help="], component: "help()", reason: "a ? button names its topic and opens the side panel", unless: "" },
    Covered { markers: &["<svg"], component: "icon()", reason: "an icon is drawn once, in icons.html", unless: "" },
    Covered {
        markers: &["class=\"skip-link\""],
        component: "skip_link()",
        reason: "the skip link is the first stop in the frame",
        unless: "",
    },
    Covered {
        markers: &["class=\"toasts\"", "class=\"banners\"", "id=\"announcer\""],
        component: "toast_region(), banner_region(), announcer()",
        reason: "the frame's live regions",
        unless: "",
    },
    Covered {
        markers: &["class=\"status status-"],
        component: "status()",
        reason: "a status is a symbol and a word; one that a script fills has an id, which status() cannot give",
        unless: " id=",
    },
    Covered {
        markers: &["class=\"notice "],
        component: "notice()",
        reason: "a notice has an icon, a hidden kind word and a text",
        unless: "",
    },
    Covered {
        markers: &["<div class=\"field\"><label for="],
        component: "field()",
        reason: "a labelled control with its hint and error line",
        unless: "",
    },
    Covered {
        markers: &[
            "<button type=\"submit\" class=\"btn btn-primary\">",
            "<button type=\"button\" class=\"btn btn-primary\">",
            "<button type=\"button\" class=\"btn btn-secondary\">",
            "<button type=\"button\" class=\"btn btn-danger\">",
        ],
        component: "button()",
        reason: "a button with a kind and no other attribute (button() cannot add an id or a form attribute; those are not checked)",
        unless: "",
    },
    Covered {
        markers: &["<button type=\"button\" class=\"btn btn-quiet btn-icon\" aria-label="],
        component: "icon_button()",
        reason: "an icon-only button with its name and tooltip",
        unless: "",
    },
    Covered {
        markers: &["class=\"menu\" role=\"menu\""],
        component: "menu()",
        reason: "a popup menu with its role and hidden state",
        unless: "",
    },
    Covered { markers: &["class=\"na\""], component: "na()", reason: "a greyed item: aria-disabled, focusable, with the note", unless: "" },
    Covered { markers: &["class=\"na-tag\""], component: "na_tag()", reason: "the small Not available yet tag", unless: "" },
];

/// Whitespace runs become one space, and spaces next to a tag go, so a marker matches however the markup is laid out.
fn normalise(text: &str) -> String {
    let mut s = text.split_whitespace().collect::<Vec<_>>().join(" ");
    loop {
        let t = s.replace("> <", "><").replace("< ", "<").replace(" >", ">");
        if t == s {
            return s;
        }
        s = t;
    }
}

/// The tag that holds byte `at` of `s`, from its `<` to its `>`.
fn enclosing_tag(s: &str, at: usize) -> &str {
    let start = s[..at].rfind('<').unwrap_or(0);
    let end = s[at..].find('>').map_or(s.len(), |j| at + j + 1);
    &s[start..end]
}

#[test]
fn no_template_hand_writes_markup_a_component_covers() {
    let mut bad = Vec::new();
    let mut used = vec![false; MARKUP_EXEMPT.len()];
    for (name, text) in sources() {
        // components.html is where the components are written; the scripts build their DOM and the e2e suite checks it.
        if !name.starts_with("templates/") || name == "templates/components.html" {
            continue;
        }
        let s = normalise(&without_comments(&name, &text));
        for c in COVERED {
            for marker in c.markers {
                for (at, _) in s.match_indices(marker) {
                    let tag = enclosing_tag(&s, at);
                    if !c.unless.is_empty() && tag.contains(c.unless) {
                        continue;
                    }
                    let after = s.get(at..(at + 160).min(s.len())).unwrap_or("");
                    let exempt = MARKUP_EXEMPT
                        .iter()
                        .position(|(f, m, ctx, _)| *f == name && m == marker && (tag.contains(ctx) || after.contains(ctx)));
                    match exempt {
                        Some(i) => used[i] = true,
                        None => bad.push(format!(
                            "{name}: `{marker}` is hand-written; use {} ({}): {}",
                            c.component,
                            c.reason,
                            &s[at..(at + 70).min(s.len())]
                        )),
                    }
                }
            }
        }
    }
    assert!(bad.is_empty(), "hand-written markup a component covers:\n{}", bad.join("\n"));
    let stale: Vec<String> =
        MARKUP_EXEMPT.iter().zip(&used).filter(|(_, u)| !**u).map(|((f, m, c, _), _)| format!("{f}: `{m}` with `{c}`")).collect();
    assert!(stale.is_empty(), "markup exemptions that match nothing; remove them:\n{}", stale.join("\n"));
}

/// Numbers a script or template may not type in: every numeric value in `constants.json`, except the allowed ones.
/// 0, 1, 2, 10 and 100 are everyday digits; 60, 1000, 3600 and 24 are time-unit maths.
const ALLOWED_LITERALS: &[f64] = &[0.0, 1.0, 2.0, 10.0, 100.0, 60.0, 1000.0, 3600.0, 24.0];

/// Attributes whose number is a size or a count of layout rows, not a limit (CSS-like): `rows="3"` is a textarea's height.
const SIZE_ATTRS: &[&str] = &["rows", "cols", "width", "height", "tabindex", "colspan", "rowspan", "stroke-width"];

/// Each (file, literal, context, reason). A literal on a line that holds `context` is accepted. An entry that matches
/// nothing fails the test. Every entry is a decision, recorded here; a new literal of the same value fails.
const LITERAL_EXEMPT: &[(&str, &str, &str, &str)] = &[
    ("app.js", "3", "setStep(3", "the setup card's third screen (step 3 of 3): a screen index, not a limit"),
    (
        "app.js",
        "200",
        "const DISMISSED_MAX = 200",
        "how many dismissed alerts this browser remembers (localStorage): a per-browser cap, not a server limit",
    ),
    ("components.js", "365", "const UNITS", "calendar units for the relative time text (a year and a month, in seconds), not a limit"),
    ("components.js", "30", "const UNITS", "calendar units for the relative time text (a month, in days), not a limit"),
    ("templates/audit_calls.html", "7", "value=\"7\"", "the Model calls window choice (last 7 days): a filter option, not a limit"),
    ("templates/audit_calls.html", "30", "value=\"30\"", "the Model calls window choice (last 30 days): a filter option, not a limit"),
    ("templates/audit_routing.html", "7", "value=\"7\"", "the Routing window choice (last 7 days): a filter option, not a limit"),
    ("templates/audit_routing.html", "30", "value=\"30\"", "the Routing window choice (last 30 days): a filter option, not a limit"),
    ("templates/audit_calls.html", "3", "skeleton(3)", "placeholder rows while the trace loads: decorative, not a limit"),
    ("templates/live.html", "3", "skeleton(3)", "placeholder rows in the components reference: decorative, not a limit"),
    ("components.js", "6", "/^H[1-6]$/", "the heading levels h1 to h6 in a regular expression, not a limit"),
    ("templates/base.html", "3", "3. Backup passphrase", "the number of the third setup step in its list (1, 2, 3): a label, not a limit"),
];

/// Every numeric leaf of the served constants JSON.
fn json_numbers(v: &Value, out: &mut Vec<f64>) {
    match v {
        Value::Number(n) => out.push(n.as_f64().expect("a finite number")),
        Value::Array(a) => a.iter().for_each(|x| json_numbers(x, out)),
        Value::Object(o) => o.values().for_each(|x| json_numbers(x, out)),
        _ => {}
    }
}

/// The numbers a script or template may not type in, from `constants.json`.
fn forbidden_literals() -> Vec<f64> {
    let v: Value = serde_json::from_str(json()).expect("constants.json parses");
    let mut all = Vec::new();
    json_numbers(&v, &mut all);
    all.retain(|n| !ALLOWED_LITERALS.contains(n));
    all.sort_by(f64::total_cmp);
    all.dedup();
    assert!(!all.is_empty(), "the constants define numbers to guard");
    all
}

/// The number tokens of a line that stand alone: not part of a name, a hex value, a version, a URL port, an id (`#`)
/// or a longer number, and not a size attribute's value.
fn number_tokens(line: &str) -> Vec<&str> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let joins = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b"_#$./:&".contains(&b[i - 1]));
        if !b[i].is_ascii_digit() || joins {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
            i += 1;
        }
        let tok = &line[start..i];
        let continues = matches!(b.get(i), Some(c) if c.is_ascii_alphanumeric() || *c == b'_' || *c == b'%');
        let size_attr = line[..start].strip_suffix("=\"").is_some_and(|p| {
            let name = p.rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).next().unwrap_or("");
            SIZE_ATTRS.contains(&name)
        });
        if !continues && !size_attr {
            out.push(tok);
        }
    }
    out
}

#[test]
fn no_script_or_template_types_in_a_number_the_constants_define() {
    let forbidden = forbidden_literals();
    let mut bad = Vec::new();
    let mut used = vec![false; LITERAL_EXEMPT.len()];
    for (name, original) in sources() {
        // The comments are blanked, so their numbers are not read; the raw line is what an exemption's context matches.
        let stripped = without_comments(&name, &original);
        for (ln, (line, raw)) in stripped.lines().zip(original.lines()).enumerate() {
            for tok in number_tokens(line) {
                let lit = tok.trim_end_matches('.');
                let Ok(v) = lit.parse::<f64>() else { continue };
                if !forbidden.contains(&v) {
                    continue;
                }
                let exempt = LITERAL_EXEMPT.iter().position(|(f, l, ctx, _)| *f == name && *l == lit && raw.contains(ctx));
                match exempt {
                    Some(i) => used[i] = true,
                    None => bad.push(format!("{name}:{}: {lit} is a limit in constants.json; read it from K / ui.constants (or add an exemption with a reason): {}", ln + 1, raw.trim())),
                }
            }
        }
    }
    assert!(bad.is_empty(), "typed-in numbers that constants.json defines:\n{}", bad.join("\n"));
    let stale: Vec<String> =
        LITERAL_EXEMPT.iter().zip(&used).filter(|(_, u)| !**u).map(|((f, l, c, _), _)| format!("{f}: {l} on `{c}`")).collect();
    assert!(stale.is_empty(), "literal exemptions that match nothing; remove them:\n{}", stale.join("\n"));
}

#[test]
fn every_page_address_in_the_scripts_and_templates_names_a_page() {
    let mut bad = Vec::new();
    for (name, text) in sources() {
        let text = without_comments(&name, &text);
        for (at, _) in text.match_indices("/admin/") {
            if text[..at].ends_with("/api") {
                continue; // `/api/admin/v1/...` is the admin's API, not a page
            }
            let run: String = text[at + "/admin/".len()..]
                .chars()
                .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '/' | '-' | '_'))
                .collect();
            if !run.ends_with('/') {
                continue; // a file (`/admin/app.js`) or the bare `/admin/`
            }
            let slug = run.trim_end_matches('/');
            if slug == "reference" || slug.starts_with("reference/") || PAGE_SLUGS.contains(&slug) {
                continue;
            }
            bad.push(format!("{name}: /admin/{run} is not a page in constants::PAGE_SLUGS"));
        }
    }
    assert!(bad.is_empty(), "page addresses that name no page:\n{}", bad.join("\n"));
}
