---
name: claude-workhorse
description: Delegate bounded, non-destructive project analysis or implementation to the local Claude Code worker, then inspect and verify its result. Use when the user asks to use Claude or when a separable work package would materially benefit from a Claude pass. Do not use for secrets, destructive work, external mutations, or unbounded autonomous changes.
---

# Claude workhorse

Use the repository's `scripts/claude-worker.ts`; Codex remains the orchestrator
and owner of the final result.

1. Inspect `git status` and the relevant diff before delegation. Existing edits
   are user-owned.
2. Choose `analyze` for read-only investigation or review. Choose `implement`
   only when the user's request authorizes code changes.
3. Give Claude one bounded work package with the goal, relevant scope,
   constraints, acceptance criteria, and useful verification commands. Tell the
   user when Claude is being invoked.
4. Invoke one of:

   ```powershell
   bun run claude:worker --mode analyze -- "<bounded task>"
   bun run claude:worker --mode implement -- "<bounded task>"
   ```

   For long or awkward prompts, pipe the task on stdin. The wrapper is pinned to
   `claude-opus-5`; use `--model` only when the user asks for an override. A
   returned session can be continued with `--resume <session-id>` when one
   focused correction is more efficient than a fresh run.
5. Treat Claude's handoff as a claim, not verification. Inspect every changed
   file and the actual diff, check for interference with pre-existing edits,
   and run the proportionate tests yourself.
6. Fix small integration issues locally. Resume Claude at most for a genuinely
   useful bounded correction; avoid open-ended retry loops.

The wrapper uses Claude Code's restricted mode, disables MCP, browser/web tools,
nested agents, git mutations, downloads, process launching, and destructive
shell commands. Never replace it with `--dangerously-skip-permissions`.

Do not run simultaneous Claude implementation workers in the shared working
tree. Use a separate, verified worktree only when concurrency is explicitly
needed and it will not hide user-owned uncommitted state.
