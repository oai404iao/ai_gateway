---
name: ai-gateway-development-workflow
description: Project-specific Git and GitHub workflow for ai-gateway. Use when starting or finishing repository changes, creating branches or commits, opening or reviewing pull requests, choosing a merge method, handling CI failures, cleaning up merged branches, preparing hotfixes or releases, or configuring protection for main.
compatibility: Requires git. GitHub operations require an authenticated gh CLI. Run commands from the ai-gateway repository root.
---

# ai-gateway development workflow

Use a pull-request-first workflow that protects `main`, runs the checks that
match the change, and preserves intentional history without using GitHub's
rebase-and-merge mode.

## Sources of truth

1. Read the repository `AGENTS.md` before making changes.
2. Read [verification-matrix.md](references/verification-matrix.md) before
   declaring a branch or pull request ready.
3. Read [github-settings.md](references/github-settings.md) when changing
   repository settings or explaining the merge policy.
4. For releases, also read `docs/development/releasing.md`.

Repository instructions override this skill if they become more specific.

## Merge policy

| Pull request type | Merge method | Reason |
|---|---|---|
| Normal feature, fix, refactor, docs, CI, dependency, or release-preparation PR | **Squash and merge** | Keeps `main` to one logical commit per PR and makes rollback straightforward. |
| Deliberately structured, independently valid commit series whose exact commits are useful after merge | **Create a merge commit** | Preserves the reviewed commit SHAs, ordering, and PR boundary. Typical examples are staged migrations or a large mechanical refactor split by component. |
| Any PR | **Do not use GitHub Rebase and merge** | GitHub recreates the commits on the base branch, changing their SHAs and dropping the explicit PR merge boundary. This makes local cleanup and audit trails needlessly confusing. |

A local rebase of an unshared branch before review is different from GitHub's
merge method and is allowed. Once review has started or another developer has
based work on the branch, do not rewrite it without coordination.

If the user explicitly chooses a different allowed method, follow that choice
and state the consequences before merging.

## End-to-end workflow

### 1. Inspect before changing anything

```bash
git status --short
git branch --show-current
git remote -v
git fetch origin --prune
```

- Start only from an understood worktree. Never discard, reset, stash, or
  overwrite changes that may belong to the user or another agent.
- If local `main` and `origin/main` diverge, stop and inspect:

  ```bash
  git rev-list --left-right --count main...origin/main
  git log --oneline --left-right --cherry-pick main...origin/main
  git cherry origin/main main
  ```

  Do not hard-reset until every local-only change is proven duplicated
  upstream or safely backed up.
- Never run the paid real-upstream smoke test or the forwarding performance
  harness without the explicit authorization required by `AGENTS.md`.

### 2. Synchronize `main` and create a branch

```bash
git switch main
git pull --ff-only origin main
git switch -c <type>/<short-description>
```

Use one of:

- `feat/` — user-visible capability
- `fix/` — bug or regression
- `refactor/` — behavior-preserving restructuring
- `docs/` — documentation or agent instructions only
- `test/` — test-only work
- `ci/` — automation
- `chore/` — maintenance or dependencies
- `release/` — version and changelog preparation
- `hotfix/` — urgent production correction

Do not develop directly on `main`.

### 3. Implement in reviewable slices

- Verify the current implementation before relying on plans or the PRD.
- Keep each commit focused on one logical concern.
- Update tests, configuration examples, generated artifacts, notices, and
  documentation required by `AGENTS.md` in the same branch.
- Do not mix drive-by formatting or unrelated cleanup into the PR.
- Do not commit ignored configuration, credentials, JWT keys, database
  passwords, `.env.real-upstream`, browser auth state, or generated secrets.

### 4. Run the matching verification

Use [verification-matrix.md](references/verification-matrix.md).

At minimum:

```bash
git diff --check
git status --short
```

Run the narrowest relevant tests while iterating, then the full required gate
for every affected area before requesting review.

Documentation-only PRs currently do not start the repository CI workflow
because `.github/workflows/ci.yml` ignores `**/*.md`. For those PRs, explicitly
record local validation instead of claiming that absent CI checks passed.

### 5. Commit

Use concise Conventional Commit-style subjects:

```text
feat(console): add ...
fix(routing): prevent ...
refactor(console): migrate ...
docs: define development workflow
ci: pin ...
chore(deps): update ...
```

Rules:

