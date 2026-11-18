# Phase 10: script library (branch `feat/script-library`, deferrable, coarse)

Part of [the tiered routing plan](../plan-tiered-routing.md). Its How to run, Owner gates and Assumptions sections apply to every step here.

Deferrable: starts only when the owner asks. Expand at phase start. Needs Phase 9, G11. Library thresholds (`match_min`, `fit_min`, `candidates`, `demote_below`, `retire_after_failures`) go in `constants.rs`, bounds in `askbar-proto`.

- [ ] **P10.1** `feat(server): add the script catalog and entry format` · sonnet · folder with `SCRIPT.md`, `inputs.json`, `run.*`, `tests/`, `manifest.yaml`; hash.
- [ ] **P10.2** `feat(executor): run tests for a draft in a sandbox` · sonnet · no network beyond the manifest's hosts.
- [ ] **P10.3** `feat(server): author drafts from finished tasks` · sonnet · `library.author` never sees fetched pages or tool output.
- [ ] **P10.4** `feat(admin): review, promote, edit and reject scripts (API)` · sonnet · only the owner promotes; hash pinned.
- [ ] **P10.5** `feat(model-gateway): match requests to scripts in shadow` · sonnet · decider `choice` over all entries (hosted Jev up to 255 options per question, self-hosted passes of 52) with a keyword prefilter above that, then `noul`; no embeddings.
- [ ] **P10.6** `feat(server): fill inputs and validate against the schema` · sonnet
- [ ] **P10.7** `feat(server): run a matched script with a signed approval` · sonnet · dry-run first for writing scripts.
- [ ] **P10.8** `feat(server): track success, demote and retire scripts` · sonnet
- [ ] **P10.9** `test(server): unpromoted or changed scripts do not run` · sonnet · ADR 0016 "Defend".
- [ ] **P10.10** `docs(adr): amend 0016, 0054, 0039, 0040 for the library` · haiku · one commit each.
