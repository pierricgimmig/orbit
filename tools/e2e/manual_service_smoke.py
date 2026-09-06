#!/usr/bin/env python3
"""Manual instrumentation smoke test for Linux and macOS (requires pyarrow).

Exercises a separate, late-starting Python producer through the real C ABI,
HTTP controls, shared mappings, and Parquet export/import. No root or browser.
"""
import argparse
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[2]
LONG_NAME = "manual smoke: " + "long-name-" * 20


def producer():
    sys.path.insert(0, str(ROOT / "src/OrbitTestPython"))
    import orbit
    assert orbit.init() == 0, "manual API failed to initialize"
    for line in sys.stdin:
        label = line.strip()
        # A nonzero handle acknowledges the service discovered this segment.
        deadline = time.monotonic() + 10
        handle = 0
        while not handle and time.monotonic() < deadline:
            handle = orbit.start("ready")
            if not handle:
                time.sleep(0.02)
        assert handle, "producer was not discovered"
        orbit.stop(handle)
        with orbit.scope(label):
            with orbit.scope(LONG_NAME):
                time.sleep(0.01)
            async_handle = orbit.start_async(label + " async")
            def finish_async():
                time.sleep(0.01)
                orbit.stop(async_handle)
            worker = threading.Thread(target=finish_async)
            worker.start()
            worker.join()
            orbit.value(label + " value", 42)
            orbit.instant(label + " instant")
        # Stay alive until the parent has stopped/exported the capture.
        print("done", flush=True)
    orbit.shutdown()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--service", required=True)
    parser.add_argument("--api-lib", required=True)
    args = parser.parse_args()
    import pyarrow.parquet as parquet
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    base = f"http://127.0.0.1:{port}"

    def request(path, body=None):
        content_type = "application/octet-stream" if path.endswith("/import") else "application/json"
        req = urllib.request.Request(base + path, data=body, headers={"Content-Type": content_type})
        with urllib.request.urlopen(req, timeout=30) as response:
            return response.read()

    def get(path):
        return json.loads(request(path))

    with tempfile.TemporaryFile(mode="w+") as log:
        service = subprocess.Popen(
            [str(Path(args.service).resolve()), "--host", "127.0.0.1", "--serve", str(port)],
            stdout=log, stderr=log,
        )
        child = None
        try:
            deadline = time.monotonic() + 30
            while True:
                try:
                    get("/api/status")
                    break
                except OSError:
                    if service.poll() is not None or time.monotonic() >= deadline:
                        raise AssertionError("service did not start")
                    time.sleep(0.1)
            assert b"<html" in request("/").lower(), "embedded viewer missing"
            assert any(p["pid"] == service.pid for p in get("/api/processes"))
            if sys.platform == "darwin":
                try:
                    request("/api/capture/start", b'{"pid":0,"instrumented_functions":[{"function_id":1}]}')
                except urllib.error.HTTPError as error:
                    assert b"not yet supported on macOS" in error.read()
                else:
                    raise AssertionError("macOS silently accepted a dynamic hook")
                assert not get("/api/status")["capturing"]
            for label in ("first capture", "second capture"):
                request("/api/capture/start", b'{"pid":0}')
                if child is None:
                    # Initialize only after capture starts, exercising discovery.
                    env = dict(os.environ, ORBIT_API_LIB=str(Path(args.api_lib).resolve()))
                    child = subprocess.Popen([sys.executable, __file__, "--producer"],
                                             stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                             text=True, env=env)
                child.stdin.write(label + "\n")
                child.stdin.flush()
                # Bound the producer handshake without blocking forever on a pipe.
                import select
                ready, _, _ = select.select([child.stdout], [], [], 15)
                assert ready and child.stdout.readline().strip() == "done", "producer failed"
                request("/api/capture/stop", b"")
                status = get("/api/status")
                assert not status["capturing"], "stop returned before capture finished"
                assert get("/api/timeline")["instance_count"] > 0
                bundle = request("/api/capture/export?format=bundle")
                with zipfile.ZipFile(io.BytesIO(bundle)) as capture:
                    manifest = json.loads(capture.read("manifest.json"))
                    rows = parquet.read_table(io.BytesIO(capture.read(manifest["files"]["events"]))).to_pylist()
                ours = [row for row in rows if row["pid"] == child.pid]
                by_name = {row["name"]: row for row in ours}
                assert {label, LONG_NAME, label + " async", label + " value", label + " instant"} <= by_name.keys()
                assert by_name[LONG_NAME]["depth"] == 1
                assert by_name[label]["duration_ns"] >= by_name[LONG_NAME]["duration_ns"] > 0
                assert by_name[label + " async"]["duration_ns"] > 0
                assert all(row["start_ns"] >= status["capture_start_ns"] for row in ours)
                if label == "second capture":
                    assert "first capture" not in by_name, "restart retained previous session"
                assert any(p["pid"] == child.pid for p in manifest["bundle"]["processes"])
                imported = json.loads(request("/api/capture/import", bundle))
                assert imported["events"] == len(rows), "bundle did not round trip"
            if sys.platform == "darwin":
                with tempfile.TemporaryDirectory() as directory:
                    output = Path(directory) / "timed.orbit.zip"
                    timed = subprocess.Popen(
                        [str(Path(args.service).resolve()), "--pid", str(child.pid),
                         "--duration-ms", "1500", "--out", str(output)], stdout=log, stderr=log,
                    )
                    try:
                        child.stdin.write("timed capture\n")
                        child.stdin.flush()
                        ready, _, _ = select.select([child.stdout], [], [], 15)
                        assert ready and child.stdout.readline().strip() == "done"
                        assert timed.wait(timeout=10) == 0
                        with zipfile.ZipFile(output) as capture:
                            manifest = json.loads(capture.read("manifest.json"))
                            rows = parquet.read_table(io.BytesIO(capture.read(manifest["files"]["events"]))).to_pylist()
                        assert any(r["name"] == "timed capture" and r["pid"] == child.pid for r in rows)
                    finally:
                        if timed.poll() is None:
                            timed.kill()
                            timed.wait()
            print("PASS: viewer, discovery, nested/async/long scopes, values, export/import, restart")
        except BaseException:
            log.seek(0)
            print(log.read(), file=sys.stderr)
            raise
        finally:
            if child is not None:
                child.stdin.close()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
            service.terminate()
            try:
                service.wait(timeout=5)
            except subprocess.TimeoutExpired:
                service.kill()
                service.wait()


if __name__ == "__main__":
    if sys.argv[1:] == ["--producer"]:
        producer()
    else:
        main()
