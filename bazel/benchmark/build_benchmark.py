#!/usr/bin/env python3
# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""Measures what building Orbit costs, in time and in disk.

Two questions, both answered against the checkout this runs in:

  * How long does a build from scratch take, with nothing cached anywhere, and
    how much disk does it leave behind?
  * How long is one iteration -- edit a file, build again -- for the kinds of
    edit that actually happen?

Every measurement runs in an output base of the benchmark's own, so a run never
disturbs the output base you work in, and never depends on what happens to be
cached there.

Run it with

    bazel run //bazel/benchmark:build_benchmark

or directly, without Bazel building anything first:

    python3 bazel/benchmark/build_benchmark.py

See --help for the scenarios and how to run a subset of them.
"""

import argparse
import datetime
import json
import os
import platform
import re
import shutil
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass, field, asdict
from pathlib import Path

# ------------------------------------------------------------------ utilities


def workspace_root() -> Path:
    """The checkout to measure: where `bazel run` was called, or this file's."""
    from_bazel = os.environ.get("BUILD_WORKSPACE_DIRECTORY")
    if from_bazel:
        return Path(from_bazel)
    return Path(__file__).resolve().parents[2]


def run(command, cwd, env=None):
    """Runs `command`, returning (seconds, exit code, combined output)."""
    started = time.monotonic()
    completed = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    return time.monotonic() - started, completed.returncode, completed.stdout


