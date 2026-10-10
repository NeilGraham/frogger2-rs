# Orchestration

- Main session = orchestrator: plans, writes briefs, launches agents, merges branches, runs gates,
  updates `work/tasks.json`, `docs/source-map.md`, `docs/DECISIONS.md`.
- Parallelism: independent crates run concurrently; f2-game waves run concurrently in separate git
  worktrees (branches merged by the orchestrator; conflicts resolved centrally).
- Model tiering (owner instruction 2026-10-09): **Opus** for hard work (gameplay ports, reverse
  engineering, renderer/determinism, parity debugging, architecture); **Sonnet** for routine work
  (tooling, packaging, CLI/UX, docs, small format changes, test scaffolding).
- Every brief: one goal, exact files, executable "done when", budget, report format (`docs/AGENT-BRIEF.md`).

## Memory caps (added 2026-10-09 after a runaway test OOM-crashed WSL)
A WIP test (`hp-script` `vm_media`, HP recomp) grew to 62 GB and exhausted the WSL VM, killing every agent in all four recomp projects at once.
* `.cargo/config.toml` sets `tools/memcap.sh` as the cargo runner: every `cargo test`/`run`/`bench` binary runs in its own cgroup capped at 8 GB (no swap). Over the cap, only that binary is killed (exit 137 / SIGKILL). Do not remove the runner.
* A legitimately big job: `MEMCAP=16G cargo test ...`. Do not use `MEMCAP=off` for anything unbounded.
* Non-cargo heavy work (Python oracles, trace dumps, emulators, anything that loops over all game data) must be wrapped too: `tools/memcap.sh python3 -I tools/... `.
* Interpreters/VMs/fuzzers under test need their own step or allocation budget; an exit-137 test means a missing budget, not a reason to raise the cap.