- Normal squash PRs may contain iterative commits, but avoid meaningless
  `wip`, `fix`, or `oops` commits when practical.
- A PR intended for **Create a merge commit** must contain only deliberate,
  ordered, reviewable commits. Each commit should build or clearly document
  why it is an inseparable step in the series.
- Never amend or force-push a branch after review begins without telling the
  reviewer. If rewriting an unshared branch is necessary, use
  `git push --force-with-lease`, never plain `--force`.

### 6. Push and open a pull request

Pushing and creating a PR are external actions. Do them only when the user
requested them or the task explicitly includes publication.

```bash
branch="$(git branch --show-current)"
git push -u origin "$branch"
```

This repository uses an SSH host alias, so pass the GitHub repository
explicitly to `gh`:

```bash
gh pr create \
  --repo oai404iao/ai_gateway \
  --base main \
  --head "$branch" \
  --title "<conventional PR title>" \
  --body-file /tmp/ai-gateway-pr-body.md
```

Use this body structure:

```markdown
## Summary
- What changed and why

## Validation
- Exact commands and results

## Risk and manual QA
- Behavior changes, rollout concerns, or `None`

## Follow-ups
- Deferred work or `None`
```

Open a draft PR when required tests, paid smoke authorization, migration
coordination, or manual QA is still outstanding.

### 7. Review and CI

```bash
gh pr view <number> --repo oai404iao/ai_gateway
gh pr checks <number> --repo oai404iao/ai_gateway --watch --interval 10
```

- Do not merge with pending or failing checks.
- Fix failures on the PR branch and push normally.
- Re-run local checks when a fix changes the affected area.
- Resolve review conversations and update the PR description when scope or
  risk changes.
- A solo maintainer may self-merge only after reading the final diff and
  confirming the verification matrix. Use another reviewer when one is
  available for security, auth, billing, persistence, release, or forwarding
  changes.

### 8. Merge only with explicit authorization

The default is to stop after the PR is ready. Merge only when the user
explicitly asks for it.

Normal PR:

```bash
gh pr merge <number> \
  --repo oai404iao/ai_gateway \
  --squash \
  --delete-branch
```

Intentional commit series:

```bash
gh pr merge <number> \
  --repo oai404iao/ai_gateway \
  --merge \
  --delete-branch
```

Never pass `--rebase`.

After merging, verify the server-side result:

```bash
gh pr view <number> \
  --repo oai404iao/ai_gateway \
  --json state,mergedAt,mergeCommit,url
```

For code, configuration, dependency, CI, Docker, or release-related changes,
also watch the new `main` push workflow and report its final result.

```bash
run_id="$(
  gh run list \
    --repo oai404iao/ai_gateway \
    --branch main \
    --event push \
    --limit 1 \
    --json databaseId \
    --jq '.[0].databaseId'
)"
gh run watch "$run_id" \
  --repo oai404iao/ai_gateway \
  --exit-status
```

### 9. Synchronize and clean up locally

```bash
git fetch origin --prune
git switch main
git pull --ff-only origin main
git status --short
```

- Delete the local topic branch only after GitHub reports the PR as merged.
- After a squash merge, the topic commits are not ancestors of `main`, so
  `git branch -d` may refuse. Verify the merged PR first, then use
  `git branch -D <branch>` if needed.
- After a merge commit, prefer `git branch -d <branch>`.
- Confirm `git rev-list --left-right --count main...origin/main` prints
  `0 0`.

## Release workflow

1. Create `release/<version>` from current `main`.
2. Update every version source and the dated changelog entry listed in
   `docs/development/releasing.md`.
3. Run:

   ```bash
   ./scripts/check-release-version.sh <version>
   ./scripts/verify-release.sh <version>
   ```

4. Open a release-preparation PR and use **Squash and merge**.
5. Synchronize local `main`, confirm a clean worktree, and run:

   ```bash
   ./scripts/release.sh <version> --push
   ```

6. Watch the tag-triggered Release workflow through completion.

Never move, overwrite, or reuse a published tag. Fix release problems with a
new patch version.

## Completion report

Report:

- branch and final commit
- PR number and state, if created
- merge method and resulting `main` commit, if merged
- checks run and their results
- known warnings, skipped checks, or required manual validation
- whether local `main` matches `origin/main`
- whether topic branches were deleted

Do not describe a PR as merged, a check as passed, or a branch as deleted
without verifying it.