def directory_size_bytes(path: Path) -> int:
    """Size of `path` on disk, or 0 if it does not exist."""
    if not path.exists():
        return 0
    # `du` is considerably faster than walking the tree in Python, and an output
    # base is millions of files.
    result = subprocess.run(
        ["du", "--summarize", "--bytes", "--one-file-system", str(path)],
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    if result.returncode != 0 or not result.stdout:
        return 0
    return int(result.stdout.split()[0])


def tracked_source_size_bytes(root: Path) -> int:
    """Size of the files git tracks, so build directories do not inflate it."""
    try:
        listed = subprocess.run(
            ["git", "-C", str(root), "ls-files", "-z"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, check=True,
        )
        summed = subprocess.run(
            ["du", "--files0-from=-", "--bytes", "--total", "--summarize"],
            input=listed.stdout, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            cwd=str(root),
        )
        for line in reversed(summed.stdout.decode().splitlines()):
            if line.endswith("total"):
                return int(line.split()[0])
    except (subprocess.CalledProcessError, OSError, ValueError):
        pass
    return 0


def human_bytes(count: int) -> str:
    if count <= 0:
        return "-"
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if count < 1024 or unit == "TiB":
            return f"{count:.1f} {unit}" if unit != "B" else f"{count} B"
        count /= 1024.0
    return f"{count:.1f} TiB"


def human_duration(seconds: float) -> str:
    if seconds < 1:
        return f"{seconds * 1000:.0f} ms"
    if seconds < 60:
        return f"{seconds:.1f} s"
    minutes, remainder = divmod(int(round(seconds)), 60)
    if minutes < 60:
        return f"{minutes}m {remainder:02d}s"
    hours, minutes = divmod(minutes, 60)
    return f"{hours}h {minutes:02d}m"


# Bazel reports what it did on two lines we can lift numbers from:
#   INFO: Elapsed time: 44.300s, Critical Path: 28.67s
#   INFO: 50 processes: 523 action cache hit, 1 internal, 49 processwrapper-sandbox.
_ELAPSED = re.compile(r"^INFO: Elapsed time: ([0-9.]+)s", re.MULTILINE)
_PROCESSES = re.compile(r"^INFO: ([0-9,]+) process(?:es)?: (.+)\.$", re.MULTILINE)
_CACHE_HIT = re.compile(r"([0-9,]+) (disk cache hit|remote cache hit|action cache hit)")


def parse_bazel_output(output: str) -> dict:
    parsed = {"bazel_elapsed_s": None, "processes": None, "cache_hits": 0}
    elapsed = _ELAPSED.search(output)
    if elapsed:
        parsed["bazel_elapsed_s"] = float(elapsed.group(1))
    processes = _PROCESSES.search(output)
    if processes:
        parsed["processes"] = int(processes.group(1).replace(",", ""))
        parsed["cache_hits"] = sum(
            int(count.replace(",", "")) for count, _ in _CACHE_HIT.findall(processes.group(2))
        )
    return parsed


# ------------------------------------------------------------------- the runs


@dataclass
class Measurement:
    name: str
    description: str
    wall_s: float
    bazel_elapsed_s: float = None
    processes: int = None
    cache_hits: int = 0
    repeats: int = 1
    all_wall_s: list = field(default_factory=list)
    ok: bool = True
    note: str = ""


@dataclass
class DiskUsage:
    name: str
    path: str
    bytes: int
    note: str = ""


class Benchmark:
    def __init__(self, args):
        self.args = args
        self.root = workspace_root()
        self.measurements: list[Measurement] = []
        self.disk: list[DiskUsage] = []
        self.artifacts: list[tuple[str, int]] = []
        # Deliberately not /tmp: that is a tmpfs on many distributions, and an
        # output base for this tree is tens of gigabytes.
        self.scratch = Path(
            os.path.expanduser(args.scratch or "~/.cache/bazel/orbit-build-benchmark")
        )
        self.scratch.mkdir(parents=True, exist_ok=True)
        self.nonce = f"{time.time_ns():x}"
        self.output_base = self.scratch / "output_base"
        self.repository_cache = self.scratch / "repository_cache"
        # A disk cache of the benchmark's own. It starts empty, so the cold
        # build really is cold, and what it holds afterwards is what one full
        # build costs in cache. Point --disk-cache at ~/.cache/bazel/orbit to
        # measure against the cache this checkout actually uses instead.
        self.owns_disk_cache = args.disk_cache is None
        self.disk_cache = Path(
            os.path.expanduser(args.disk_cache or (self.scratch / "disk_cache"))
        )

    # -- bazel invocations ---------------------------------------------------

    def bazel(self, command_args, output_base=None, disk_cache=None,
              repository_cache=None):
        """Runs bazel in an output base of ours, never the caller's."""
        base = output_base or self.output_base
        argv = ["bazel", f"--output_base={base}"]
        argv += command_args
        # An empty --disk_cache disables it, which is what "no cache hits
        # whatsoever" needs; passing a path overrides whatever .bazelrc says.
        argv.append(f"--disk_cache={disk_cache or ''}")
        if repository_cache is not None:
            argv.append(f"--repository_cache={repository_cache}")
        if self.args.verbose:
            print(f"    $ {' '.join(argv)}", flush=True)
        return run(argv, cwd=self.root)

    def shutdown(self, output_base=None):
        subprocess.run(
            ["bazel", f"--output_base={output_base or self.output_base}", "shutdown"],
            cwd=self.root,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    def record(self, name, description, seconds, output, repeats=1, all_seconds=None,
               ok=True, note=""):
        parsed = parse_bazel_output(output)
        measurement = Measurement(
            name=name,
            description=description,
            wall_s=seconds,
            bazel_elapsed_s=parsed["bazel_elapsed_s"],
            processes=parsed["processes"],
            cache_hits=parsed["cache_hits"],
            repeats=repeats,
            all_wall_s=all_seconds or [seconds],
            ok=ok,
            note=note,
        )
        self.measurements.append(measurement)
        status = "ok" if ok else "FAILED"
        print(f"  {name}: {human_duration(seconds)} ({status})", flush=True)
        return measurement

    # -- scenarios -----------------------------------------------------------

    def scenario_cold(self):
        """From scratch: no output base, no disk cache, nothing downloaded."""
        print("cold: fetching every dependency, then building everything", flush=True)
        self.shutdown()
        for path in (self.output_base, self.repository_cache):
            shutil.rmtree(path, ignore_errors=True)
        if self.owns_disk_cache:
            # A second run in the same scratch directory would otherwise find
            # the first run's cache and report a warm build as a cold one.
            shutil.rmtree(self.disk_cache, ignore_errors=True)
        else:
            print("  note: --disk-cache was given, so this build starts warm and "
                  "is not a cold one", flush=True)

        # Not `bazel fetch`: it forces every module extension to be evaluated,
        # which fails on rules_rust's crate_universe here. An analysis-only
        # build fetches exactly what the build needs, and nothing else.
        seconds, code, output = self.bazel(
            ["build", "--nobuild", self.args.target],
            disk_cache=self.disk_cache,
            repository_cache=self.repository_cache,
        )
        cold_ok = code == 0
        self.record(
            "cold-fetch",
            "Download and analyse every dependency (needs the network)",
            seconds,
            output,
            ok=cold_ok,
            note="" if cold_ok else "fetch/analysis failed",
        )

        seconds, code, output = self.bazel(
            ["build", self.args.target],
            disk_cache=self.disk_cache,
            repository_cache=self.repository_cache,
        )
        cold_ok = cold_ok and code == 0
        self.record(
            "cold-build",
            f"Compile and link {self.args.target}, every cache empty",
            seconds,
            output,
            ok=cold_ok,
            note="" if code == 0 else "build failed",
        )
        if cold_ok and self.args.measure_disk:
            self.measure_disk(after="cold")
        self.shutdown()

    def scenario_clean_cached(self):
        """A fresh output base, but the shared disk cache is warm."""
        print("clean-cached: fresh output base, warm disk cache", flush=True)
        self.shutdown()
        shutil.rmtree(self.output_base, ignore_errors=True)
        seconds, code, output = self.bazel(
            ["build", self.args.target],
            disk_cache=self.disk_cache,
            repository_cache=self.repository_cache,
        )
        self.record(
            "clean-cached",
            f"Build {self.args.target} in a new output base, caches warm",
            seconds,
            output,
            ok=(code == 0),
        )
        if not self.disk and self.args.measure_disk and code == 0:
            self.measure_disk(after="clean-cached")

    def ensure_built(self):
        """Everything below measures an edit, so the tree has to be built first."""
        if any(m.name in ("cold-build", "clean-cached") and m.ok for m in self.measurements):
            return True
        print("preparing: building once so that iteration can be measured", flush=True)
        seconds, code, output = self.bazel(
            ["build", self.args.target],
            disk_cache=self.disk_cache,
            repository_cache=self.repository_cache,
        )
        print(f"  prepared in {human_duration(seconds)}", flush=True)
        return code == 0

    def timed_repeats(self, command_args, edit: Path = None):
        """Runs a scenario `--repeats` times, returning the per-run seconds."""
        results = []
        last_output = ""
        original = edit.read_text() if edit else None
        try:
            for iteration in range(self.args.repeats):
                if edit:
                    # Unique per run: the same edit twice would be served from
                    # the disk cache the second time, and the number would be a
                    # cache lookup rather than a compile.
                    edit.write_text(
                        original
                        + f"\n// build benchmark {self.nonce} iteration {iteration}\n"
                    )
                seconds, code, last_output = self.bazel(
                    command_args,
                    disk_cache=self.disk_cache,
                    repository_cache=self.repository_cache,
                )
                results.append(seconds)
                if code != 0:
                    return results, last_output, False
        finally:
            if edit and original is not None:
                edit.write_text(original)
        return results, last_output, True

    def scenario_noop(self):
        print("noop: nothing changed", flush=True)
        results, output, ok = self.timed_repeats(["build", self.args.target])
        self.record(
            "noop",
            f"`bazel build {self.args.target}` with nothing changed",
            statistics.median(results),
            output,
            repeats=len(results),
            all_seconds=results,
            ok=ok,
        )

    def scenario_edit(self, name, description, relative_path, build_args):
        path = self.root / relative_path
        if not path.exists():
            print(f"  skipping {name}: {relative_path} is gone", flush=True)
            return
        print(f"{name}: editing {relative_path}", flush=True)
        results, output, ok = self.timed_repeats(build_args, edit=path)
        self.record(
            name,
            description,
            statistics.median(results),
            output,
            repeats=len(results),
            all_seconds=results,
            ok=ok,
        )

    # -- disk ----------------------------------------------------------------

    def measure_disk(self, after: str):
        print(f"measuring disk after the {after} build (this walks a few million "
              "files)", flush=True)
        execroot = self.output_base / "execroot" / "_main"
        entries = [
            DiskUsage("Output base", str(self.output_base),
                      directory_size_bytes(self.output_base),
                      "one per checkout, throwaway"),
            DiskUsage("- external repositories", str(self.output_base / "external"),
                      directory_size_bytes(self.output_base / "external"),
                      "sources of everything Bazel fetched"),
            DiskUsage("- build outputs", str(execroot / "bazel-out"),
                      directory_size_bytes(execroot / "bazel-out"),
                      "objects, archives, binaries"),
            DiskUsage("Repository cache", str(self.repository_cache),
                      directory_size_bytes(self.repository_cache),
                      "downloads, shared by every checkout"),
            DiskUsage("Disk cache", str(self.disk_cache),
                      directory_size_bytes(self.disk_cache),
                      "action results, shared by every checkout"),
            DiskUsage("Source checkout", str(self.root),
                      tracked_source_size_bytes(self.root), "the files git tracks"),
        ]
        self.disk = entries
        self.artifacts = self.largest_outputs(execroot / "bazel-out")
        for entry in entries:
            print(f"  {entry.name}: {human_bytes(entry.bytes)}", flush=True)

    def largest_outputs(self, bazel_out: Path, count: int = 12):
        """The biggest files the build produced -- where the disk actually goes."""
        if not bazel_out.exists():
            return []
        interesting_suffixes = (".so", ".a", ".dylib", ".dll", ".lib")
        largest = []
        for directory, _, files in os.walk(bazel_out, followlinks=False):
            for name in files:
                candidate = Path(directory) / name
                try:
                    info = candidate.lstat()
                except OSError:
                    continue
                executable = bool(info.st_mode & 0o111)
                if not executable and not name.endswith(interesting_suffixes):
                    continue
                largest.append((str(candidate.relative_to(bazel_out)), info.st_size))
        largest.sort(key=lambda entry: entry[1], reverse=True)
        return largest[:count]

    # -- reporting -----------------------------------------------------------

    def machine(self) -> dict:
        def first_line(command):
            try:
                return subprocess.run(
                    command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                    text=True, shell=isinstance(command, str)
                ).stdout.strip().splitlines()[0]
            except (IndexError, OSError):
                return "unknown"

        cpu_model = "unknown"
        try:
            for line in Path("/proc/cpuinfo").read_text().splitlines():
                if line.startswith("model name"):
                    cpu_model = line.split(":", 1)[1].strip()
                    break
        except OSError:
            pass
        memory_gib = None
        try:
            for line in Path("/proc/meminfo").read_text().splitlines():
                if line.startswith("MemTotal:"):
                    memory_gib = round(int(line.split()[1]) / 1024 / 1024, 1)
                    break
        except OSError:
            pass
        distribution = "unknown"
        try:
            for line in Path("/etc/os-release").read_text().splitlines():
                if line.startswith("PRETTY_NAME="):
                    distribution = line.split("=", 1)[1].strip().strip('"')
                    break
        except OSError:
            pass
        return {
            "cpu": cpu_model,
            "logical_cores": os.cpu_count(),
            "memory_gib": memory_gib,
            "os": distribution,
            "kernel": platform.release(),
            "bazel": first_line(["bazel", "--version"]),
            "compiler": first_line("gcc --version"),
            "commit": first_line(["git", "-C", str(self.root), "rev-parse", "--short", "HEAD"]),
            "date": datetime.date.today().isoformat(),
        }

    def to_markdown(self, machine: dict) -> str:
        lines = []
        lines.append("| Scenario | What it measures | Wall time | Actions run | Cache hits |")
        lines.append("| --- | --- | --- | --- | --- |")
        for m in self.measurements:
            wall = human_duration(m.wall_s)
            if m.repeats > 1:
                wall += f" (median of {m.repeats})"
            if not m.ok:
                wall += " ⚠️"
            lines.append(
                f"| `{m.name}` | {m.description} | {wall} | "
                f"{m.processes if m.processes is not None else '-'} | "
                f"{m.cache_hits or '-'} |"
            )
        timing_table = "\n".join(lines)

        disk_table = ""
        if self.disk:
            lines = ["| What | Size | Shared between checkouts? |", "| --- | --- | --- |"]
            for entry in self.disk:
                lines.append(f"| {entry.name} | {human_bytes(entry.bytes)} | {entry.note} |")
            total = sum(e.bytes for e in self.disk if not e.name.startswith("- "))
            lines.append(f"| **Total** | **{human_bytes(total)}** | |")
            disk_table = "\n".join(lines)

        artifact_table = ""
        if self.artifacts:
            lines = ["| Output | Size |", "| --- | --- |"]
            for path, size in self.artifacts:
                lines.append(f"| `{path}` | {human_bytes(size)} |")
            artifact_table = "\n".join(lines)

        machine_lines = [
            f"- **CPU**: {machine['cpu']} ({machine['logical_cores']} logical cores)",
            f"- **Memory**: {machine['memory_gib']} GiB",
            f"- **OS**: {machine['os']}, kernel {machine['kernel']}",
            f"- **Bazel**: {machine['bazel']}",
            f"- **Compiler**: {machine['compiler']}",
            f"- **Commit**: `{machine['commit']}`, measured {machine['date']}",
        ]

        scenario_note = ", ".join(self.args.scenarios)
        parts = [
            "# What it costs to build Orbit",
            "",
            "Generated by `bazel run //bazel/benchmark:build_benchmark`. Re-run it to",
            "replace this file with the numbers for your machine; the tables below are",
            "one machine's, not a promise.",
            "",
            "```",
            "bazel run //bazel/benchmark:build_benchmark             # everything below",
            "bazel run //bazel/benchmark:build_benchmark -- --quick  # iteration only",
            "```",
            "",
            "The benchmark builds in an output base, a repository cache and a disk cache",
            "of its own, so a run neither disturbs nor benefits from the caches you build",
            "with: `cold` starts with all three empty, which is as close to a first-ever",
            "build as a machine gets without being reinstalled. Budget ~15 GB of free",
            "disk for the scratch directory, and a re-download of every dependency.",
            "",
            "## The machine these numbers came from",
            "",
            "\n".join(machine_lines),
            f"- **Scenarios**: {scenario_note}",
            "",
            "## Time",
            "",
            timing_table,
        ]
        if disk_table:
            parts += [
                "",
                "## Disk",
                "",
                "What one full build leaves behind. The output base belongs to a single",
                "checkout and can be deleted at any time. In everyday use the repository",
                "cache and the disk cache are shared by every checkout on the machine and",
                "grow beyond this as branches and configurations accumulate; the sizes",
                "here are what a single build from scratch puts in them.",
                "",
                disk_table,
            ]
        if artifact_table:
            parts += [
                "",
                "## Largest build outputs",
                "",
                "Where the output base actually goes, and the place to start if it needs",
                "to get smaller. Binaries that appear twice under different paths are",
                "copies of one file, not two builds of it.",
                "",
                artifact_table,
            ]
        return "\n".join(parts) + "\n"

    # -- driver --------------------------------------------------------------

    def run_all(self):
        selected = self.args.scenarios
        # A build with a fresh repository cache records what it downloaded in
        # MODULE.bazel.lock. Measuring should not leave the checkout dirty, so
        # put the file back the way it was.
        lockfile = self.root / "MODULE.bazel.lock"
        lockfile_before = lockfile.read_bytes() if lockfile.exists() else None
        try:
            if "cold" in selected:
                self.scenario_cold()
            if "clean-cached" in selected:
                self.scenario_clean_cached()
            iteration_scenarios = {"noop", "edit-cpp", "edit-header", "edit-test"}
            if iteration_scenarios & set(selected):
                if not self.ensure_built():
                    print("the preparing build failed; iteration numbers would be "
                          "meaningless", file=sys.stderr)
                    return 1
            if "noop" in selected:
                self.scenario_noop()
            if "edit-cpp" in selected:
                self.scenario_edit(
                    "edit-cpp",
                    "Edit one .cpp in a library, rebuild OrbitService",
                    "src/UserSpaceInstrumentation/InstrumentProcess.cpp",
                    ["build", "//src/Service:OrbitService"],
                )
            if "edit-header" in selected:
                self.scenario_edit(
                    "edit-header",
                    "Edit a header everything includes, rebuild OrbitService",
                    "src/OrbitBase/include/OrbitBase/Logging.h",
                    ["build", "//src/Service:OrbitService"],
                )
            if "edit-test" in selected:
                # A test that does not need root, ptrace or a network, so that
                # the number is a build-and-run cost and not a flake.
                self.scenario_edit(
                    "edit-test",
                    "Edit a test, build and run that test target",
                    "src/OrbitBase/ThreadUtilsTest.cpp",
                    ["test", "//src/OrbitBase:OrbitBaseTests"],
                )
            if not self.disk and self.args.measure_disk:
                self.measure_disk(after="last")
        finally:
            self.shutdown()
            if lockfile_before is not None and lockfile.read_bytes() != lockfile_before:
                lockfile.write_bytes(lockfile_before)
                print("restored MODULE.bazel.lock", flush=True)
            if not self.args.keep_scratch:
                print(f"removing {self.scratch}", flush=True)
                shutil.rmtree(self.scratch, ignore_errors=True)

        machine = self.machine()
        markdown = self.to_markdown(machine)
        print()
        print(markdown)

        if self.args.out and self.args.out != "-":
            out_path = Path(self.args.out)
            if not out_path.is_absolute():
                out_path = self.root / out_path
            out_path.parent.mkdir(parents=True, exist_ok=True)
            out_path.write_text(markdown)
            print(f"wrote {out_path}", flush=True)
        if self.args.json:
            json_path = Path(self.args.json)
            if not json_path.is_absolute():
                json_path = self.root / json_path
            json_path.write_text(
                json.dumps(
                    {
                        "machine": machine,
                        "measurements": [asdict(m) for m in self.measurements],
                        "disk": [asdict(d) for d in self.disk],
                        "largest_outputs": self.artifacts,
                    },
                    indent=2,
                )
                + "\n"
            )
            print(f"wrote {json_path}", flush=True)
        return 0 if all(m.ok for m in self.measurements) else 1


def render_from_json(args) -> int:
    """Rewrites the report from numbers measured earlier, building nothing."""
    saved = json.loads(Path(args.from_json).read_text())
    benchmark = Benchmark.__new__(Benchmark)
    benchmark.args = args
    benchmark.root = workspace_root()
    benchmark.measurements = [Measurement(**m) for m in saved["measurements"]]
    benchmark.disk = [DiskUsage(**d) for d in saved["disk"]]
    benchmark.artifacts = [tuple(pair) for pair in saved["largest_outputs"]]
    markdown = benchmark.to_markdown(saved["machine"])
    print(markdown)
    if args.out and args.out != "-":
        out_path = Path(args.out)
        if not out_path.is_absolute():
            out_path = benchmark.root / out_path
        out_path.write_text(markdown)
        print(f"wrote {out_path}", flush=True)
    return 0


ALL_SCENARIOS = ["cold", "clean-cached", "noop", "edit-cpp", "edit-header", "edit-test"]
QUICK_SCENARIOS = ["noop", "edit-cpp", "edit-header", "edit-test"]


def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--scenarios",
        default=",".join(ALL_SCENARIOS),
        help="comma-separated subset of: " + ", ".join(ALL_SCENARIOS),
    )
    parser.add_argument(
        "--quick",
        action="store_true",
        help="only the iteration scenarios, against the disk cache this "
             "checkout is configured to use, so nothing is built from scratch",
    )
    parser.add_argument("--target", default="//...", help="what to build (default //...)")
    parser.add_argument("--repeats", type=int, default=3,
                        help="how often to repeat each iteration scenario (default 3)")
    parser.add_argument("--out", default="docs/build_benchmark.md",
                        help="where to write the report, relative to the checkout "
                             "(default docs/build_benchmark.md; - for stdout only)")
    parser.add_argument("--json", default=None, help="write the raw numbers to this file")
    parser.add_argument("--scratch", default=None,
                        help="where to put the throwaway output base "
                             "(default ~/.cache/bazel/orbit-build-benchmark; "
                             "needs tens of GB, and not on a tmpfs)")
    parser.add_argument("--keep-scratch", action="store_true",
                        help="keep the throwaway output base instead of deleting it")
    parser.add_argument("--disk-cache", default=None,
                        help="disk cache to use (default: one inside --scratch, "
                             "so the run starts cold and leaves your own cache "
                             "alone)")
    parser.add_argument("--no-measure-disk", dest="measure_disk", action="store_false",
                        help="skip the disk measurements, which walk the output base")
    parser.add_argument("--verbose", action="store_true", help="print every bazel command")
    parser.add_argument("--from-json", default=None,
                        help="re-render the report from a --json file, without "
                             "building anything")
    args = parser.parse_args()

    if args.quick and args.disk_cache is None:
        # Otherwise --quick would begin with a full build from scratch, which is
        # the opposite of quick. The edits measured are unique per run, so a
        # warm cache cannot answer them.
        args.disk_cache = "~/.cache/bazel/orbit"
    args.scenarios = QUICK_SCENARIOS if args.quick else [
        s.strip() for s in args.scenarios.split(",") if s.strip()
    ]
    unknown = set(args.scenarios) - set(ALL_SCENARIOS)
    if unknown:
        parser.error(f"unknown scenario(s): {', '.join(sorted(unknown))}")

    if args.from_json:
        return render_from_json(args)

    if shutil.which("bazel") is None:
        print("bazel is not on PATH", file=sys.stderr)
        return 1

    return Benchmark(args).run_all()


if __name__ == "__main__":
    sys.exit(main())
