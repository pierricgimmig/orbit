# Copyright (c) 2026 The Orbit Authors. All rights reserved.
# Use of this source code is governed by a BSD-style license that can be
# found in the LICENSE file.

"""A minimal Chrome DevTools Protocol client, standard library only.

Why not playwright or websocket-client: this harness has to run wherever the
repository is checked out, and on this machine pip refuses to install into the
system Python (PEP 668) while no websocket module is present. Rather than make
screenshots depend on a working package index, the ~100 lines of RFC 6455 that
CDP actually needs are here.

Why CDP rather than `chrome --screenshot`: that path drives the page with
virtual time, which never advances a requestAnimationFrame loop. The viewer is
a WASM app that paints from rAF, so a virtual-time screenshot captures the
clear colour and nothing else -- which is exactly what it did the first time.
Real time plus an explicit wait is the only thing that photographs a frame.
"""

import base64
import json
import os
import socket
import struct
import subprocess
import time
import urllib.request


class CdpError(RuntimeError):
    pass


class WebSocket:
    """Just enough of RFC 6455 to talk to Chrome: text frames, client-masked."""

    def __init__(self, url, timeout=30.0):
        if not url.startswith("ws://"):
            raise CdpError(f"only ws:// is supported, got {url}")
        rest = url[len("ws://"):]
        hostport, _, path = rest.partition("/")
        host, _, port = hostport.partition(":")
        self.sock = socket.create_connection((host, int(port or 80)), timeout=timeout)
        self.sock.settimeout(timeout)
        key = base64.b64encode(os.urandom(16)).decode()
        handshake = (
            f"GET /{path} HTTP/1.1\r\n"
            f"Host: {hostport}\r\n"
            "Upgrade: websocket\r\n"
            "Connection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\n"
            "Sec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(handshake.encode())
        self._buf = b""
        while b"\r\n\r\n" not in self._buf:
            chunk = self.sock.recv(4096)
            if not chunk:
                raise CdpError("connection closed during handshake")
            self._buf += chunk
        head, _, self._buf = self._buf.partition(b"\r\n\r\n")
        if b"101" not in head.split(b"\r\n")[0]:
            raise CdpError(f"handshake refused: {head.splitlines()[0]!r}")

    def _recv_exact(self, n):
        while len(self._buf) < n:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise CdpError("connection closed")
            self._buf += chunk
        out, self._buf = self._buf[:n], self._buf[n:]
        return out

    def send(self, text):
        payload = text.encode()
        header = bytearray([0x81])  # FIN + text
        mask = os.urandom(4)
        n = len(payload)
        if n < 126:
            header.append(0x80 | n)
        elif n < (1 << 16):
            header.append(0x80 | 126)
            header += struct.pack(">H", n)
        else:
            header.append(0x80 | 127)
            header += struct.pack(">Q", n)
        header += mask
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        self.sock.sendall(bytes(header) + masked)

    def recv(self):
        """One complete text message, reassembling continuation frames."""
        chunks = []
        while True:
            b0, b1 = self._recv_exact(2)
            fin = b0 & 0x80
            opcode = b0 & 0x0F
            n = b1 & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._recv_exact(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._recv_exact(8))[0]
            data = self._recv_exact(n) if n else b""
            if opcode == 0x8:
                raise CdpError("server closed the socket")
            if opcode == 0x9:  # ping -> pong
                self.sock.sendall(b"\x8a\x80" + os.urandom(4))
                continue
            if opcode == 0xA:
                continue
            chunks.append(data)
            if fin:
                return b"".join(chunks).decode("utf-8", "replace")

    def close(self):
        try:
            self.sock.close()
        except OSError:
            pass


class Chrome:
    """A headless Chrome, driven over CDP."""

    def __init__(self, port=9333, width=1600, height=1000, binary="google-chrome"):
        self.port = port
        self.profile = f"/tmp/orbit-e2e-chrome-{port}"
        self.proc = subprocess.Popen(
            [
                binary,
                "--headless=new",
                "--disable-gpu",
                # The viewer renders through wgpu; without a GPU, SwiftShader
                # is what makes WebGL exist at all in this container.
                "--enable-unsafe-swiftshader",
                "--no-sandbox",
                "--hide-scrollbars",
                "--mute-audio",
                "--disable-dev-shm-usage",
                f"--remote-debugging-port={port}",
                f"--user-data-dir={self.profile}",
                f"--window-size={width},{height}",
                "about:blank",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.ws = None
        self._id = 0
        self._connect()

    def _connect(self, timeout=30.0):
        deadline = time.time() + timeout
        last = None
        while time.time() < deadline:
            try:
                raw = urllib.request.urlopen(
                    f"http://127.0.0.1:{self.port}/json/list", timeout=2
                ).read()
                pages = [t for t in json.loads(raw) if t.get("type") == "page"]
                if pages:
                    self.ws = WebSocket(pages[0]["webSocketDebuggerUrl"])
                    return
            except Exception as exc:  # noqa: BLE001 - retried until the deadline
                last = exc
            time.sleep(0.25)
        raise CdpError(f"chrome did not expose a page target: {last}")

    def call(self, method, **params):
        self._id += 1
        want = self._id
        self.ws.send(json.dumps({"id": want, "method": method, "params": params}))
        while True:
            message = json.loads(self.ws.recv())
            if message.get("id") != want:
                continue  # an event, or a reply to something we no longer await
            if "error" in message:
                raise CdpError(f"{method}: {message['error']}")
            return message.get("result", {})

    def goto(self, url, settle=6.0):
        self.call("Page.enable")
        self.call("Page.navigate", url=url)
        # No load-event wait: the viewer keeps loading assets and workers well
        # past it, and what matters is that it has painted a few frames.
        time.sleep(settle)

    def eval(self, expression):
        result = self.call(
            "Runtime.evaluate", expression=expression, returnByValue=True, awaitPromise=True
        )
        return result.get("result", {}).get("value")

    def click(self, x, y, button="left", hold=0.05):
        """A press and release at a canvas position. Right-clicks must be
        quick: the viewer opens its scope menu on release and treats a
        press held longer than a beat as a drag."""
        buttons = {"left": 1, "right": 2}[button]
        self.call("Input.dispatchMouseEvent", type="mouseMoved", x=x, y=y)
        self.call("Input.dispatchMouseEvent", type="mousePressed", x=x, y=y,
                  button=button, buttons=buttons, clickCount=1)
        time.sleep(hold)
        self.call("Input.dispatchMouseEvent", type="mouseReleased", x=x, y=y,
                  button=button, buttons=0, clickCount=1)

    def move(self, x, y):
        self.call("Input.dispatchMouseEvent", type="mouseMoved", x=x, y=y)

    def key(self, name, code=None, vk=None):
        """A key press and release, by DOM key name ("Escape", "Home")."""
        codes = {"Escape": ("Escape", 27), "Home": ("Home", 36), "Enter": ("Enter", 13)}
        code, vk = codes.get(name, (code or name, vk or 0))
        for kind in ("keyDown", "keyUp"):
            self.call("Input.dispatchKeyEvent", type=kind, key=name, code=code,
                      windowsVirtualKeyCode=vk, nativeVirtualKeyCode=vk)

    def screenshot(self, path):
        data = self.call("Page.captureScreenshot", format="png", captureBeyondViewport=False)
        raw = base64.b64decode(data["data"])
        os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
        with open(path, "wb") as handle:
            handle.write(raw)
        return len(raw)

    def close(self):
        if self.ws:
            self.ws.close()
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
