// Loads the admin's constants once (plan step 1.2). The values come from /admin/constants.json, which is
// generated from services/admin/src/constants.rs, so no limit or interval is typed in a script. This file
// loads before app.js (base.html); app.js waits for window.askbarConstantsReady before it starts.
(() => {
  'use strict';
  // The one unit conversion the scripts use: a value in seconds times this is milliseconds.
  const MS_PER_SEC = 1000;
  window.askbarMsPerSec = MS_PER_SEC;
  window.askbarConstantsReady = fetch('/admin/constants.json', { credentials: 'same-origin' })
    .then((r) => {
      if (!r.ok) throw new Error('constants ' + r.status);
      return r.json();
    })
    .then((c) => {
      window.askbarConstants = c;
      return c;
    });
})();
