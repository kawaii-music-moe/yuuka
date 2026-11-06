# Codex build safety

`hooks.json` registers a synchronous `PreToolUse` hook for `Bash` (including
Codex's `exec_command`). It calls `scripts/guard-build.sh`, the same guard used
by Claude Code, to reject common build commands that bypass the shared runner.
`AGENTS.md` remains at the repository root.

## Enable

Open this repository as a trusted project in a Codex client supporting hooks.
Review and trust the hook definition using `/hooks` in Codex CLI. New or changed
definitions require review again. Untrusted hooks are skipped; creating this
file does not activate them in an already-running session.

Do not bypass hook trust. If the client does not support hooks, follow
`AGENTS.md` and use the shared runner explicitly.

## Build

From the repository root:

```bash
scripts/run-build.sh cargo check
scripts/run-build.sh pnpm build:front
```

The runner verifies `git branch --show-current` immediately before executing
the command, allows only `develop`, holds `/tmp/yuuka-codex-build.lock` until
completion, and exports `CARGO_BUILD_JOBS=1`. A busy lock or another branch
causes a skip with an explanation and exit code 2. The branch check uses the
command's working directory; use an absolute runner path in subdirectories.

Do not run builds in the background or concurrently, or override the job limit
inside a command/script. The hook detects common commands conservatively and
may also reject non-build package commands. It is not a security sandbox and
cannot inspect arbitrary scripts, aliases, or code executed through other tools.

Official documentation: <https://learn.chatgpt.com/docs/hooks>
