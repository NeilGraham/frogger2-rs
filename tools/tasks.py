#!/usr/bin/env python3
"""Task graph for the Frogger 2 Rust port (copied from theme-park-world-recomp): the durable, resumable state of the project.

State lives in work/tasks.json (committed). Every orchestrator session reads it,
works the ready frontier, and checkpoints it back. Stdlib only.

  python3 tools/tasks.py status                  # counts + ready + in-progress
  python3 tools/tasks.py ready                   # tasks whose deps are all done
  python3 tools/tasks.py show A2                 # full task record
  python3 tools/tasks.py claim A2 --by wf:run-label
  python3 tools/tasks.py done A2 --note "proxy loads; log at work/runs/..." [--artifact path ...]
  python3 tools/tasks.py block A2 --note "needs owner: ..."
  python3 tools/tasks.py reset A2 --note "agent died, retry"
  python3 tools/tasks.py recover                 # session start: list stale in_progress tasks
  python3 tools/tasks.py add W1-003 --title ... --stage B --deps A9,A11 [--files crates/x] [--accept ...]
  python3 tools/tasks.py graph                   # mermaid dependency graph
  python3 tools/tasks.py validate                # cycles, unknown deps
"""
import argparse, json, os, sys, datetime

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PATH = os.path.join(ROOT, "work", "tasks.json")
STATUSES = ("todo", "in_progress", "done", "blocked")


def now():
    return datetime.datetime.now().astimezone().isoformat(timespec="seconds")


def load():
    with open(PATH) as f:
        return json.load(f)


def save(db):
    tmp = PATH + ".tmp"
    with open(tmp, "w") as f:
        json.dump(db, f, indent=2)
        f.write("\n")
    os.replace(tmp, PATH)


def index(db):
    return {t["id"]: t for t in db["tasks"]}


def is_ready(t, idx):
    return t["status"] == "todo" and all(idx[d]["status"] == "done" for d in t.get("deps", []))


def log(t, msg):
    t.setdefault("history", []).append(f"{now()} {msg}")


def get(idx, tid):
    if tid not in idx:
        sys.exit(f"unknown task {tid}")
    return idx[tid]


def cmd_status(db, a):
    idx = index(db)
    counts = {s: sum(1 for t in db["tasks"] if t["status"] == s) for s in STATUSES}
    print("counts:", counts)
    for label, sel in (("IN PROGRESS", lambda t: t["status"] == "in_progress"),
                       ("BLOCKED", lambda t: t["status"] == "blocked"),
                       ("READY", lambda t: is_ready(t, idx))):
        rows = [t for t in db["tasks"] if sel(t)]
        if rows:
            print(f"\n{label}:")
            for t in rows:
                extra = f"  [by {t.get('owner')}]" if t["status"] == "in_progress" else ""
                print(f"  {t['id']:8} {t['stage']}  {t['title']}{extra}")


def cmd_ready(db, a):
    idx = index(db)
    for t in db["tasks"]:
        if is_ready(t, idx):
            print(f"{t['id']}\t{t['stage']}\t{'owner' if t.get('needs_owner') else 'agent'}\t{t['title']}")


def cmd_show(db, a):
    print(json.dumps(get(index(db), a.id), indent=2))


def cmd_claim(db, a):
    idx = index(db)
    t = get(idx, a.id)
    if not is_ready(t, idx) and not a.force:
        sys.exit(f"{a.id} is not ready (status={t['status']}, deps={t.get('deps')})")
    t["status"], t["owner"] = "in_progress", a.by
    log(t, f"claimed by {a.by}")
    save(db)


def cmd_done(db, a):
    t = get(index(db), a.id)
    t["status"] = "done"
    if a.artifact:
        t.setdefault("artifacts", []).extend(a.artifact)
    log(t, f"done: {a.note or ''}")
    save(db)


def cmd_block(db, a):
    t = get(index(db), a.id)
    t["status"] = "blocked"
    log(t, f"blocked: {a.note}")
    save(db)


def cmd_reset(db, a):
    t = get(index(db), a.id)
    t["status"], t["owner"] = "todo", None
    log(t, f"reset: {a.note or ''}")
    save(db)


def cmd_recover(db, a):
    stale = [t for t in db["tasks"] if t["status"] == "in_progress"]
    if not stale:
        print("no in-progress tasks; nothing to recover")
    for t in stale:
        print(f"{t['id']}  owner={t.get('owner')}  last={t.get('history', ['?'])[-1]}")
    if stale:
        print("\nFor each: check its branch/worktree (git branch --list 'task/*'). "
              "If its acceptance criteria are met, merge and `done`; otherwise `reset`.")


def cmd_add(db, a):
    idx = index(db)
    if a.id in idx:
        sys.exit(f"{a.id} exists")
    deps = [d for d in (a.deps or "").split(",") if d]
    for d in deps:
        get(idx, d)
    t = {"id": a.id, "title": a.title, "stage": a.stage, "deps": deps, "status": "todo",
         "owner": None, "needs_owner": a.needs_owner, "files": a.files or [],
         "accept": a.accept or [], "notes": a.notes or "", "artifacts": [], "history": []}
    log(t, "added")
    db["tasks"].append(t)
    save(db)


def cmd_graph(db, a):
    print("graph TD")
    for t in db["tasks"]:
        print(f'  {t["id"].replace("-", "_")}["{t["id"]}: {t["title"][:40]} ({t["status"]})"]')
        for d in t.get("deps", []):
            print(f'  {d.replace("-", "_")} --> {t["id"].replace("-", "_")}')


def cmd_validate(db, a):
    idx, bad = index(db), 0
    for t in db["tasks"]:
        for d in t.get("deps", []):
            if d not in idx:
                print(f"{t['id']}: unknown dep {d}"); bad += 1
        if t["status"] not in STATUSES:
            print(f"{t['id']}: bad status {t['status']}"); bad += 1
    state = {}
    def visit(n, stack):
        if state.get(n) == 1:
            print("cycle:", " -> ".join(stack + [n])); return 1
        if state.get(n) == 2:
            return 0
        state[n] = 1
        r = sum(visit(d, stack + [n]) for d in idx[n].get("deps", []) if d in idx)
        state[n] = 2
        return r
    bad += sum(visit(n, []) for n in idx)
    print("ok" if not bad else f"{bad} problem(s)")
    sys.exit(1 if bad else 0)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    s = p.add_subparsers(dest="cmd", required=True)
    s.add_parser("status"); s.add_parser("ready"); s.add_parser("recover")
    s.add_parser("graph"); s.add_parser("validate")
    x = s.add_parser("show"); x.add_argument("id")
    x = s.add_parser("claim"); x.add_argument("id"); x.add_argument("--by", required=True); x.add_argument("--force", action="store_true")
    x = s.add_parser("done"); x.add_argument("id"); x.add_argument("--note"); x.add_argument("--artifact", action="append")
    x = s.add_parser("block"); x.add_argument("id"); x.add_argument("--note", required=True)
    x = s.add_parser("reset"); x.add_argument("id"); x.add_argument("--note")
    x = s.add_parser("add"); x.add_argument("id"); x.add_argument("--title", required=True)
    x.add_argument("--stage", required=True); x.add_argument("--deps"); x.add_argument("--files", nargs="*")
    x.add_argument("--accept", nargs="*"); x.add_argument("--notes"); x.add_argument("--needs-owner", action="store_true")
    a = p.parse_args()
    db = load()
    globals()["cmd_" + a.cmd](db, a)


if __name__ == "__main__":
    main()
