# Orchestration

- Main session = orchestrator: plans, writes briefs, launches agents, merges branches, runs gates,
  updates `work/tasks.json`, `docs/source-map.md`, `docs/DECISIONS.md`.
- Parallelism: independent crates run concurrently; f2-game waves run concurrently in separate git
  worktrees (branches merged by the orchestrator; conflicts resolved centrally).
- Model tiering (owner instruction 2026-10-09): **Opus** for hard work (gameplay ports, reverse
  engineering, renderer/determinism, parity debugging, architecture); **Sonnet** for routine work
  (tooling, packaging, CLI/UX, docs, small format changes, test scaffolding).
- Every brief: one goal, exact files, executable "done when", budget, report format (`docs/AGENT-BRIEF.md`).
