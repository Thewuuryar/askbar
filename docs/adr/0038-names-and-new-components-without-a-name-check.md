# 0038. Names and new components, without a name check

Status: Accepted
Date: 2026-11-07
Source: owner decision to delete `scripts/check-names.sh`; supersedes [0021](0021-names-and-new-components.md)

## Context

[0021](0021-names-and-new-components.md) gives every role one name and keeps retired words out with `scripts/check-names.sh`, which refuses "management", "mgmt" and "device" anywhere in the repository unless a line carries `names-ok` or a file is on an allow list. The renames it guarded are done. The check now mostly catches the words in their ordinary meaning, such as a third-party product's own "management API", and each catch needs a marker or an allow-list entry. The owner decided the script is no longer needed.

## Decision

Everything in 0021 stands except the script:

1. Every role has one name, used in API paths, code, environment variables, UI labels, and documents.
2. The architecture glossary defines each name. Reviewers check that changes use it. No script refuses words; "management" and "device" may be used in their ordinary meaning, and for a third party's own terms.
3. A part becomes a named component when it gets its own container.
4. A new component gets a name and an ADR before it exists. Its name must not be confusable with an existing one.
5. A component with its own API audience also needs a path prefix and an OpenAPI file.

`scripts/check-names.sh` and `scripts/names-allow.txt` are deleted and `npm test` no longer runs them. Existing `names-ok` markers have no effect; they are left in place (some are in migration text, which is never edited) and may be removed when their line changes for another reason.

## Alternatives considered

- **Keep the script:** rejected by the owner; the renames are done and it now flags ordinary words.
- **Narrow the script to Askbar's own identifiers:** not chosen; reviewers and the glossary are enough for a project this size.

## Consequences

Retired words for Askbar's own roles can come back only through review. The glossary in [docs/architecture.md](../architecture.md#glossary) stays the one list of names.
