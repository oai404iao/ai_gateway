# Recommended GitHub settings

These are target settings for `oai404iao/ai_gateway`, not assumptions. Re-read
the live settings before changing them.

## Merge methods

Enable:

- Squash merging
- Merge commits
- Auto-merge
- Automatic deletion of head branches

Disable:

- Rebase merging

Keep the default squash commit title aligned with the conventional PR title.
Use merge commits only for intentionally structured commit series as defined
in the skill.

After explicit user approval, these merge settings can be applied with:

```bash
gh repo edit oai404iao/ai_gateway \
  --enable-squash-merge \
  --enable-merge-commit \
  --enable-rebase-merge=false \
  --enable-auto-merge \
  --delete-branch-on-merge \
  --squash-merge-commit-message pr-title-description
```

Do not require linear history while merge commits remain an approved
exception.

## Default-branch ruleset

Target `~DEFAULT_BRANCH` and set enforcement to active.

Recommended rules:

- Restrict deletions
- Block non-fast-forward updates and force pushes
- Require a pull request before merging
- Require all review conversations to be resolved
- Require one approval when another maintainer is available
- Allow a documented owner bypass only for repository recovery, not routine
  development

For a solo-maintainer period, approval count may remain zero while PRs and
checks are still required. Security, authentication, billing, persistence,
release, and forwarding changes should receive another human review whenever
possible.

## Required check

The path-aware CI workflow runs for every PR:

1. `changes` detects the affected areas and validates patch whitespace.
2. The reusable quality workflow runs or skips docs, Rust, MSRV, Console, and
   Playwright jobs.
3. The image job runs in parallel when production artifacts are affected.
4. The always-present `ci-gate` succeeds only when every selected job succeeds.

CI and Security explicitly handle only Pull Request `opened`, `synchronize`,
and `reopened` activity. Do not run the path planner for `closed`: after a
squash merge and branch cleanup, the event payload may still identify a head
commit that is no longer fetchable. Post-merge authority comes from the
merged SHA's `main` push workflows.

Require only the exact `ci-gate` context in the default-branch ruleset.
Markdown-only PRs run `scripts/check-docs.py` and therefore satisfy the same
stable check without running unrelated build jobs.

## Suggested verification commands

```bash
gh api repos/oai404iao/ai_gateway \
  --jq '{allow_squash_merge,allow_merge_commit,allow_rebase_merge,allow_auto_merge,delete_branch_on_merge}'

gh api repos/oai404iao/ai_gateway/rulesets \
  --jq 'map({id,name,target,enforcement})'

gh pr checks <number> \
  --repo oai404iao/ai_gateway
```

Repository settings are an external side effect. Change them only with
explicit user approval and report the before/after values.
