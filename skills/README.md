# Orbit skills for coding agents

Each directory here is an agent skill: a `SKILL.md` with a name and a
description in its front matter, then the instructions an agent follows when
the description matches what it is asked. Claude Code, Codex, Cursor and
agents that read `~/.agents/skills` all load this format.

| Skill | For |
|---|---|
| [`orbit`](orbit/SKILL.md) | driving the profiler: run or build `orbit-service`, capture over HTTP, read sampling reports, hook functions, capture files, settings, the viewer, the e2e suite |
| [`orbit-timeline`](orbit-timeline/SKILL.md) | putting an agent's, a script's or a work queue's own activity on the timeline: `orbit-scope`, `POST /api/scope`, `POST /api/events`, the benchmark producer |
| [`orbit-instrument`](orbit-instrument/SKILL.md) | instrumenting a program: `orbit.h`, the `orbit-api` crate and Python package, the pip wheels, Unreal Engine |

## Install

```sh
tools/install/install_skills.sh            # every agent: ~/.agents, ~/.claude, ~/.cursor, ~/.codex
tools/install/install_skills.sh claude     # one or more of: agents claude cursor codex
tools/install/install_skills.sh --dir /path/to/skills
```

The script copies each `SKILL.md` to `<dir>/<skill>/SKILL.md` and overwrites
what is there, so rerunning it after a pull updates the skills. A project can
also point its agent at this directory directly (Claude Code reads
`.claude/skills/`; a symlink to `skills/` works).

## Keeping them true

The skills quote paths, JSON fields, pill labels and query parameters in the
exact spelling the software uses. When a feature changes, change the skill
in the same pull request, the way `docs/manual/features.md` is kept: that
catalogue is the long form, the skills are what an agent reads before acting.
