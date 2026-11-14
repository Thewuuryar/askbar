# Plan: release branches, and patches beside the next minor

Status: proposed. Not built, and not decided: it changes the release rule in [ADR 0018](adr/0018-releases-and-migrations.md), so the owner decides first.

## Goal

After `v1.0.0-rc.1` there is a week of testing, then more release candidates for fixes, then `v1.0.0`, then patch releases (1.0.1, 1.0.2). While that happens, features for 1.1 are built, and none of them reaches a 1.0 release.

## Best practice this plan follows

This is trunk-based development with release branches, the usual setup for software that ships versioned releases (Kubernetes, Go and Django work this way).

- **Fix in `main` first, then backport.** The next release then has the fix too. Fix on the release branch directly only for a bug that exists only there.
- **Cut the release branch late, at the first rc,** so `main` stays open for features until the last moment.
- **Never merge a release branch back into `main`.** Only cherry-picks (`-x`) go across.
- **Tag on the release branch,** and only from a commit whose checks passed.
- **Support the latest minor only,** so each fix is one backport.
- **A patch is a fix:** no feature, no migration, no API change.
- **Small pull requests,** one fix each, so each cherry-pick is clean.

## Why a change is needed

Today there is one trunk. `.github/workflows/images.yml` refuses a release tag whose commit is not an ancestor of `main` (the step "A tag is on main"). So `v1.0.0` could only be a `main` commit, and everything merged to `main` before it would ship in 1.0.0. A branch for the 1.0 line removes that limit.

## The model

| Branch | Holds | Tags pushed on it | Images |
| --- | --- | --- | --- |
| `main` | the next version (1.1) | `v1.1.0-rc.N`, `v1.1.0` | edge: `:main`, `:sha-<7>` |
| `release/1.0` | fixes for 1.0 only | `v1.0.0-rc.N`, `v1.0.0`, `v1.0.N` | none until a tag |
| `<type>/<name>` | one change | none | none |

- `release/1.0` is cut from the commit tagged `v1.0.0-rc.1`: `git branch release/1.0 v1.0.0-rc.1 && git push origin release/1.0`.
- `release/*` gets the same ruleset as `main`: pull requests, required checks, no force-push.
- A release branch is cut at the first rc of each minor version. `release/1.1` is cut when `v1.1.0-rc.1` is tagged, and then `main` carries 1.2.
- Only the latest minor version is supported (next section).
- **Environments need no change.** `staging` (rc tags) and `production` (the owner approves) are chosen by the tag, so they serve every version. Check that their deployment rules accept tags on `release/*`.

## Supported versions

Only the latest minor version gets fixes. When 1.1.0 ships, 1.0.x is no longer patched and users upgrade to 1.1. The exception is real usage: if many people are found to stay on an older version, the owner may patch it for a stated period, for example a security fix only.

- `release/1.0` is not deleted when 1.1.0 ships; it is left unpatched. Its tags keep the history and the images.
- A fix after that goes to `main` first, then to the newest `release/*` branch only.
- The statement goes in `SECURITY.md`, which has none today.

## Rules for working in parallel

