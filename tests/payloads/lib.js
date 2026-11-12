// The hostile-Markdown library and what a safe rendering looks like (ADR 0016, ADR 0044, plan step 5.2).
// Used by the renderer's Node test, the popup's browser test and the admin's browser test.
'use strict';
const fs = require('node:fs');
const path = require('node:path');

const LIBRARY = path.join(__dirname, 'markdown.json');

/** The cases, each with its `input` text made (a case may generate a large one). */
function cases() {
  const doc = JSON.parse(fs.readFileSync(LIBRARY, 'utf8'));
  return doc.cases.map((c) => {
    let input = c.input;
    if (input === undefined && c.generate) {
      const g = c.generate;
      if (g.repeat) input = (g.repeat.prefix || '') + g.repeat.unit.repeat(g.repeat.times) + (g.repeat.suffix || '');
      else if (g.indented_list) input = Array.from({ length: g.indented_list }, (_, i) => ' '.repeat(i) + '- x').join('\n');
    }
    if (typeof input !== 'string') throw new Error(`payload "${c.name}" has no input`);
    return { ...c, input, max_ms: c.max_ms || 2000 };
  });
}

// What renderMarkdown may produce, and nothing else (see the comment at the top of ui/markdown.js).
const TAGS = new Set('p br strong em del code pre h1 h2 h3 h4 h5 h6 ul ol li blockquote hr a table thead tbody tr th td div span button'.split(' '));
const unescape = (v) => v.replace(/&quot;/g, '"').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');
// For each tag, each attribute it may have, and a pattern its value must match (as a string, so a browser test can use it too).
const ATTRS = {
  a: { href: '^https?://[^\\s\\u0000-\\u001f\\u007f]+$' },
  th: { style: '^text-align:(left|right|center)$' },
  td: { style: '^text-align:(left|right|center)$' },
  div: { class: '^(code|code-head|table-wrap)$' },
  span: { class: '^code-lang$' },
  button: { class: '^copy-code$', type: '^button$' },
};

/**
 * Problems in `html` as a list of sentences (empty when it is safe): a tag or attribute outside the allowlist, an attribute value
 * that is not what the renderer makes, a link that is not http(s), and any text that holds markup or a raw `&`.
 */
function problems(html) {
  const out = [];
  const tag = /<(\/?)([a-z][a-z0-9]*)((?:\s+[a-z-]+="[^"]*")*)\s*>/g;
  let m;
  while ((m = tag.exec(html))) {
    const [, close, name, attrs] = m;
    if (!TAGS.has(name)) out.push(`tag <${name}> is not allowed`);
    if (close) { if (attrs) out.push(`closing tag </${name}> has attributes`); continue; }
    const re = /\s+([a-z-]+)="([^"]*)"/g;
    let a;
    while ((a = re.exec(attrs))) {
      const pattern = (ATTRS[name] || {})[a[1]];
      if (!pattern) out.push(`<${name}> may not have ${a[1]}`);
      else if (!new RegExp(pattern).test(a[1] === 'href' ? unescape(a[2]) : a[2])) out.push(`<${name} ${a[1]}="${a[2].slice(0, 60)}"> is not a value the renderer makes`);
    }
  }
  const text = html.replace(tag, '');
  if (/[<>]/.test(text)) out.push('text holds a raw < or >');
  if (/&(?!amp;|lt;|gt;|quot;)/.test(text.replace(/="[^"]*"/g, ''))) out.push('text holds a raw &');
  return out;
}

/** The links in `html`, as addresses. */
function links(html) {
  return [...html.matchAll(/<a href="([^"]*)">/g)].map((m) => unescape(m[1]));
}

module.exports = { cases, problems, links, TAGS, ATTRS };
