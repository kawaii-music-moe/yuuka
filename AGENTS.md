# Codex repository rules

## Build safety (mandatory)

The build host has limited memory and can crash when builds run concurrently.
These rules apply to Codex and any subagents working in this repository.

- Run build or compilation-producing commands only when the current Git branch is
  exactly `develop`. Immediately before running one, verify the branch with
  `git branch --show-current`. If it is not `develop` (including detached HEAD),
  do not run the command; report that verification was skipped because of this
  rule. Do not switch branches merely to bypass the restriction.
- Never run more than one build or compilation-producing command at a time. Do
  not start one in the background, delegate one to a subagent, or launch build
  commands in parallel tool calls.
- Serialize builds across Codex sessions by holding the shared lock
  `/tmp/yuuka-codex-build.lock` for the entire command. Use a non-blocking lock;
  if the lock is already held, skip the build and report that another build is
  active. For example:

  ```bash
  flock -n /tmp/yuuka-codex-build.lock -c '<build command>'
  ```

- Treat any command that may compile code as a build for these rules, including
  `cargo build`, `cargo check`, `cargo test`, `cargo clippy`, frontend/package
  build scripts, Docker image builds, and compilation invoked indirectly by
  `xtask` or another script.
- For Rust compilation, also limit compiler parallelism to one job, for example
  with `CARGO_BUILD_JOBS=1` or `--jobs 1`.
