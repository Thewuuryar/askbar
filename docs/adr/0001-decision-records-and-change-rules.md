# 0001. Decision records and change rules

Status: Accepted
Date: 2026-11-04
Plan decisions: 21

## Context

Askbar needs one place to record decisions that a later contributor, person or AI agent, could otherwise reopen. The owner approves decisions. The ADRs must stay short, current and easy to find. A reader should learn a rule from one ADR, not from a chain of records that change each other.

## Decision

**Approval and agents**

1. **Approval:** only the owner moves an ADR from Proposed to Accepted. A Proposed ADR can be edited freely until then.
2. **AI agents** can draft ADRs and amendments and open pull requests. They cannot accept an ADR, approve a change to a decision, merge, or change workflows without the owner's approval. This rule is kept through 1.0, then reviewed.

**When an ADR is required**

3. **Required for:**
   - any change to a boundary, container, network, volume, API audience or hard rule;
   - any new dependency that is an image or a service, or is outside the license allowlist ([0026](0026-dependency-license-check.md)).

   A library inside the allowlist needs a reason in the pull request instead.
4. **A change to a security rule** brings its docs and isolation tests in the same pull request.
5. **Emergencies:** a security fix can ship first, with its ADR following within 7 days.

**Format**

6. **Files:** `NNNN-short-title.md`, numbered in order. A number is never reused or removed.
7. **Header:** a title, `Status:`, `Date:`, and optionally `Source:` and `Kind:`.
8. **Sections:** Context, Decision, Alternatives considered, Consequences, and Amendments, in that order.
   - An invariant is written so that a test or a script can check it, and Consequences say how it is enforced.
   - Where the reasoning is unknown, the ADR says "Not recorded" rather than inventing one.
9. **Keep it short:** link to other docs instead of repeating them, and state each fact once.
10. **Kinds:** an ADR is a decision ADR unless its header says otherwise.
    - `Kind: component` describes one component (a container, a library crate or a client) as it is built now.
    - `Kind: register` keeps a list the project maintains, such as the risk register ([0016](0016-risk-register.md)).
11. **Statuses:**
    - Proposed, Accepted and Rejected.
    - `Superseded by NNNN`.
    - `Merged into NNNN` (rule 16).
    - `Deferred`, with a `Deferred:` line giving the date and the reason. A Proposed ADR the owner holds back is Deferred; taking it up again returns it to Proposed.
    - `Retired`, with a `Retired:` line giving the date and the reason. A component ADR whose component is gone is Retired, and its body stays as the last description of it.
12. **The index:** `docs/adr/README.md` lists every ADR with its status.

**Changing an accepted ADR**

13. **Amend in place:** any accepted ADR may be changed in place when what it decides or describes changes.
    - The body always says how things are now.
    - Its `## Amendments` section says how it got there: one line per amendment, giving the date, what changed and why, and the pull request.
    - For a component ADR, the amendment goes in the pull request with the code where it can.
14. **One commit per amendment,** `docs(adr): amend NNNN: <what>`, listed in the pull request.
    - **A change to a decision** (what is decided, a rule, a scope or a consequence) needs the owner's approval in the pull request. The owner's merge is that approval.
    - **A change only to a description** (how a component is built now, an entry in a register) is approved by the merge.
    - **A reversal** says, in its amendment line, what changed since the decision was made.
15. **Superseding:** when an ADR's subject is replaced by a different decision, a new ADR supersedes it. The old ADR keeps its text, and its status becomes `Superseded by NNNN`.
16. **Merging:** an ADR may be folded into another, to keep related rules in one place.
    - The folded ADR stays as a short stub, so links to its number still work. It has `Status: Merged into NNNN`, a `Merged:` line (the date and the section where its content now lives), and one sentence.
    - The receiving ADR gets an amendment line naming what came in.
17. **Editorial edits** need no amendment line: a moved file path, a broken or moved link, a typo, or a term renamed through the [glossary](../architecture.md#glossary). Each is its own commit, `docs(adr): editorial: <what>`, and must change no meaning.

## Alternatives considered

- **Immutable ADRs, changed only by superseding:** the original rule, replaced in [0053](0053-amending-adrs-in-place.md). Each change added a record, and one rule had to be read across a chain (0022, 0047, 0048).
- **In-place amendments for component and register ADRs only, with partial-amendment ADRs for decisions:** the rule of [0046](0046-component-and-register-adrs-and-partial-amendments.md), replaced in 0053 for the same reason.
- **Editing ADRs in place with no record:** rejected. The history would live only in git, and a reader could not tell from the ADR that it changed.
- **Moving component descriptions to `docs/components/`:** rejected. Each component has an ADR, and the plan relies on that.
- **Deleting folded ADRs and rewriting every link:** rejected. Code, docs, commits and pull requests cite the numbers.

## Consequences

- Each ADR shows the current decision, and its Amendments list shows how it got there. A decision is easier to change, so review matters: the owner's approval and the amendment line make each change visible.
- Agents can prepare changes, but the owner stays responsible for approvals, merges and workflow changes through 1.0.
- **Enforced by `scripts/check-docs.sh`:**
  - every ADR has a row in the index, and its status matches;
  - every ADR other than a stub has `## Amendments`;
  - a `Kind:` is `component` or `register`;
  - a Deferred or Retired ADR has its dated line;
  - a `Merged into NNNN` stub has a `Merged:` line and names an ADR that exists, is not a stub, and mentions it in its Amendments;
  - numbers run from 0001 with no gaps.
- **By review:** whether an amendment changes a decision (no script can tell), and whether an editorial edit changes meaning.

## Amendments

- 2026-11-07: editorial edits allowed ([0035](0035-editorial-edits-to-accepted-adrs.md)).
- 2026-11-08: component and register ADRs amended in place; partial amendments; Deferred and Retired ([0046](0046-component-and-register-adrs-and-partial-amendments.md)).
- 2026-11-10: any ADR amended in place, and merging ([0053](0053-amending-adrs-in-place.md)).
- 2026-11-10: merged in 0027 (format and numbering: rules 6 to 12), 0035 (editorial edits: rule 17), 0046 (kinds, Deferred and Retired: rules 10 and 11) and 0053 (amending and merging: rules 13 to 16); the rules are one list.