1. **A fix goes to `main` first, then to `release/1.0` with `git cherry-pick -x`.** Then 1.1 never lacks a 1.0 fix. A feature goes to `main` only. If the code has drifted so the cherry-pick conflicts, or the bug is easier to reproduce on 1.0, fix on `release/1.0` first and cherry-pick onto `main` (case 2 in the worked example). The fix must end up on both lines either way.
2. **No database migration in a patch.** A 1.0.x migration would take the same number as a 1.1 one, and migrations are only ever appended. Solve the problem without one, or it is not a patch.
3. **The API stays additive** (an existing rule), so 1.1 on `main` does not break a 1.0 client.
4. **Versions.** `release/1.0` carries 1.0.x: a `chore: version 1.0.N` commit before each tag. `main` becomes 1.1.0 in the pull request that cuts the first 1.1 rc.
5. **Keep the stacks apart.** Run the rc on the staging stack ([the guide](guide.md#staging-a-release): port 8443, `./data-staging`, project `askbar-staging`). Run 1.1 development from a `main` checkout with its own `DATA_DIR` and `-p` project name. Never point a `main` build at the rc's data.
6. **Notes and changelog come first.** A release tag needs `docs/releases/<tag>.md` and `docs/releases/changelog/CHANGELOG-<tag>.md`, or the release job fails. Write them in a pull request into the branch the tag is on, then copy them to `main`.

Work on both lines at once. Pausing features until 1.0.x is finished is worth it only if the 1.0 bug list is long.

## Example timeline

- **Day 0.** Tag `v1.0.0-rc.1` on a `main` commit. Create `release/1.0` from it. A 1.1 feature (pull request #120) merges to `main`.
- **Day 3, a bug.** The fix (#121, commit `F`) merges to `main`. Backport it: `git switch -c fix/backport-121 origin/release/1.0 && git cherry-pick -x F`, and open a pull request into `release/1.0`. After it merges, tag `v1.0.0-rc.2` on `release/1.0`. rc.2 has no 1.1 feature.
- **Day 7, no more bugs.** A pull request into `release/1.0` adds the 1.0.0 notes and changelog. Tag `v1.0.0` on the branch tip, approve `production`, check the draft and publish it. Copy the two files to `main`.
- **Week 9, another bug.** Fix on `main`, cherry-pick to `release/1.0`, version commit, notes and changelog, tag `v1.0.1`.
- **Week 12, 1.1 is ready.** On `main`: bump to 1.1.0, notes, changelog, tag `v1.1.0-rc.1`. Cut `release/1.1`. `main` carries 1.2. After `v1.1.0` ships, 1.0.x gets no more patches.

## `:latest`

A production tag pushes `:X.Y.Z`, `:X.Y`, `:X` and `:latest`, in the order the tags are pushed. With only the latest minor patched, that is right: the newest tag is always the highest version. It would go wrong only if an older line were patched after a newer minor shipped (`v1.0.1`, `v1.1.0`, `v1.0.2` would leave `:latest` and `:1` on 1.0.2). If the owner ever patches an older version, change the workflow first to move `:latest` and `:X` only for the highest stable version (`git tag --sort=-v:refname`).

## Changes to make

One pull request, with the `.github/` diff shown to the owner first.

1. `images.yml`: the tag check passes if the commit is an ancestor of `main` or of an `origin/release/*` branch (fetch both, and fail with a message if neither). Edge stays `main` only.
2. `ci.yml` and `codeql.yml`: run for pull requests and pushes on `release/*` as well as `main`.
3. [ADR 0018](adr/0018-releases-and-migrations.md): a dated amendment for the release-branch rule. It is a decision change, so the status stays and the owner keeps or drops it.
4. [The guide](guide.md#release-channels), [CONTRIBUTING.md](../CONTRIBUTING.md) and [SECURITY.md](../SECURITY.md): the branch flow, the rules above and the supported-versions statement.
5. The owner's checklist in `docs/releases/checklist/`: the `main` ruleset also for `release/*`, and the environment rule check.
6. If the tag check moves into a script, a test for it in `tests/`, added to `test:services`.

## Open point

Dependabot targets `main` only, so a security fix for 1.0.x needs a manual backport.

## Trying it with `v1.0.0-rc.2`

Before relying on the setup for `v1.0.0`, run it once with a harmless fix. The owner does these steps, in the web UI or a terminal, after the pull request above merges and `v1.0.0-rc.1` is under test.

1. **Create the branch.** `git fetch origin && git branch release/1.0 v1.0.0-rc.1 && git push origin release/1.0`. In Settings, Rules, apply the `main` ruleset to `release/*`.
2. **Make a fix-only commit**, for example a one-line comment fix. `git switch -c fix/rc2-check origin/main`, commit, open a pull request into `main` and merge it.
3. **Backport it.** `git switch -c fix/backport-rc2-check origin/release/1.0 && git cherry-pick -x <the commit>`, push, and open a pull request with base `release/1.0`.
4. **Check CI.** Actions, `ci`, must run on that pull request and pass. If it does not start, the branch filter in `ci.yml` is wrong.
5. **Merge, then tag.** `git fetch origin && git tag v1.0.0-rc.2 origin/release/1.0 && git push origin v1.0.0-rc.2`.
6. **Watch the `images` run for the tag.** `plan` and "A tag is on main" must pass (the commit is on `release/1.0`, not on `main`), then `publish`, `installers` and `release`.
7. **Check the draft** under Releases: a prerelease holding `compose.release.yaml`, the installers, the SBOM tar, `SHA256SUMS` and `SHA256SUMS.sigstore.json`, with links that open at the tag.
8. **Stage it** as [the guide](guide.md#staging-a-release) says, and check that a feature merged to `main` after the branch was cut is not in it.
9. **If a step fails**, the workflow change is wrong. Fix it in a pull request to `main`, backport to `release/1.0`, and tag `rc.3`. Do not delete tags to retry; use the next rc number.

## Worked example: a patch and a feature at the same time

Starting point: `v1.0.0` is released from `release/1.0`. `main` is on its way to 1.1. Two things are in flight: bug #130 ("the backup page shows the wrong date", a patch) and feature #131 ("export a conversation", for 1.1). Optionally keep one checkout for each line:

```bash
git clone <repo> askbar && cd askbar          # main: the 1.1 line
git worktree add ../askbar-1.0 release/1.0     # a second folder on release/1.0
```

### A. The patch, released as 1.0.1

1. **Fix on `main`.**
   ```bash
   git switch -c fix/backup-date origin/main
   # edit, add a test, then:
   git commit -m "fix(admin): show the backup date in the right zone"
   git push -u origin fix/backup-date           # open a PR into main, merge it
   ```
2. **Backport.** Copy the squashed commit's SHA from `main`'s history.
   ```bash
   cd ../askbar-1.0
   git fetch origin
   git switch -c fix/backup-date-1.0 origin/release/1.0
   git cherry-pick -x <sha>                     # resolve a conflict if there is one, then git cherry-pick --continue
   git push -u origin fix/backup-date-1.0       # open a PR with base release/1.0, wait for ci, merge
   ```
**Case 2: the code changed in 1.1.** If the 1.1 PRs already merged touch the code you are fixing, the backport in steps 1 and 2 conflicts, or the bug is easier to reproduce on 1.0. Fix on the release branch first, then copy the fix onto `main`, in the other order:

```bash
git fetch origin
git switch -c fix/backup-date-1.0 origin/release/1.0
# edit, add a test, commit, push, open a PR with base release/1.0, wait for ci, merge
git switch -c fix/backup-date origin/main
git cherry-pick -x <sha of the squashed commit on release/1.0>   # resolve conflicts here
git push -u origin fix/backup-date          # open a PR into main, merge
```

The fix is then written and tested against the code it patches, and the port to `main` is where the conflicts are resolved. Either way, continue with step 3, and do not skip the second PR: without it 1.1 brings the bug back.

3. **Prepare the release** in one more PR into `release/1.0`:
   - a `chore: version 1.0.1` commit (the same version files the 1.0.0 version commit changed);
   - `docs/releases/v1.0.1.md` and `docs/releases/changelog/CHANGELOG-v1.0.1.md` (a `## [1.0.1]` section with `### Fixed`).
4. **Stage it (optional for a one-line fix).** Tag `v1.0.1-rc.1` on the branch tip, run the staging stack, then continue. A rc needs its own notes file `docs/releases/v1.0.1-rc.1.md`.
5. **Tag and release.**
   ```bash
   git fetch origin
   git tag v1.0.1 origin/release/1.0
   git push origin v1.0.1                       # the images run starts; approve `production`
   ```
   Check the draft under Releases, then publish it by hand.
6. **Carry the notes to `main`.** Copy `v1.0.1.md` and `CHANGELOG-v1.0.1.md` into a PR into `main`, so `main` has every release's notes.

### B. The feature, released as 1.1.0

1. **Build it on `main`**, in the same folder as step A1, in parallel with the patch:
   ```bash
   git switch -c feat/export-conversation origin/main
   git push -u origin feat/export-conversation   # open a PR into main, merge it when green
   ```
   Merge other 1.1 features the same way. None of them is on `release/1.0`, so 1.0.1 does not contain them.
2. **When 1.1 is ready, bump the version on `main`** in a PR: `chore: version 1.1.0`, `docs/releases/v1.1.0-rc.1.md`, `docs/releases/changelog/CHANGELOG-v1.1.0-rc.1.md`.
3. **Tag the rc and cut the branch.**
   ```bash
   git fetch origin
   git tag v1.1.0-rc.1 origin/main
   git push origin v1.1.0-rc.1                  # staging: no approval
   git branch release/1.1 v1.1.0-rc.1
   git push origin release/1.1                  # give it the same ruleset as main
   ```
   `main` now carries 1.2.
4. **Test and fix.** A bug in the rc follows part A with `release/1.1` in place of `release/1.0`. Tag `v1.1.0-rc.2` on `release/1.1`.
5. **Release.** A PR into `release/1.1` adds `docs/releases/v1.1.0.md` and its changelog. Then:
   ```bash
   git tag v1.1.0 origin/release/1.1 && git push origin v1.1.0
   ```
   Approve `production`, check the draft, publish. Copy the notes to `main`.
6. **After 1.1.0 ships**, 1.0.x gets no more fixes; new fixes go to `main` and `release/1.1`.

### Checks at each step

- A PR into either branch needs `ci` to pass before merging.
- A tag run fails if the notes file is missing or a link in it is wrong; fix it on the branch and use the next rc number, never move a tag.
- A feature that appears in a `release/*` build is a mistake: it was merged into the wrong branch. Revert it there and open it against `main`.
