#!/usr/bin/env python3
"""Quick analysis-phase A/B for a patch applied to the Stockfish tree:
build, save the binary, check the bit-exact fingerprint, run N alternating
pairs against the baseline binary pinned to one P-core reading nps,
cycles:u and instructions:u, print the paired summary, revert the tree.
Not the gate — the gate (experiment_driver.py) comes after, for keepers."""
import json, os, re, shutil, statistics as st, subprocess, sys, time

S = "<scratch>"
SF = os.path.expanduser("~/git/stockfish")
BASE = S + "/opt-notes/run2/stockfish-baseline"
OUT = S + "/opt-notes/iter"
CPU = os.environ.get("CPU", "2")
PAIRS = int(os.environ.get("PAIRS", "6"))
FP_EXPECTED = "af278ac38b7025a4-1659671"


def run(binary):
    p = subprocess.run(["perf", "stat", "-e", "instructions:u,cycles:u", "-x,", "taskset", "-c", CPU, binary, "bench", "128", "1", "13"],
                       capture_output=True, text=True)
    o = p.stdout + p.stderr
    nps = int(re.search(r"Nodes/second\s*:\s*(\d+)", o).group(1))
    ins = int(re.search(r"(\d+),,cpu_core/instructions/u", o).group(1))
    cyc = int(re.search(r"(\d+),,cpu_core/cycles/u", o).group(1))
    return nps, ins, cyc


def main():
    label = sys.argv[1]
    os.makedirs(OUT, exist_ok=True)
    b = subprocess.run(["make", "-C", "src", "-j16", "build", "ARCH=native", "COMP=gcc"], cwd=SF, capture_output=True, text=True)
    if b.returncode != 0:
        print("BUILD FAILED\n" + b.stderr[-1500:]); subprocess.run(["git", "checkout", "--", "src"], cwd=SF); sys.exit(1)
    cand = f"{OUT}/stockfish-{label}"
    shutil.copy(SF + "/src/stockfish", cand)
    subprocess.run(["git", "diff"], cwd=SF, capture_output=True, text=True).stdout and \
        open(f"{OUT}/{label}.patch", "w").write(subprocess.run(["git", "diff"], cwd=SF, capture_output=True, text=True).stdout)
    fp = subprocess.run([S + "/sf-bench.sh"], capture_output=True, text=True).stdout
    fp = re.search(r"ORBIT_FINGERPRINT (\S+)", fp).group(1)
    subprocess.run(["git", "checkout", "--", "src"], cwd=SF, check=True)
    time.sleep(15)  # cool down after the build
    rows = {"base": [], label: []}
    for i in range(PAIRS):
        for k, binary in ((("base", BASE), (label, cand)) if i % 2 == 0 else ((label, cand), ("base", BASE))):
            rows[k].append(run(binary))
    res = {"label": label, "fingerprint": fp, "fingerprint_ok": fp == FP_EXPECTED, "pairs": PAIRS}
    for k, v in rows.items():
        res[k] = {"nps": st.fmean(x[0] for x in v), "nps_cv": 100 * st.pstdev(x[0] for x in v) / st.fmean(x[0] for x in v),
                  "insn": st.fmean(x[1] for x in v), "cyc": st.fmean(x[2] for x in v)}
    pb, pc = rows["base"], rows[label]
    d_cyc = [100 * (c[2] - b[2]) / b[2] for b, c in zip(pb, pc)]
    d_nps = [100 * (c[0] - b[0]) / b[0] for b, c in zip(pb, pc)]
    res["paired"] = {"insn_pct": 100 * (res[label]["insn"] - res["base"]["insn"]) / res["base"]["insn"],
                     "cyc_pct_mean": st.fmean(d_cyc), "cyc_wins": sum(d < 0 for d in d_cyc),
                     "nps_pct_mean": st.fmean(d_nps), "nps_wins": sum(d > 0 for d in d_nps)}
    json.dump(res, open(f"{OUT}/{label}.json", "w"), indent=1)
    p = res["paired"]
    print(f"[{label}] fingerprint {fp} {'OK' if res['fingerprint_ok'] else '** CHANGED **'} | "
          f"instructions {p['insn_pct']:+.2f}% | cycles {p['cyc_pct_mean']:+.2f}% ({p['cyc_wins']}/{PAIRS} faster) | "
          f"nps {p['nps_pct_mean']:+.2f}% ({p['nps_wins']}/{PAIRS} wins) | base nps {res['base']['nps']:.0f} cv {res['base']['nps_cv']:.2f}%")


if __name__ == "__main__":
    main()
