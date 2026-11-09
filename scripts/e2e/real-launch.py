#!/usr/bin/env python3
"""Launch the real administrator through Launcher against a running engine, and check it.

Usage: real-launch.py <Launcher binary> <engine URL> <admin password> <new run folder>

Needs JAVA_HOME (a JavaFX-enabled JDK, used by the administrator), and tauri-driver,
WebKitWebDriver, Xvfb and dbus-run-session on PATH. Start the engine first
(scripts/e2e/start-engine.sh). Standard library only.

1. Starts a private display and tauri-driver (start-display.sh) with a fresh HOME in which
   ~/.launcher/launcher-data.json holds one connection to the engine, never launched before:
   no pinned certificate, nothing downloaded.
2. Clicks its Launch button and logs every status-bar message. The certificate prompt must
   appear and is answered with "Trust certificate".
3. After that answer the status bar must show the launch moving on ("Fetching server
   configuration" or "Downloading"). Before Launcher 2.4.1 it stayed on "Connecting..."
   for the whole download, because the progress channel didn't survive the retry after the
   prompt.
4. The launch must end without an error, and the engine's event log must then show a new
   successful Login by admin: the administrator started and logged in.

Writes status.log, events.json, screenshots and the drivers' logs into the run folder, stops
the display, the app and the administrator, and exits 0 only when every check passed.
"""
import json
import os
import ssl
import subprocess
import sys
import time
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from wd import Client, WebDriverError  # noqa: E402

NAME = "e2e engine"
CONN_ID = "e2e00000-0000-4000-8000-000000000001"
POLL = """
const bar = [...document.querySelectorAll('div.border-t.bg-surface-1 p.truncate')].map(p => p.textContent.trim());
const err = [...document.querySelectorAll('p.text-danger.truncate')].map(p => p.textContent.trim());
const buttons = [...document.querySelectorAll('button')].map(b => b.textContent.trim());
return {bar, err, trust: buttons.includes('Trust certificate'), cont: buttons.includes('Continue')};
"""
MOVED_ON = ("Fetching server configuration", "Downloading")


class Engine:
    """The engine's REST API, with the session cookie and the header it requires."""

    def __init__(self, url, password):
        self.base = url.rstrip("/") + "/api"
        ctx = ssl.create_default_context()
        ctx.check_hostname = False
        ctx.verify_mode = ssl.CERT_NONE  # a test engine on 127.0.0.1 with its own self-signed certificate
        self.opener = urllib.request.build_opener(urllib.request.HTTPSHandler(context=ctx),
                                                  urllib.request.HTTPCookieProcessor())
        self.call("POST", "/users/_login", urllib.parse.urlencode({"username": "admin", "password": password}).encode(),
                  "application/x-www-form-urlencoded")

    def call(self, method, path, body=None, content_type=None):
        headers = {"X-Requested-With": "OpenAPI", "Accept": "application/json"}
        if content_type:
            headers["Content-Type"] = content_type
        req = urllib.request.Request(self.base + path, data=body, method=method, headers=headers)
        with self.opener.open(req, timeout=60) as r:
            text = r.read().decode()
        return json.loads(text) if text.strip().startswith(("{", "[")) else text

    def events(self, min_id):
        data = self.call("GET", f"/events?minEventId={min_id}&limit=100")
        found = ((data or {}).get("list") or {}).get("event") or []
        return found if isinstance(found, list) else [found]


def admin_logins(events):
    out = []
    for e in events:
        attrs = json.dumps(e.get("attributes", {}))
        if e.get("name") == "Login" and e.get("outcome") == "SUCCESS" and '"admin"' in attrs:
            out.append(e)
    return out


def seed(home, url, password, java_home):
    data = os.path.join(home, ".launcher")
    os.makedirs(data, mode=0o700)
    conn = {CONN_ID: {
        # The keys the app writes (src-tauri/src/connection.rs, serde renames).
        "id": CONN_ID, "name": NAME, "address": url, "heapSize": "512m",
        "javaHome": java_home, "javaArgs": "", "username": "admin", "password": password,
        "group": "Default", "notes": "", "donotcache": False, "lastConnected": None,
        "showConsole": False, "suppressPortMismatch": False,
    }}
    path = os.path.join(data, "launcher-data.json")
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        json.dump(conn, f)


