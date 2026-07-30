# Linked-worktree development standard

Use Git linked worktrees to isolate every mutable ai-gateway task. The primary
checkout is the stable coordination checkout; task work happens in dedicated
worktrees.

## When a worktree is required

Create or reuse a dedicated worktree before modifying tracked files.

Exceptions:

- read-only investigation or review
- GitHub settings or other remote-only operations that do not change files
- continuing the same task in its existing, verified worktree
- an explicit user instruction to use another safe layout

Do not develop directly in the primary checkout. Keep its `main` branch clean
so it can be synchronized, used as the worktree administration point, and
compared with `origin/main`.

## Ownership and isolation

- One task owns one branch and one worktree.
- One branch may be checked out in only one worktree.
- Concurrent agents or tasks must use different branch names and paths.
- Before using, removing, or repairing a worktree, verify its branch, status,
  and owner. Never assume an unfamiliar worktree is abandoned.
- Git refs, remotes, stashes, hooks, and repository-level configuration are
  shared. Avoid `git stash` as cross-worktree ownership is ambiguous.
- Never run repository-wide destructive commands such as `git reset --hard`,
  `git clean -fdx`, branch deletion, or forced worktree removal against work
  that may belong to another task.

## Canonical location and names

Task worktrees live under the ignored `.worktrees/` directory in the primary
checkout:

```text
ai_gateway/
├── .git/
├── .worktrees/
│   └── fix-routing-timeout/
└── ...
```

Use the normal branch prefixes from the workflow skill. Derive the directory
name by replacing `/` in the branch name with `-`:

```text
branch:   fix/routing-timeout
worktree: .worktrees/fix-routing-timeout
```

Use a short, task-specific slug. Do not reuse a path until the previous
worktree has been intentionally removed.

## Create a worktree for a new task

Start in the primary checkout:

```bash
git status --short
git branch --show-current
git worktree list --porcelain
git fetch origin --prune
git switch main
git pull --ff-only origin main
test "$(git branch --show-current)" = "main"
test -z "$(git status --porcelain)"

branch="fix/routing-timeout"
worktree=".worktrees/${branch//\//-}"

test ! -e "$worktree"
git worktree add -b "$branch" "$worktree" main
git -C "$worktree" status --short
git -C "$worktree" branch --show-current
```

Stop and inspect if:

- the primary checkout contains changes
- local `main` and `origin/main` diverge
- the branch or target path already exists
- `git worktree list` shows unexpected or locked entries

Do not stash, reset, delete, or overwrite existing work to make the command
succeed. Do not use `-B` or `--force`.

## Reuse an existing branch

First determine whether the branch is already attached:

```bash
git worktree list --porcelain
git branch --list "<branch>"
git branch -r --list "origin/<branch>"
```

Attach an existing local branch only when it is not checked out elsewhere:

```bash
git worktree add ".worktrees/<branch-slug>" "<branch>"
```

For a remote-only branch:

```bash
git fetch origin --prune
git worktree add \
  --track \
  -b "<branch>" \
  ".worktrees/<branch-slug>" \
  "origin/<branch>"
```

If the branch belongs to another user or agent, obtain confirmation before
continuing it.

## Work inside the task worktree

Use the task worktree as the working directory for all file operations:

```bash
cd ".worktrees/<branch-slug>"
git rev-parse --show-toplevel
git status --short
```

- Read, edit, generate, format, build, test, commit, push, and open the PR from
  this directory.
- Give automation tools the task worktree path, not the primary checkout.
- Check `git status --short` before and after generators or broad formatters.
- Do not edit the same tracked file through both the primary checkout and the
  task worktree.
- Do not symlink or share `target/`, `node_modules/`, `dist/`, ignored runtime
  configuration, browser state, or secret files between worktrees. Cargo and
  pnpm may use their normal user-level download stores.
- A fresh worktree does not contain ignored configuration or credentials.
  Recreate only the minimum local setup required for the task; never commit or
  print secrets.

### Shared PostgreSQL and local services

The default Compose file exposes fixed ports. Starting it independently from
multiple worktrees can create duplicate projects and port conflicts.

When an existing canonical development PostgreSQL instance is sufficient,
start or inspect it through the primary checkout:

```bash
primary="$(
  git worktree list --porcelain |
    awk '/^worktree / { print substr($0, 10); exit }'
)"

docker compose \
  --project-directory "$primary" \
  -f "$primary/docker-compose.yml" \
  up -d
```

Still run source-dependent commands such as migrations and tests from the task
worktree. Validate changed Compose files from the task worktree. If a task
needs an isolated service stack, use an explicit unique project name and
non-conflicting ports, then remove that stack during cleanup.

Tasks that add or change migrations must use a task-specific database or an
isolated stack; do not apply unreleased migrations to a shared development
database used by another worktree. Likewise, serialize commands that bind the
project's fixed development ports, including Gateway servers, Vite, and
Playwright, unless the task explicitly configures non-conflicting ports.

Do not start the forwarding performance harness or paid real-upstream tests
without the explicit authorization required by `AGENTS.md`.

## Push and pull-request lifecycle

- Push only the task branch.
- Use `git push -u origin "$(git branch --show-current)"` on first push.
- Pass `--repo oai404iao/ai_gateway` to `gh`.
- Keep the worktree until the PR is merged, closed with an explicit retention
  decision, or the user authorizes abandoning the task.
- Fix CI and review findings in the same worktree.
- Do not detach the branch merely because a PR is waiting for review.

## Cleanup after a merged PR

Verify the merge and status before removal:

```bash
primary="/absolute/path/to/ai_gateway"
worktree="$primary/.worktrees/<branch-slug>"
branch="<type>/<short-description>"
pr="<number>"

gh pr view "$pr" \
  --repo oai404iao/ai_gateway \
  --json state,mergedAt,mergeCommit,url

git -C "$worktree" status --short
cd "$primary"
git -C "$primary" fetch origin --prune
git -C "$primary" worktree list --porcelain
```

Stop task-owned servers, browsers, and containers. A normal removal must
succeed without force:

```bash
git -C "$primary" worktree remove "$worktree"
```

Then delete the local branch according to the verified merge method:

```bash
# Squash merge: the topic commits are intentionally not main ancestors.
git -C "$primary" branch -D "$branch"

# Merge commit: prefer the ancestry-safe form.
git -C "$primary" branch -d "$branch"
```

Finally synchronize and verify:

```bash
git -C "$primary" switch main
git -C "$primary" pull --ff-only origin main
git -C "$primary" worktree prune --dry-run
git -C "$primary" status --short
git -C "$primary" rev-list --left-right --count main...origin/main
```

The divergence result must be `0 0`. Run `git worktree prune` only after
reviewing the dry-run output and confirming every listed path was intentionally
removed.

Never use `git worktree remove --force` or `git clean` to hide an unclear
status. Inspect tracked, untracked, and ignored files first. If a PR was closed
without merge or the task was abandoned, preserve or delete its branch only
with explicit user direction.

## Completion report

State:

- primary checkout and task worktree paths
- task branch and final commit
- PR and merge result, when applicable
- whether task-owned services were stopped
- whether the worktree and local/remote branches were removed
- whether primary `main` is clean and matches `origin/main`

Verify each cleanup claim with Git and GitHub before reporting it.
