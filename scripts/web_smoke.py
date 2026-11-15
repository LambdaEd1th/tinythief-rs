#!/usr/bin/env python3
"""Smoke test for the web build: boot it in headless Chrome in real time via CDP.

Usage:
    scripts/web_smoke.py --url http://127.0.0.1:8765/ [--profile /tmp/tt-profile]
                         [--seconds 25] [--expect-keys 3]

Requires the served `crates/tt-web/dist` and `pip install websocket-client`.

Why not plain `chrome --screenshot`? Chrome's `--virtual-time-budget` does not
deliver IndexedDB callbacks, so the page would hang on the storage backend.
This harness waits in real time and reads the page console over CDP instead.
"""
import argparse
import json
import re
import subprocess
import sys
import time
import urllib.request

import websocket

CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"


class Chrome:
    def __init__(self, profile: str, port: int = 9223, size: str = "1100,900"):
        self.proc = subprocess.Popen(
            [
                CHROME,
                "--headless=new",
                "--disable-gpu",
                "--enable-unsafe-swiftshader",
                "--no-sandbox",
                f"--remote-debugging-port={port}",
                f"--user-data-dir={profile}",
                f"--window-size={size}",
                "about:blank",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        self.port = port
        self.console = []
        self.timed_console = []
        self.frames = []
        self.next_id = 0
        self.ws = self._connect()

    def _connect(self):
        deadline = time.time() + 20
        targets = None
        while time.time() < deadline:
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/json") as r:
                    targets = json.load(r)
                break
            except Exception:
                time.sleep(0.2)
        if not targets:
            raise RuntimeError("chrome did not start")
        page = next(t for t in targets if t["type"] == "page")
        ws = websocket.create_connection(page["webSocketDebuggerUrl"], timeout=120)
        return ws

    def call(self, method, params=None):
        self.next_id += 1
        self.ws.send(json.dumps({"id": self.next_id, "method": method, "params": params or {}}))
        while True:
            msg = json.loads(self.ws.recv())
            if msg.get("id") == self.next_id:
                return msg.get("result")
            self._event(msg)

    def _event(self, msg):
        if msg.get("method") == "Page.screencastFrame":
            self.frames.append(msg["params"])
            # Ack without waiting for the response (calling `call` here would
            # re-enter the message loop and deadlock).
            self.next_id += 1
            self.ws.send(json.dumps({
                "id": self.next_id,
                "method": "Page.screencastFrameAck",
                "params": {"sessionId": msg["params"]["sessionId"]},
            }))
        if msg.get("method") == "Runtime.consoleAPICalled":
            args = msg["params"].get("args") or []
            text = " ".join(str(a.get("value", a.get("description", ""))) for a in args)
            self.console.append(text)
            self.timed_console.append((time.time(), text))

    def drain(self, seconds):
        end = time.time() + seconds
        while time.time() < end:
            try:
                self.ws.settimeout(0.5)
                msg = json.loads(self.ws.recv())
            except Exception:
                continue
            self._event(msg)

    def navigate(self, url):
        self.call("Page.enable")
        self.call("Runtime.enable")
        self.call("Page.navigate", {"url": url})

    def close(self):
        try:
            self.ws.close()
        finally:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=10)
            except Exception:
                self.proc.kill()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", required=True)
    ap.add_argument("--profile", default="/tmp/tt-web-smoke")
    ap.add_argument("--seconds", type=float, default=25.0)
    ap.add_argument("--probe", default=None,
                    help="JS expression to evaluate and print (e.g. '__ttStats()')")
    ap.add_argument("--expect-keys", type=int, default=None,
                    help="require IndexedDB to already contain N shared objects")
    args = ap.parse_args()

    chrome = Chrome(args.profile)
    try:
        chrome.navigate(args.url)
        chrome.drain(args.seconds)
        if args.probe:
            result = chrome.call(
                "Runtime.evaluate",
                {"expression": args.probe, "returnByValue": True, "awaitPromise": True},
            )
            print("PROBE", json.dumps(result.get("result", {}).get("value")))
    finally:
        chrome.close()

    keys = None
    ready = False
    panic = False
    for line in chrome.console:
        match = re.search(r"opened shared object store keys = (\d+)", line)
        if match:
            keys = int(match.group(1))
        if "player ready" in line:
            ready = True
        if "panicked" in line:
            panic = True

    print(f"player_ready={ready} panic={panic} indexeddb_keys={keys}")
    if panic or not ready:
        return 1
    if args.expect_keys is not None and keys != args.expect_keys:
        print(f"FAIL: expected {args.expect_keys} saved objects, got {keys}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
