// Hover sweep for the admin pages (docs/style/ui-ux.md, "Hover"). `hoverSweep(page)` hovers the visible items on the page that is
// open, a few of each kind, and returns { checked, misses }: a miss is an item whose background, background image, border colour or
// box-shadow (on itself, its row, or its label) does not change under the pointer. Inert items (disabled or aria-disabled), header
// rows and chart bars are not asked to change.
// Used by tests/e2e/admin.test.js (every page) and by throwaway scripts that print the counts.

const KINDS = [
  ['table row', 'table tbody tr:not(.empty-row):not(.row-confirm-row)', 'tr'],
  ['home widget', '.home-sec', 'self'],
  ['list row', '.alerts-row, .span-row, .sealed-row', 'self'],
  ['nav item', '.sm-item, .tab, .ref-list a, .menu-item, .bell-item, .bell-all', 'self'],
  ['button', 'button:not([disabled]):not([aria-disabled="true"]), a.btn:not([aria-disabled="true"]), a.button', 'self'],
  ['field', 'input:not([type="hidden"]):not([type="checkbox"]):not([type="radio"]):not([type="file"]):not([disabled]):not([readonly]), select:not([disabled]), textarea:not([disabled]):not([readonly])', 'self'],
  ['check or radio', 'input[type="checkbox"]:not([disabled]):not(.bn-row input), input[type="radio"]:not([disabled])', 'label'],
];

async function hoverSweep(page, { perKind = 6 } = {}) {
  // Tag the candidates in the page, so Playwright can address each by an attribute.
  const found = await page.evaluate(({ kinds, perKind }) => {
    const visible = (e) => { const r = e.getBoundingClientRect(); const cs = getComputedStyle(e); return r.width > 0 && r.height > 0 && cs.visibility !== 'hidden' && !e.closest('[hidden]'); };
    let n = 0; const out = [];
    for (const [name, sel, target] of kinds) {
      let list = [...document.querySelectorAll(sel)].filter(visible).filter((e) => !e.closest('.visually-hidden'));
      if (name === 'button') list = list.filter((b) => !b.closest('thead') && !b.closest('.chart') && !b.matches('.btn-primary[aria-disabled]'));
      // A few of each, spread over the list rather than the first few.
      // Rows are taken from every table on the page, a few from each.
      const groups = name === 'table row' ? [...new Set(list.map((e) => e.closest('table')))].map((t) => list.filter((e) => e.closest('table') === t)) : [list];
      for (const g of groups) {
        const step = Math.max(1, Math.floor(g.length / perKind)); let took = 0;
        for (let i = 0; i < g.length && took < perKind; i += step, took++) {
          const e = g[i]; const id = 'hs' + (n++);
          e.setAttribute('data-hs', id); out.push({ name, id, target, text: (e.textContent || e.id || e.name || '').trim().slice(0, 30), tag: e.tagName.toLowerCase() + (e.id ? '#' + e.id : '') });
        }
      }
    }
    return out;
  }, { kinds: KINDS, perKind });

  const snap = (id, target) => page.evaluate(([id, target]) => {
    const e = document.querySelector('[data-hs="' + id + '"]');
    const pick = (x) => { const c = getComputedStyle(x); return [c.backgroundColor, c.backgroundImage, c.borderTopColor, c.boxShadow, c.outlineStyle].join('|'); };
    const parts = [pick(e)];
    window.__bg = getComputedStyle(e).backgroundColor;
    if (target === 'tr') for (const td of e.cells) parts.push(pick(td));
    if (target === 'label') { const l = e.closest('label') || document.querySelector('label[for="' + e.id + '"]'); parts.push(l ? pick(l) : 'nolabel'); }
    return parts.join('||');
  }, [id, target]);
  const bgNow = () => page.evaluate(() => window.__bg);

  const misses = []; const moves = []; let checked = 0;
  for (const f of found) {
    const loc = page.locator('[data-hs="' + f.id + '"]');
    await loc.scrollIntoViewIfNeeded({ timeout: 2000 }).catch(() => {});
    await page.mouse.move(0, 0);
    const rest = await snap(f.id, f.target); const restBg = await bgNow();
    // Hover the middle of a row's first cell, a label's text, or the item itself; skip what another element covers.
    const box = await loc.boundingBox();
    if (!box) continue;
    const point = { x: box.x + Math.min(box.width / 2, 40), y: box.y + box.height / 2 };
    const hit = await page.evaluate(([id, x, y]) => { const t = document.elementFromPoint(x, y); const e = document.querySelector('[data-hs="' + id + '"]'); return !!t && (e.contains(t) || t.closest('label') === e.closest('label') && !!e.closest('label')); }, [f.id, point.x, point.y]);
    if (!hit) continue;
    await page.mouse.move(point.x, point.y);
    let hov = await snap(f.id, f.target);
    if (hov === rest) { await page.waitForTimeout(120); hov = await snap(f.id, f.target); }
    checked++; if (f.name === 'button' || f.name === 'nav item') moves.push(f.tag + ' "' + f.text + '" ' + restBg + ' -> ' + await bgNow());
    if (hov === rest) misses.push(f.name + ' ' + f.tag + ' "' + f.text + '"');
  }
  await page.mouse.move(0, 0);
  await page.evaluate(() => document.querySelectorAll('[data-hs]').forEach((e) => e.removeAttribute('data-hs')));
  return { checked, misses, moves };
}

module.exports = { hoverSweep };
