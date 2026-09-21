#!/usr/bin/env python3
"""Drive code-change experiments on Stockfish through the Orbit optimize-loop
MCP server: baseline (+ noise-floor batches) -> for each patch: apply_patch ->
rebuild -> rerun_compare -> gate -> save binary -> revert tree.

Every number printed comes straight from the tool results."""
import json, os, shutil, statistics, subprocess, sys, time
sys.path.insert(0, "/home/pierric/git/orbit/tools/optimize-loop")
import loop as orbit_loop  # evaluate_gate (the real gate, not a re-implementation)
import threading


class MCP:
    """Minimal JSON-RPC-over-stdio client for tools/optimize-loop/mcp/server.py."""
    SERVER = ["python3", "-u", "/home/pierric/git/orbit/tools/optimize-loop/mcp/server.py"]

    def __init__(self):
        self.p = subprocess.Popen(self.SERVER, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True, bufsize=1)
        self._id = 0
        threading.Thread(target=self._drain_err, daemon=True).start()

    def _drain_err(self):
        for line in self.p.stderr:
            sys.stderr.write("[srv] " + line)

    def call(self, method, params=None):
        self._id += 1
        msg = {"jsonrpc": "2.0", "id": self._id, "method": method}
        if params is not None:
            msg["params"] = params
        self.p.stdin.write(json.dumps(msg) + "\n"); self.p.stdin.flush()
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError("server closed")
            line = line.strip()
            if not line:
                continue
            obj = json.loads(line)
            if obj.get("id") == self._id:
                return obj

    def tool(self, name, args):
        res = self.call("tools/call", {"name": name, "arguments": args}).get("result", {})
        for c in res.get("content", []):
            if c.get("type") == "text":
                try:
                    return json.loads(c["text"])
                except Exception:
                    return c["text"]
        return res

S = "<scratch>"
CFG = S + "/stockfish.toml"
N = S + "/opt-notes/run2"
SF = "/home/pierric/git/stockfish"
ITERS = int(os.environ.get("ITERS", "8"))
NOISE_BATCHES = int(os.environ.get("NOISE_BATCHES", "3"))
TRACE = S + "/opt-notes/TRACE.md"
COOL = int(os.environ.get("COOL", "20"))  # seconds to let the CPU settle after a 16-core build


def log(msg):
    with open(TRACE, "a") as f:
        f.write(f"- {time.strftime('%H:%M:%S')}  {msg}\n")
    print(msg, flush=True)


def bench(out):
    b = out.get("bench") or {}
    return b.get("mean"), b.get("stdev"), b.get("fingerprint"), len(b.get("values") or [])


def main():
    m = MCP()
    m.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                          "clientInfo": {"name": "experiment-driver", "version": "0"}})
    assert subprocess.run(["git", "-C", SF, "status", "--short"], capture_output=True, text=True).stdout == "", "tree dirty"

    # --- baseline + noise floor -------------------------------------------
    base_out = N + "/suite-baseline"
    m.tool("optimize_rebuild", {"config": CFG}); time.sleep(COOL)
    r = m.tool("optimize_run_suite", {"config": CFG, "skip_build": True, "capture": "none",
                                      "bench_iters": ITERS, "out": base_out})
    bmean, bsd, bfp, n = bench(r)
    log(f"baseline via optimize_run_suite: mean={bmean:.0f} nps stdev={bsd:.0f} (cv {100*bsd/bmean:.2f}%) fp={bfp} n={n}")
    batch_means = [bmean]
    for i in range(NOISE_BATCHES - 1):
        r2 = m.tool("optimize_rerun_compare", {"config": CFG, "baseline": base_out, "bench_iters": ITERS,
                                               "capture": "none", "skip_build": True, "out": N + f"/suite-noise-{i+1}"})
        m2, s2, f2, _ = bench(r2)
        batch_means.append(m2)
        log(f"noise batch {i+1}: mean={m2:.0f} (Δ vs baseline {100*(m2-bmean)/bmean:+.2f}%) fp={f2}")
    between = statistics.pstdev(batch_means)
    grand = statistics.fmean(batch_means)
    floor = 100.0 * (max(batch_means) - min(batch_means)) / grand
    eff_sd = max(bsd or 0.0, between)
    log(f"noise floor: batch means {[round(x) for x in batch_means]} → between-batch stdev {between:.0f} "
        f"({100*between/grand:.2f}%), floor (max-min) {floor:.2f}%, σ used = max(within,between) = {eff_sd:.0f}")
    shutil.copy(SF + "/src/stockfish", N + "/stockfish-baseline")
    results = {"baseline": {"mean": bmean, "stdev": bsd, "fingerprint": bfp, "batch_means": batch_means,
                            "noise_floor_percent": floor, "effective_stdev": eff_sd}, "experiments": []}

    # --- experiments -------------------------------------------------------
    for label, patch in [a.split("=", 1) for a in sys.argv[1:]]:
        build_cfg = CFG
        if patch.startswith("cfg:"):  # a build-level change: same tree, different build command
            build_cfg = patch[4:]
            log(f"[{label}] build-level variant: rebuild with {os.path.basename(build_cfg)}")
        else:
            diff = open(patch).read()
            ap = m.tool("optimize_apply_patch", {"config": CFG, "unified_diff": diff, "apply": True})
            if not ap.get("applied"):
                log(f"[{label}] apply_patch FAILED: {ap.get('error')} {ap.get('git_apply_check','')[:200]}")
                continue
            log(f"[{label}] optimize_apply_patch applied ({len(diff.splitlines())} diff lines)")
        rb = m.tool("optimize_rebuild", {"config": build_cfg}); time.sleep(COOL)
        if not rb.get("ok", rb.get("returncode", 1) == 0):
            log(f"[{label}] rebuild FAILED: {str(rb)[:300]}")
            subprocess.run(["git", "-C", SF, "checkout", "--", "src"])
            continue
        log(f"[{label}] optimize_rebuild ok ({rb.get('duration_sec', rb.get('elapsed_sec', '?'))}s)")
        rc = m.tool("optimize_rerun_compare", {"config": CFG, "baseline": base_out, "bench_iters": ITERS,
                                               "capture": "none", "skip_build": True, "out": N + f"/suite-{label}"})
        cmean, csd, cfp, cn = bench(rc)
        gate = orbit_loop.evaluate_gate(baseline_mean=grand, current_mean=cmean, baseline_stdev=eff_sd,
                                        baseline_fingerprint=str(bfp), current_fingerprint=str(cfp),
                                        noise_floor_percent=floor)
        shutil.copy(SF + "/src/stockfish", N + f"/stockfish-{label}")
        subprocess.run(["git", "-C", SF, "checkout", "--", "src"], check=True)
        log(f"[{label}] rerun_compare: mean={cmean:.0f} stdev={csd:.0f} fp={cfp} n={cn} → "
            f"Δ {100*(cmean-grand)/grand:+.2f}% vs grand baseline {grand:.0f}; gate={'ACCEPT' if gate['accepted'] else 'reject'}: "
            + "; ".join(gate.get("reasons", [])))
        results["experiments"].append({"label": label, "patch": patch, "mean": cmean, "stdev": csd,
                                       "fingerprint": cfp, "delta_percent": 100*(cmean-grand)/grand, "gate": gate})
    json.dump(results, open(N + "/experiments.json", "w"), indent=1, default=str)
    clean = subprocess.run(["git", "-C", SF, "status", "--short"], capture_output=True, text=True).stdout == ""
    log(f"experiments done; stockfish tree clean={clean}")


if __name__ == "__main__":
    main()
