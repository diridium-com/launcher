#!/usr/bin/env python3
"""Minimal W3C WebDriver client for driving the launcher through tauri-driver
(scripts/e2e/real-launch.py uses it; the commands below are for working by hand).

Standard library only. State (driver URL and session id) lives in the JSON file
named by WD_STATE, so each command is a separate process.

Usage:
  WD_STATE=<state.json> wd.py new <driver url> <app binary>
  wd.py wait <xpath> [timeout]      wait until an element exists, print its text
  wd.py gone <xpath> [timeout]      wait until no element matches
  wd.py exists <xpath>              print yes/no
  wd.py click <xpath>
  wd.py type <xpath> <text>         clear the field, then type
  wd.py text <xpath>
  wd.py attr <xpath> <name>
  wd.py exec <javascript>           run a script, print its JSON result
  wd.py shot <file.png>             screenshot of the current window
  wd.py handles                     list window handles
  wd.py switch <handle>
  wd.py source <file.html>
  wd.py quit

Also importable: Client(state_path).
"""
import base64
import json
import os
import sys
import time
import urllib.error
import urllib.request

ELEMENT = "element-6066-11e4-a52e-4f735466cecf"


class WebDriverError(Exception):
    pass


class Client:
    def __init__(self, state_path):
        self.state_path = state_path
        self.state = {}
        if os.path.exists(state_path):
            with open(state_path, encoding="utf-8") as f:
                self.state = json.load(f)

    def _save(self):
        with open(self.state_path, "w", encoding="utf-8") as f:
            json.dump(self.state, f)

    def _req(self, method, path, body=None, timeout=60):
        url = self.state["url"].rstrip("/") + path
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(url, data=data, method=method,
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                out = json.loads(r.read().decode() or "{}")
        except urllib.error.HTTPError as e:
            raise WebDriverError(f"{method} {path}: HTTP {e.code} {e.read().decode(errors='replace')}")
        return out.get("value")

    def _s(self, path):
        return f"/session/{self.state['session']}{path}"

    def new(self, url, app):
        self.state = {"url": url}
        caps = {"capabilities": {"alwaysMatch": {"tauri:options": {"application": app}}}}
        value = self._req("POST", "/session", caps, timeout=120)
        self.state["session"] = value["sessionId"]
        self._save()
        return value["sessionId"]

    def find_all(self, xpath):
        found = self._req("POST", self._s("/elements"), {"using": "xpath", "value": xpath})
        return [e[ELEMENT] for e in found]

    def wait(self, xpath, timeout=30.0):
        end = time.time() + timeout
        while True:
            found = self.find_all(xpath)
            if found:
                return found[0]
            if time.time() > end:
                raise WebDriverError(f"timed out after {timeout}s waiting for {xpath}")
            time.sleep(0.25)

    def gone(self, xpath, timeout=30.0):
        end = time.time() + timeout
        while self.find_all(xpath):
            if time.time() > end:
                raise WebDriverError(f"timed out after {timeout}s waiting for {xpath} to go")
            time.sleep(0.25)

    def click(self, xpath, timeout=30.0):
        el = self.wait(xpath, timeout)
        self._req("POST", self._s(f"/element/{el}/click"), {})

    def type(self, xpath, text, timeout=30.0):
        el = self.wait(xpath, timeout)
        self._req("POST", self._s(f"/element/{el}/clear"), {})
        self._req("POST", self._s(f"/element/{el}/value"), {"text": text})

    def text(self, xpath, timeout=30.0):
        el = self.wait(xpath, timeout)
        return self._req("GET", self._s(f"/element/{el}/text"))

    def attr(self, xpath, name, timeout=30.0):
        el = self.wait(xpath, timeout)
        return self._req("GET", self._s(f"/element/{el}/property/{name}"))

    def exec(self, script, args=None):
        return self._req("POST", self._s("/execute/sync"), {"script": script, "args": args or []})

    def shot(self, path):
        png = self._req("GET", self._s("/screenshot"))
        with open(path, "wb") as f:
            f.write(base64.b64decode(png))

    def handles(self):
        return self._req("GET", self._s("/window/handles"))

    def switch(self, handle):
        self._req("POST", self._s("/window"), {"handle": handle})

    def source(self):
        return self._req("GET", self._s("/source"))

    def quit(self):
        try:
            self._req("DELETE", self._s(""))
        finally:
            self.state.pop("session", None)
            self._save()


def main(argv):
    if len(argv) < 2:
        sys.exit(__doc__)
    state = os.environ.get("WD_STATE")
    if not state:
        sys.exit("WD_STATE is not set")
    c = Client(state)
    cmd, args = argv[1], argv[2:]
    if cmd == "new":
        print(c.new(args[0], args[1]))
    elif cmd == "wait":
        el = c.wait(args[0], float(args[1]) if len(args) > 1 else 30.0)
        print(c._req("GET", c._s(f"/element/{el}/text")))
    elif cmd == "gone":
        c.gone(args[0], float(args[1]) if len(args) > 1 else 30.0)
        print("gone")
    elif cmd == "exists":
        print("yes" if c.find_all(args[0]) else "no")
    elif cmd == "click":
        c.click(args[0])
        print("clicked")
    elif cmd == "type":
        c.type(args[0], args[1])
        print("typed")
    elif cmd == "text":
        print(c.text(args[0]))
    elif cmd == "attr":
        print(c.attr(args[0], args[1]))
    elif cmd == "exec":
        print(json.dumps(c.exec(args[0])))
    elif cmd == "shot":
        c.shot(args[0])
        print(args[0])
    elif cmd == "handles":
        print(json.dumps(c.handles()))
    elif cmd == "switch":
        c.switch(args[0])
        print("switched")
    elif cmd == "source":
        with open(args[0], "w", encoding="utf-8") as f:
            f.write(c.source())
        print(args[0])
    elif cmd == "quit":
        c.quit()
        print("quit")
    else:
        sys.exit(f"unknown command {cmd}\n{__doc__}")


if __name__ == "__main__":
    try:
        main(sys.argv)
    except WebDriverError as e:
        sys.exit(f"wd: {e}")
