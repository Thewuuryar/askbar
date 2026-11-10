# 0053. Amending any ADR in place

Status: Accepted
Date: 2026-11-10
Source: owner request, 2026-11-10

## Context

Four ADRs set how an accepted ADR may change, and together they make most changes expensive:

- [0001](0001-decision-records-and-change-rules.md) rule 2: once on `main`, a decision changes only through a new ADR that supersedes it.
- [0035](0035-editorial-edits-to-accepted-adrs.md): editorial edits only (a path, a link, a typo, a glossary rename).
- [0046](0046-component-and-register-adrs-and-partial-amendments.md): component and register ADRs are amended in place, with an Amendments list. A decision ADR can only be changed in part by a new ADR with an `Amends:` line.
- [0027](0027-adr-format-and-numbering.md): the format and the statuses.

The result is chains a reader must follow to learn one rule. The space route list is in 0022, changed by 0047 and again by 0048. The container exceptions are in 0032 and 0050. The ADR rules themselves are spread over these four records. The amendment list works well for component ADRs: the body says how things are now, and the list says how they got there. The owner wants the same for every ADR, so that the set can be consolidated and kept short.

## Decision

This ADR changes 0001 rule 2, 0035 and 0046 as below. Their other rules stand until they are folded into one ADR (see Consequences).

1. **Any accepted ADR may be amended in place:** a decision, component or register ADR, when what it decides or describes changes. The body always says how things are now.
2. **Every ADR ends with `## Amendments`,** empty until the first amendment. Each amendment adds one line there: the date, what changed and why, and the pull request.
3. **Each amendment is its own commit,** `docs(adr): amend NNNN: <what>`, and the pull request lists it.
4. **Approval:**
   - **A change to a decision** (what is decided, a rule, a scope, a consequence) needs the owner's approval in the pull request. The owner's merge is that approval.
   - **A change only to a description** (how a component is built now, an entry in a register) is approved by the merge, as under 0046.
   - Agents can draft amendments but not approve them (0001 rule 7).
5. **Reversals:** an amendment that reverses a decision says, in its amendment line, what changed since the decision was made (0001 rule 4).
6. **Superseding:** this is kept for when an ADR's subject is replaced by a different decision. The old ADR keeps its text and gets `Status: Superseded by NNNN`.
7. **Merging:** one ADR may be folded into another, to keep related rules in one place.
   - The folded ADR stays as a short stub, so every link to its number still works. It has `Status: Merged into NNNN`, a `Merged:` line (the date and the section where its content now lives), and one sentence.
   - The receiving ADR gets an amendment line naming what came in.
   - Numbers are never reused or removed.
8. **Editorial edits** (0035's list) need no amendment line. Each goes in its own commit, `docs(adr): editorial: <what>`.
9. **No new partial-amendment ADRs** (`Amends:` lines): a change to part of an ADR is an in-place amendment.
10. **Unchanged from 0046:**
    - `Kind: component` and `Kind: register` labels;
    - `Status: Retired` and `Status: Deferred`, with their `Retired:` and `Deferred:` lines.

**Checked by `scripts/check-docs.sh`** once this ADR is accepted:
- every ADR other than a stub has an `## Amendments` section;
- a `Merged into NNNN` stub has a `Merged:` line and names an ADR that exists and is not itself a stub;
- that ADR's Amendments section mentions the stub's number;
- `Merged` is a known status in the index.

## Alternatives considered

- **Keep 0046 as it is (in place for component and register ADRs only):** rejected. Decision ADRs keep growing chains, and the owner wants one current text per topic.
- **Edit decision ADRs in place with no record:** rejected. The history of a decision would live only in git, and a reader could not tell from the ADR that it changed.
- **Delete folded ADRs and rewrite every link:** rejected by the owner. Code comments, docs, commit messages and pull requests cite the numbers. A stub keeps them working.
- **Supersede the four ADR-process records with one new ADR:** possible, but it adds a fifth record about records. Folding them into 0001, the one AGENTS.md already cites, keeps the number people know.

## Consequences

- An ADR shows the current decision, and its Amendments list shows how it got there. A reader no longer follows chains.
- Once this is accepted, the planned consolidation goes ahead:
  - 0027, 0035, 0046 and this ADR fold into 0001;
  - 0047 and 0048 fold into 0022, and 0050 into 0032;
  - four topic pairs merge: 0034 into 0015, 0026 into 0017, 0030 into 0011, and 0025 and 0031 into 0010;
  - every ADR is trimmed, with no decision changed.
- A decision is now easier to change, so review matters more. The owner's approval is still required, and the amendment line makes each change visible.
- Enforced by `scripts/check-docs.sh` (the markers, stubs and index) and by review (whether an amendment changes a decision). No script can tell a decision from a description.

## Amendments
