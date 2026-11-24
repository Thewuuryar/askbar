'use strict';
// Ports come from a band below the kernel's ephemeral range, so outgoing connections
// (fetches, service clients, Chromium) never take one between the check and the real bind.
const fs = require('node:fs');
const net = require('node:net');

let low = 32768;
try { low = Number(fs.readFileSync('/proc/sys/net/ipv4/ip_local_port_range', 'utf8').split(/\s+/)[0]) || low; } catch { /* not Linux: assume the default */ }
const MIN = 20000;
const max = Math.max(low - 1000, MIN + 1000) - 1;
const handed = new Set();

const bindable = (port) => new Promise((resolve) => {
  const s = net.createServer().once('error', () => resolve(false));
  s.listen(port, '127.0.0.1', () => s.close(() => resolve(true)));
});

async function freePort() {
  for (let i = 0; i < 200; i++) {
    const port = MIN + Math.floor(Math.random() * (max - MIN + 1));
    if (handed.has(port)) continue;
    handed.add(port); // reserve before awaiting: concurrent callers (Promise.all) must not pick it too
    if (await bindable(port)) return port;
  }
  throw new Error('freePort: no free port in ' + MIN + '-' + max + ' after 200 tries');
}

module.exports = { freePort };
