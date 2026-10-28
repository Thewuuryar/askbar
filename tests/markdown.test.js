// Run: node tests/markdown.test.js
const assert = require('node:assert/strict');
const { renderMarkdown: md } = require('../ui/markdown.js');

let passed = 0;
function t(name, fn) {
  try { fn(); passed++; } catch (e) { console.error('FAIL ' + name + '\n  ' + e.message); process.exitCode = 1; }
}
const has = (html, s) => assert.ok(html.includes(s), `expected ${JSON.stringify(s)} in\n${html}`);
const lacks = (html, s) => assert.ok(!html.includes(s), `did not expect ${JSON.stringify(s)} in\n${html}`);

t('escapes raw HTML and script', () => {
  const h = md('<script>alert(1)</script> <img src=x onerror=alert(1)>');
  lacks(h, '<script'); lacks(h, '<img');
  has(h, '&lt;script&gt;');
});

t('only http(s) links become anchors', () => {
  const h = md('[x](javascript:alert(1)) and [ok](https://example.com/a?b=1&c=2)');
  lacks(h, 'href="javascript');
  has(h, '<a href="https://example.com/a?b=1&amp;c=2">ok</a>');
});

t('bare urls link without swallowing trailing punctuation', () => {
  const h = md('see https://example.com/page.');
  has(h, '<a href="https://example.com/page">https://example.com/page</a>.');
});

t('urls with underscores are not italicised', () => {
  const h = md('https://example.com/a_b_c_d and snake_case_name');
  has(h, 'href="https://example.com/a_b_c_d"');
  has(h, 'snake_case_name');
  lacks(h, '<em>');
});

t('inline formatting', () => {
  const h = md('**bold** *it* _it2_ ~~gone~~ `co*de*`');
  has(h, '<strong>bold</strong>'); has(h, '<em>it</em>'); has(h, '<em>it2</em>');
  has(h, '<del>gone</del>'); has(h, '<code>co*de*</code>');
});

t('code spans escape their content', () => {
  has(md('`<b>`'), '<code>&lt;b&gt;</code>');
});

t('headings and rule', () => {
  const h = md('# One\n## Two ##\n---');
  has(h, '<h1>One</h1>'); has(h, '<h2>Two</h2>'); has(h, '<hr>');
});

t('fenced code keeps content literal and has a copy button', () => {
  const h = md('```js\nconst a = "<b>" * 2;\n\n# not a heading\n```\nafter');
  has(h, '<span class="code-lang">js</span>');
  has(h, 'class="copy-code"');
  has(h, 'const a = &quot;&lt;b&gt;&quot; * 2;');
  has(h, '# not a heading');
  has(h, '<p>after</p>');
  lacks(h, '<h1>');
});

t('unterminated fence (mid-stream) still renders as code', () => {
  const h = md('text\n```py\nprint(1)');
  has(h, '<pre><code>print(1)</code></pre>');
});

t('longer fences can contain shorter ones', () => {
  const h = md('````\n```\ninner\n```\n````');
  has(h, '```\ninner\n```');
});

t('lists: bullets, ordered, nested', () => {
  const h = md('- a\n- b\n  - b1\n  - b2\n- c\n\n1. x\n2. y');
  has(h, '<ul><li>a</li><li>b<ul><li>b1</li><li>b2</li></ul></li><li>c</li></ul>');
  has(h, '<ol><li>x</li><li>y</li></ol>');
});

t('list item continuation lines', () => {
  has(md('- one\n  wraps here\n- two'), '<li>one wraps here</li>');
});

t('blockquote renders inner markdown', () => {
  has(md('> **hi**\n> there'), '<blockquote><p><strong>hi</strong><br>there</p></blockquote>');
});

t('tables with alignment', () => {
  const h = md('| A | B |\n|:--|--:|\n| 1 | 2 |\n| x | y |');
  has(h, '<th style="text-align:left">A</th>');
  has(h, '<th style="text-align:right">B</th>');
  has(h, '<td style="text-align:right">2</td>');
  assert.equal((h.match(/<tr>/g) || []).length, 3);
});

t('a pipe in prose is not a table', () => {
  const h = md('a | b\nsecond line');
  lacks(h, '<table');
});

t('paragraph line breaks', () => {
  has(md('one\ntwo\n\nthree'), '<p>one<br>two</p><p>three</p>');
});

t('empty and whitespace input', () => {
  assert.equal(md(''), '');
  assert.equal(md('\n  \n'), '');
});

t('crlf input', () => {
  has(md('a\r\nb'), '<p>a<br>b</p>');
});

t('every prefix of a rich document renders without throwing (streaming)', () => {
  const doc = '# T\n\nSome **bold** and `code` text.\n\n- a\n  - b\n\n```js\nlet x = 1;\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n> quote\n\n[l](https://x.io)';
  for (let i = 0; i <= doc.length; i++) md(doc.slice(0, i));
});

console.log(passed + ' markdown tests passed' + (process.exitCode ? ' (with failures)' : ''));
