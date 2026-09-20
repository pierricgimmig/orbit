# TEMPLATE ONLY — propose a patch to an external project

**Not a pull request.** Copy this after the Orbit optimize-loop **accepts**
a real change (mean metric gain > 0.5% **and** outside ~1σ, fingerprint
unchanged, correctness green). Fill every `{{placeholder}}`. A human
opens the PR against **that** project. Do not invent numbers from
rejected or no-op runs.

---

## Title

`{{one-line description}}`

## Motivation

`{{Why this path is hot. Cite Orbit/perf symbols from a search/work-dominated profile.}}`

Profile: `{{path to summary.json}}` backend `{{orbit-http | orbit-file | perf}}`.

## Orbit workflow

1. Config: `{{path to toml}}` with `project = {{checkout}}`.
2. `python3 tools/optimize-loop/suite.py --config {{toml}}`
3. Inspect hotspots; `optimize_apply_patch` dry-run; rebuild; re-run with `--baseline`.
4. Gate (`min_gain_percent=0.5`, `sigma=1.0`) **accept**. Log: `docs/optimize-loop/loop-log/{{id}}/`.

Orbit does not vendor the target. The checkout is yours.

## Before / after

Host: `{{CPU, OS, compiler}}`. Same build command on both sides.

| Metric | Baseline | After | Δ |
| --- | --- | --- | --- |
| `{{metric name}}` mean × `{{N}}` | `{{mean}}` ± `{{stdev}}` | `{{mean}}` ± `{{stdev}}` | `{{pct}}` |
| Fingerprint | `{{fp}}` | `{{fp}}` | must match |
| Correctness | `{{pass}}` | `{{pass}}` | must pass |

## Patch summary

`{{what changed and why it is correct}}`

```
{{unified diff}}
```

## Test plan

```sh
{{build.command}}
{{bench.command}}     # repeat ≥ 20
{{correctness.command}}
python3 tools/optimize-loop/suite.py --config {{toml}} --baseline {{old/summary.json}}
```

- [ ] Fingerprint unchanged
- [ ] Correctness green
- [ ] Gate accept logged

## Honesty

Capture was `{{work-dominated | still libc — do not submit}}`.
Numbers are from `{{host}}` only.
