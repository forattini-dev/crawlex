#!/usr/bin/env bash
# Orchestrator: poll all active Ralph worktree outputs and merge each one
# back to main as soon as it emits NO MORE TASKS.
#
# Reads a table of (task_id, branch, gh_issue, worktree_path) from stdin
# (one per line, space-separated) and loops until all are merged.
set -euo pipefail

TASK_DIR=/tmp/claude-1000/-home-cyber-Work-FF-minibrowser/be8f907e-edab-449b-86d2-c898361ac8c7/tasks
HELPER=/home/cyber/Work/FF/minibrowser/scripts/merge-ralph-worktree.sh

declare -A PENDING
while read -r tid branch ghn wt; do
  [ -z "$tid" ] && continue
  PENDING[$tid]="$branch|$ghn|$wt"
done

while [ ${#PENDING[@]} -gt 0 ]; do
  for tid in "${!PENDING[@]}"; do
    f="$TASK_DIR/$tid.output"
    [ -f "$f" ] || continue
    if grep -q "NO MORE TASKS\|<promise>NO MORE TASKS" "$f"; then
      IFS='|' read -r branch ghn wt <<<"${PENDING[$tid]}"
      echo "[orch] $tid done. Merging $branch (gh#$ghn) from $wt..."
      if "$HELPER" "$wt" "$branch" "$ghn"; then
        echo "[orch] gh#$ghn closed."
      else
        echo "[orch] merge failed for $branch — manual intervention required" >&2
      fi
      unset 'PENDING[$tid]'
    fi
  done
  [ ${#PENDING[@]} -eq 0 ] && break
  sleep 30
done

echo "[orch] all worktrees merged."
