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

## Required-check caveat

The current CI workflow uses:

```yaml
pull_request:
  paths-ignore:
    - "**/*.md"
```

Therefore Markdown-only PRs do not create the `rust`, `console`, or `image`
checks. Making those names unconditional required checks can leave
documentation PRs blocked waiting for checks that never start.

Before requiring status checks, change CI so every PR emits one stable final
check, for example `ci-gate`:

1. Trigger the workflow for every PR.
2. Detect which areas changed.
3. Run or skip the heavy Rust, Console, and image jobs as appropriate.
4. Run an always-present final gate that succeeds only when every required
   selected job succeeded.
5. Require only that stable gate in the ruleset.

Until that gate exists, do not configure unconditional required checks that
are absent on Markdown-only changes. Agents must still run the verification
matrix locally and wait for every check that does appear.

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
