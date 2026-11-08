# 0046. Component and register ADRs, partial amendments, and deferral

Status: Accepted
Date: 2026-11-08
Source: owner request in review of plan step 1.14

## Context

Under [0001](0001-decision-records-and-change-rules.md) rule 2, an ADR on `main` changes only in its status line, and a changed decision needs a new ADR that supersedes the old one. [0035](0035-editorial-edits-to-accepted-adrs.md) allows editorial edits and nothing else. That suits a decision, which should not change once made. It does not suit three things the project now has or will soon need:

- **Component ADRs** (0036, 0037, 0039 to 0045, and one for each new component) describe how a component is built: what it holds, what it can reach, its volumes, networks and links. That changes whenever the component does. Superseding the ADR each time would scatter one component's current shape across many records.
- **The risk register** ([0016](0016-risk-register.md)) is a list that grows: it already says a new accepted risk is added "through the change rules in 0001", which today means superseding the whole register. Phase 7's ship gate needs one current list of Defend controls and their tests.
- **One rule in a decision ADR** sometimes has to change while the rest stands. Plan step 1.11 moved the generation routes under `/spaces/{space_id}`, and the space route list in [0022](0022-api-path-conventions.md) does not name them; superseding all of 0022 for that list repeats everything else in it.

[0002](0002-v1-scope-limits-and-ship-criteria.md) also sets a defer order (iOS and memory, mismatch alerts, scheduled backups to several destinations, shared spaces). An ADR written for one of those and then held back by the owner is neither Proposed work nor Rejected; [0027](0027-adr-format-and-numbering.md) has no status for it.

## Decision

This ADR adds the following to 0001 and 0027. Their other rules stand, and neither is superseded.

**Kinds.** An ADR is a *decision* ADR unless its header has a `Kind:` line:

- `Kind: component` describes one component, as the plan defines it (a container, a library crate or a client). 0036, 0037 and 0039 to 0045 are component ADRs.
- `Kind: register` keeps a list that the project maintains over time. 0016 is a register.

Component and register ADRs end with an `## Amendments` section. Once this ADR is accepted, 0016, 0036 and 0037 get their `Kind:` line and an empty Amendments section in one commit, `docs(adr): amend 0016, 0036, 0037: add the kind marker`; that is their first amendment.

**Amending in place** (component and register ADRs only):

1. An accepted component or register ADR is edited in place when what it describes changes. For a component, the edit goes in the same pull request as the code where possible.
2. Each amendment is its own commit, `docs(adr): amend NNNN: <what>`, and the pull request lists it. The owner's merge is the approval.
3. Each amendment adds a line at the end of the Amendments section: the date, what changed, and the pull request or decision ADR behind it. The body always says how things are now; the list says how they got there.
4. An amendment never makes a decision. A new or changed decision goes in a decision ADR under 0001 (or a partial amendment, below), and the component or register ADR is amended to link it. For a register, adding, changing or removing an entry is the amendment; the owner's merge is the approval 0016 asks for.
5. A retired component's ADR gets `Status: Retired` and a `Retired:` line with the date and the reason, instead of being superseded. Its body stays as the last description of the component.

**Partial amendments** (decision ADRs):

- A new decision ADR may replace one part of an accepted decision ADR instead of all of it. Its header has an `Amends:` line naming the ADR and the part, for example `Amends: [0022](0022-api-path-conventions.md), the space route list`, and its Decision says the part's new wording in full.
- The amended ADR's status line becomes `Status: Accepted; amended by MMMM`, with every amending ADR listed in order. Nothing else in it changes, and its index row shows the same status.
- A change to more than one part, or to what the ADR is for, still supersedes it.

**Deferral:** a Proposed ADR that the owner holds back gets `Status: Deferred` and a `Deferred:` line with the date and the reason. Taking it up again returns it to `Proposed`. Deferred is added to the statuses in 0027.

**What a script checks** (`scripts/check-docs.sh`): every `Kind: component` or `Kind: register` ADR has an `## Amendments` section; every `Amends:` line and every `amended by` in a status line names ADRs that exist, and each amending ADR names the ADR it amends; a Retired or Deferred ADR has its `Retired:` or `Deferred:` line; and, as before, each index row's status matches the ADR's status line.

## Alternatives considered

- **Superseding a component or register ADR on every change:** rejected. The current shape of one component, or the current list of risks, would be spread over several records, and finding it would mean reading all of them.
- **Keeping component ADRs Proposed so that 0001 rule 3 lets them change freely:** rejected. Nothing would ever be accepted, so there would be no reviewed baseline.
- **Moving component descriptions out of the ADRs (`docs/components/`):** rejected. It drops the owner rule that each component has an ADR, which the plan relies on.
- **Editing owner-set numbers (budget, target date, retention defaults) in place:** rejected. They are decisions; a partial amendment records them cheaply enough.
- **Widening 0035's editorial edits:** rejected. Its narrow limits are what keep "editorial" from becoming a way to change meaning.

## Consequences

- A component ADR shows how the component is built now, and its Amendments list shows how it got there; the reasons stay in decision ADRs.
- A one-rule change to a decision ADR costs a short ADR, not a copy of the whole one.
- A reviewer checks that an amendment makes no decision; no script can. `check-docs.sh` enforces the markers, the links between amending and amended ADRs, and the status lines.
- The plan's rule for retiring a component follows this ADR once it is accepted.