def main():
    if len(sys.argv) != 5:
        sys.exit(__doc__)
    app, url, password, run = sys.argv[1:]
    app = os.path.abspath(app)
    java_home = os.environ.get("JAVA_HOME") or sys.exit("JAVA_HOME is not set")
    os.makedirs(run)
    run = os.path.abspath(run)
    home = os.path.join(run, "home")
    os.makedirs(home)
    seed(home, url, password, java_home)
    log = open(os.path.join(run, "status.log"), "w", encoding="utf-8")

    def say(msg):
        print(msg, flush=True)
        log.write(msg + "\n")
        log.flush()

    problems = []
    display = os.path.join(run, "display")
    engine = Engine(url, password)
    baseline = max([int(e["id"]) for e in engine.events(0)] or [0])
    say(f"engine {url}: event log at id {baseline}")
    subprocess.run(["bash", os.path.join(HERE, "start-display.sh"), display, home], check=True)
    c = Client(os.path.join(run, "wd-state.json"))
    try:
        with open(os.path.join(display, "wd-url.txt"), encoding="utf-8") as f:
            c.new(f.read().strip(), app)
        c.wait(f"//button[@aria-label='Launch {NAME}']", 60)
        c.shot(os.path.join(run, "01-before-launch.png"))
        c.click(f"//button[@aria-label='Launch {NAME}']")
        t0 = time.time()
        last, seen_bar, answered_at, after_trust, outcome = None, False, None, [], None
        while outcome is None:
            t = time.time() - t0
            s = c.exec(POLL)
            key = (tuple(s["bar"]), tuple(s["err"]))
            if key != last:
                say(f"{t:7.2f}  bar={s['bar']} err={s['err']}")
                last = key
                if answered_at is not None:
                    after_trust += s["bar"]
            seen_bar = seen_bar or bool(s["bar"])
            if s["trust"] and answered_at is None:
                say(f"{t:7.2f}  certificate prompt shown; clicking Trust certificate")
                c.shot(os.path.join(run, "02-trust-prompt.png"))
                c.click("//button[normalize-space()='Trust certificate']")
                answered_at = t
            if s["cont"]:
                say(f"{t:7.2f}  port-mismatch prompt shown; clicking Continue")
                c.click("//button[normalize-space()='Continue']")
            if s["err"]:
                outcome = "error"
            elif seen_bar and not s["bar"]:
                outcome = "finished"
            elif t > 600:
                outcome = "timeout"
            time.sleep(0.05)
        c.shot(os.path.join(run, "03-after-launch.png"))
        say(f"launch {outcome} after {time.time() - t0:.1f} s")
        if answered_at is None:
            problems.append("the certificate prompt never appeared for a connection launched for the first time")
        elif not any(m.startswith(MOVED_ON) for m in after_trust):
            problems.append("after the certificate was trusted the status bar never showed the launch moving on "
                            f"(saw {sorted(set(after_trust))})")
        if outcome != "finished":
            problems.append(f"the launch did not finish cleanly: {outcome}")
        else:
            end = time.time() + 180
            logins = []
            while not logins and time.time() < end:
                time.sleep(3)
                logins = admin_logins(engine.events(baseline + 1))
            with open(os.path.join(run, "events.json"), "w", encoding="utf-8") as f:
                json.dump(engine.events(baseline + 1), f, indent=1)
            if logins:
                say(f"engine event {logins[0]['id']}: Login by admin, {logins[0]['outcome']}, from {logins[0].get('ipAddress')}")
            else:
                problems.append("no new successful Login by admin in the engine's event log within 3 minutes")
            # The whole screen, administrator included, when ImageMagick is there (optional evidence).
            try:
                with open(os.path.join(display, "display.txt"), encoding="utf-8") as f:
                    shown = f":{f.read().strip()}"
                subprocess.run(["import", "-display", shown, "-window", "root", os.path.join(run, "04-screen.png")], check=False)
            except FileNotFoundError:
                say("no screen capture: ImageMagick's import is not installed")
    except WebDriverError as e:
        problems.append(f"WebDriver: {e}")
    finally:
        try:
            c.quit()
        except Exception:  # noqa: BLE001 - the display is stopped either way
            pass
        subprocess.run(["bash", os.path.join(HERE, "stop.sh"),
                        os.path.join(display, "driver.pid"), os.path.join(display, "xvfb.pid")], check=False)
    for p in problems:
        say(f"FAIL {p}")
    if problems:
        return 1
    say("PASS: trusted on first use, progress after the prompt, admin logged in to the engine")
    return 0


if __name__ == "__main__":
    sys.exit(main())
