#!/usr/bin/env bash
# Merge a completed Ralph worktree back into main, close its gh issue.
# Usage: merge-ralph-worktree.sh <worktree-path> <branch-name> <gh-issue-number>
set -euo pipefail

WORKTREE=${1:?worktree path}
BRANCH=${2:?branch name}
GHN=${3:?gh issue number}

MAIN=/home/cyber/Work/FF/minibrowser
cd "$MAIN"

if [ "$(git rev-parse --abbrev-ref HEAD)" != "main" ]; then
  echo "ERROR: main checkout is not on main branch" >&2
  exit 1
fi

git fetch --all --quiet || true

# Bail if branch has no new commits vs main
if git merge-base --is-ancestor "$BRANCH" main; then
  echo "Branch $BRANCH already merged or empty. Skipping merge."
else
  echo "Merging $BRANCH into main (--no-ff)..."
  if ! git merge --no-ff --no-edit "$BRANCH"; then
    echo "MERGE CONFLICT. Resolve in $MAIN, then run: git commit && git push && gh issue close $GHN" >&2
    exit 2
  fi
fi

# Verify build
echo "Running cargo check..."
cargo check --all-targets --all-features

echo "Pushing main..."
git push origin main

echo "Closing gh#$GHN..."
gh issue close "$GHN" -c "Resolved via Ralph worktree \`$BRANCH\`. Merged to main."

echo "Removing worktree $WORKTREE..."
git worktree remove "$WORKTREE" --force
git branch -d "$BRANCH" 2>/dev/null || true

echo "DONE: gh#$GHN closed, $BRANCH merged."
