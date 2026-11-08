#!/usr/bin/env node
// The npm license check (ADR 0026): every package in a package-lock.json must have a license that the
// allowlist in deny.toml covers. Usage: node scripts/check-npm-licenses.js <package-lock.json>...
// An SPDX expression is covered when, for OR, either side is; for AND, both are. "A/B" is read as OR.
'use strict';
const fs = require('fs');
const path = require('path');

function allowlist() {
  const toml = fs.readFileSync(path.join(__dirname, '..', 'deny.toml'), 'utf8');
  const block = toml.match(/^allow\s*=\s*\[([\s\S]*?)^\]/m);
  if (!block) throw new Error('deny.toml has no allow = [ ... ] list');
  return new Set([...block[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]));
}

// A small SPDX expression reader: identifiers (with an optional "WITH exception"), OR, AND, parentheses.
function covered(expr, allowed) {
  const tokens = expr.replace(/\//g, ' OR ').match(/\(|\)|[^\s()]+/g) || [];
  let i = 0;
  const peek = () => tokens[i];
  function primary() {
    const t = tokens[i++];
    if (t === '(') {
      const v = or();
      if (tokens[i++] !== ')') throw new Error('unbalanced parentheses');
      return v;
    }
    if (t === undefined) throw new Error('unexpected end');
    let id = t;
    if (peek() === 'WITH') {
      i++;
      id += ' WITH ' + tokens[i++];
    }
    return allowed.has(id);
  }
  function and() {
    let v = primary();
    while (peek() === 'AND') {
      i++;
      v = primary() && v;
    }
    return v;
  }
  function or() {
    let v = and();
    while (peek() === 'OR') {
      i++;
      v = and() || v;
    }
    return v;
  }
  const v = or();
  if (i !== tokens.length) throw new Error('trailing text');
  return v;
}

function check(lockPath, allowed) {
  const lock = JSON.parse(fs.readFileSync(lockPath, 'utf8'));
  const bad = [];
  for (const [key, pkg] of Object.entries(lock.packages || {})) {
    if (key === '') continue; // the project itself
    const name = key.replace(/^.*node_modules\//, '') + '@' + pkg.version;
    let ok = false;
    try {
      ok = typeof pkg.license === 'string' && covered(pkg.license, allowed);
    } catch (e) {
      ok = false;
    }
    if (!ok) bad.push(`${name}: ${pkg.license || '(no license field)'}`);
  }
  return bad;
}

if (require.main === module) {
  const files = process.argv.slice(2);
  if (files.length === 0) {
    console.error('usage: node scripts/check-npm-licenses.js <package-lock.json>...');
    process.exit(2);
  }
  const allowed = allowlist();
  let failed = 0;
  for (const f of files) {
    const bad = check(f, allowed);
    for (const b of bad) console.log(`FAIL  ${f}: ${b}`);
    if (bad.length === 0) console.log(`ok    ${f}: every package's license is on the allowlist`);
    failed += bad.length;
  }
  process.exit(failed ? 1 : 0);
}

module.exports = { covered, allowlist };
