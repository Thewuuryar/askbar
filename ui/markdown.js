// Small, dependency-free Markdown renderer for chat replies.
// Everything is HTML-escaped first, so model output can never inject markup
// or script. Supports: headings, paragraphs, bold/italic/strike, inline code,
// fenced code (with copy button), links, block quotes, nested lists, tables,
// horizontal rules. Tolerates half-finished input, since it re-renders while
// a reply is still streaming.
(function (root) {
  'use strict';

  function esc(s) {
    return s
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  const SENTINEL = '\u0000';

  function inline(text) {
    const stash = [];
    const keep = (html) => SENTINEL + (stash.push(html) - 1) + SENTINEL;

    // Code spans first: nothing inside them is formatted.
    text = text.replace(/`([^`\n]+)`/g, (_, c) => keep('<code>' + esc(c) + '</code>'));

    // Links: [label](https://...) and bare URLs. Only http(s) is linkable.
    text = text.replace(
      /\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)|(https?:\/\/[^\s<>"]*[^\s<>"'.,;:!?)\]])/g,
      (m, label, url, bare) => {
        const href = url || bare;
        const shown = label || bare;
        return keep('<a href="' + esc(href) + '">' + esc(shown) + '</a>');
      }
    );

    text = esc(text);
    text = text
      .replace(/\*\*([^*\n]+?)\*\*/g, '<strong>$1</strong>')
      .replace(/__([^_\n]+?)__/g, '<strong>$1</strong>')
      .replace(/~~([^~\n]+?)~~/g, '<del>$1</del>')
      .replace(/(^|[^*\w])\*([^*\s][^*\n]*?)\*(?![*\w])/g, '$1<em>$2</em>')
      .replace(/(^|[^_\w])_([^_\s][^_\n]*?)_(?![_\w])/g, '$1<em>$2</em>');

    return text.replace(/\u0000(\d+)\u0000/g, (_, i) => stash[Number(i)]);
  }

  function codeBlock(code, lang) {
    const label = lang ? esc(lang) : 'text';
    return (
      '<div class="code"><div class="code-head"><span class="code-lang">' + label +
      '</span><button type="button" class="copy-code">Copy</button></div><pre><code>' +
      esc(code) + '</code></pre></div>'
    );
  }

  const RE_FENCE = /^\s*(`{3,}|~{3,})\s*([\w+#.-]*)\s*$/;
  const RE_HEADING = /^(#{1,6})\s+(.*?)\s*#*\s*$/;
  const RE_HR = /^\s*([-*_])(?:\s*\1){2,}\s*$/;
  const RE_LIST = /^(\s*)([-*+]|\d+[.)])\s+(.*)$/;
  const RE_TABLE_SEP = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

  function splitRow(line) {
    return line.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map((s) => s.trim());
  }

  function renderTable(header, sep, rows) {
    const aligns = splitRow(sep).map((c) => {
      const l = c.startsWith(':');
      const r = c.endsWith(':');
      return l && r ? 'center' : r ? 'right' : l ? 'left' : '';
    });
    const cell = (tag, text, i) =>
      '<' + tag + (aligns[i] ? ' style="text-align:' + aligns[i] + '"' : '') + '>' + inline(text) + '</' + tag + '>';
    let html = '<div class="table-wrap"><table><thead><tr>';
    splitRow(header).forEach((c, i) => (html += cell('th', c, i)));
    html += '</tr></thead><tbody>';
    rows.forEach((r) => {
      html += '<tr>';
      splitRow(r).forEach((c, i) => (html += cell('td', c, i)));
      html += '</tr>';
    });
    return html + '</tbody></table></div>';
  }

  function buildList(items) {
    let pos = 0;
    function level(base) {
      const tag = items[pos].ordered ? 'ol' : 'ul';
      let out = '<' + tag + '>';
      while (pos < items.length && items[pos].indent >= base) {
        const it = items[pos++];
        let body = inline(it.text);
        if (pos < items.length && items[pos].indent > it.indent) body += level(items[pos].indent);
        out += '<li>' + body + '</li>';
      }
      return out + '</' + tag + '>';
    }
    let html = '';
    while (pos < items.length) html += level(items[pos].indent);
    return html;
  }

  function startsBlock(line, next) {
    return (
      RE_FENCE.test(line) ||
      RE_HEADING.test(line) ||
      RE_HR.test(line) ||
      RE_LIST.test(line) ||
      /^\s*>/.test(line) ||
      (line.indexOf('|') !== -1 && next !== undefined && RE_TABLE_SEP.test(next))
    );
  }

  function render(src) {
    const lines = String(src).replace(/\r\n?/g, '\n').split('\n');
    let html = '';
    let i = 0;
    while (i < lines.length) {
      const line = lines[i];

      let m = line.match(RE_FENCE);
      if (m) {
        const marker = m[1][0];
        const len = m[1].length;
        const close = new RegExp('^\\s*\\' + marker + '{' + len + ',}\\s*$');
        const buf = [];
        i++;
        while (i < lines.length && !close.test(lines[i])) buf.push(lines[i++]);
        i++; // closing fence (absent while streaming)
        html += codeBlock(buf.join('\n'), m[2]);
        continue;
      }

      if (line.trim() === '') { i++; continue; }

      if ((m = line.match(RE_HEADING))) {
        const n = m[1].length;
        html += '<h' + n + '>' + inline(m[2]) + '</h' + n + '>';
        i++;
        continue;
      }

      if (RE_HR.test(line)) { html += '<hr>'; i++; continue; }

      if (/^\s*>/.test(line)) {
        const buf = [];
        while (i < lines.length && /^\s*>/.test(lines[i])) buf.push(lines[i++].replace(/^\s*>\s?/, ''));
        html += '<blockquote>' + render(buf.join('\n')) + '</blockquote>';
        continue;
      }

      if (RE_LIST.test(line)) {
        const items = [];
        while (i < lines.length) {
          const lm = lines[i].match(RE_LIST);
          if (lm) {
            items.push({ indent: lm[1].replace(/\t/g, '    ').length, ordered: /\d/.test(lm[2]), text: lm[3] });
            i++;
          } else if (items.length && lines[i].trim() !== '' && /^\s+\S/.test(lines[i]) && !RE_FENCE.test(lines[i])) {
            items[items.length - 1].text += ' ' + lines[i].trim();
            i++;
          } else break;
        }
        html += buildList(items);
        continue;
      }

      if (line.indexOf('|') !== -1 && i + 1 < lines.length && RE_TABLE_SEP.test(lines[i + 1])) {
        const header = line;
        const sep = lines[i + 1];
        const rows = [];
        i += 2;
        while (i < lines.length && lines[i].trim() !== '' && lines[i].indexOf('|') !== -1) rows.push(lines[i++]);
        html += renderTable(header, sep, rows);
        continue;
      }

      const buf = [line];
      i++;
      while (i < lines.length && lines[i].trim() !== '' && !startsBlock(lines[i], lines[i + 1])) buf.push(lines[i++]);
      html += '<p>' + buf.map(inline).join('<br>') + '</p>';
    }
    return html;
  }

  root.renderMarkdown = render;
  if (typeof module !== 'undefined' && module.exports) module.exports = { renderMarkdown: render };
})(typeof window !== 'undefined' ? window : globalThis);
