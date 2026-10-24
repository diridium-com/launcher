#!/usr/bin/env python3
"""List the dependency versions that changed since the last release and how old
each one is, for the release rule that a new version must be at least 14 days
old (unless it fixes a critical or high advisory in what ships, which the
release notes then say).

It compares two git refs, by default the latest v* tag and HEAD, and looks up
the publish date of every version that is new at the second ref:

  - npm packages in package-lock.json (registry.npmjs.org)
  - Rust crates in src-tauri/Cargo.lock (crates.io, one request a second as its
    crawler policy asks)
  - GitHub actions pinned by commit SHA in .github/workflows (the commit's date;
    set GH_TOKEN for a higher API rate limit)
  - exact node-version values in .github/workflows (nodejs.org's release index)

Read-only: git and HTTPS GETs, nothing else. Run from the repo root:

    python3 scripts/dependency-ages.py
    python3 scripts/dependency-ages.py --since v2.4.0 --until master --at 2026-11-04

Exit status: 0 when everything new is old enough, 1 when something is too young,
2 when a lookup failed or a version couldn't be dated (never read as a pass).
"""
import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor

UA = {"User-Agent": "launcher dependency-ages (github.com/diridium-com/launcher)"}
SHA = re.compile(r"^[0-9a-f]{40}$")


def git(*args):
    r = subprocess.run(["git", *args], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def show(ref, path):
    return git("show", f"{ref}:{path}")


def get_json(url, headers=None):
    req = urllib.request.Request(url, headers={**UA, **(headers or {})})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read())


def when(text):
    return dt.datetime.fromisoformat(text.replace("Z", "+00:00"))


# --- what each ref pins -------------------------------------------------------

def npm_packages(ref):
    text = show(ref, "package-lock.json")
    if text is None:
        return set(), set()
    pkgs = json.loads(text).get("packages", {})
    registry, other = set(), set()
    for key, v in pkgs.items():
        if not key or "version" not in v:
            continue
        name = key.split("node_modules/")[-1]
        if v.get("resolved", "").startswith("https://registry.npmjs.org/"):
            registry.add((name, v["version"]))
        elif "resolved" in v:
            other.add((name, v["version"]))
    return registry, other


def crates(ref):
    text = show(ref, "src-tauri/Cargo.lock")
    if text is None:
        return set(), set()
    registry, other = set(), set()
    for block in text.split("[[package]]")[1:]:
        name = re.search(r'^name = "([^"]+)"', block, re.M)
        version = re.search(r'^version = "([^"]+)"', block, re.M)
        source = re.search(r'^source = "([^"]+)"', block, re.M)
        if not (name and version and source):
            continue  # the workspace's own crate has no source
        item = (name.group(1), version.group(1))
        (registry if source.group(1).startswith("registry+") else other).add(item)
    return registry, other


def workflows(ref):
    names = git("ls-tree", "-r", "--name-only", ref, ".github/workflows/") or ""
    return [show(ref, n) or "" for n in names.split() if n.endswith((".yml", ".yaml"))]


def actions(ref):
    pinned, unpinned = set(), set()
    for text in workflows(ref):
        for m in re.finditer(r"uses:\s*([^\s#]+)", text):
            use = m.group(1)
            if use.startswith("./") or "@" not in use:
                continue
            name, version = use.rsplit("@", 1)
            repo = "/".join(name.split("/")[:2])
            (pinned if SHA.match(version) else unpinned).add((repo, version))
    return pinned, unpinned


def node_versions(ref):
    exact, other = set(), set()
    for text in workflows(ref):
        for m in re.finditer(r"node-version:\s*['\"]?([^'\"\s#]+)", text):
            v = m.group(1).lstrip("v")
            (exact if re.fullmatch(r"\d+\.\d+\.\d+", v) else other).add(("node", v))
    return exact, other


# --- publish dates ------------------------------------------------------------

def npm_date(item):
    name, version = item
    doc = get_json("https://registry.npmjs.org/" + urllib.parse.quote(name, safe="@"))
    return doc["time"][version]


def crate_date(item):
    name, version = item
    return get_json(f"https://crates.io/api/v1/crates/{name}/{version}")["version"]["created_at"]


def action_date(item):
    repo, sha = item
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    commit = get_json(f"https://api.github.com/repos/{repo}/commits/{sha}", headers)
    return commit["commit"]["committer"]["date"]


def date_all(items, lookup, parallel=True):
    """Map each item to its publish date, or to an 'ERROR ...' string."""
    def one(item):
        try:
            return item, lookup(item)
        except Exception as e:  # noqa: BLE001 - every failure is reported
            return item, f"ERROR {e.__class__.__name__}: {e}"
    if parallel:
        with ThreadPoolExecutor(8) as ex:
            return dict(ex.map(one, items))
    out = {}
    for item in items:
        out.update([one(item)])
        time.sleep(1)  # crates.io asks crawlers for at most one request a second
    return out


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("--since", help="ref of the last release (default: latest v* tag)")
    p.add_argument("--until", default="HEAD", help="ref being released (default: HEAD)")
    p.add_argument("--days", type=int, default=14, help="minimum age in days (default: 14)")
    p.add_argument("--at", help="judge ages at 00:00 UTC on this date, YYYY-MM-DD (default: now)")
    a = p.parse_args()

    since = a.since or (git("describe", "--tags", "--abbrev=0", "--match", "v*", a.until) or "").strip()
    if not since or git("rev-parse", "--verify", "--quiet", since) is None:
        sys.exit(f"cannot resolve the last release ref ({since or 'no v* tag found'}); pass --since")
    at = (dt.datetime.strptime(a.at, "%Y-%m-%d").replace(tzinfo=dt.timezone.utc)
          if a.at else dt.datetime.now(dt.timezone.utc))
    print(f"New since {since} at {a.until}, judged at {at:%Y-%m-%d %H:%M} UTC, minimum {a.days} days")

    young, errors, unchecked, newest = [], [], [], None
    node_index = None
    kinds = [("npm", npm_packages, npm_date, True), ("crate", crates, crate_date, False),
             ("action", actions, action_date, True), ("node", node_versions, None, True)]
    for kind, pins, lookup, parallel in kinds:
        old_ok, old_other = pins(since)
        new_ok, new_other = pins(a.until)
        new = sorted(new_ok - old_ok)
        unchecked += [f"{kind} {n} {v}" for n, v in sorted(new_other - old_other)]
        if kind == "node":
            if new and node_index is None:
                try:
                    node_index = {r["version"].lstrip("v"): r["date"] for r in
                                  get_json("https://nodejs.org/dist/index.json")}
                except Exception as e:  # noqa: BLE001
                    node_index = {}
                    errors.append(f"node release index: {e}")
            dates = {i: node_index.get(i[1], "ERROR not in nodejs.org's index") for i in new}
        else:
            dates = date_all(new, lookup, parallel)
        print(f"  {kind}: {len(new)} new")
        for item, d in sorted(dates.items(), key=lambda kv: str(kv[1])):
            label = f"{kind} {item[0]} {item[1]}"
            if str(d).startswith("ERROR"):
                errors.append(f"{label}: {d}")
                continue
            published = when(d) if "T" in d else dt.datetime.fromisoformat(d).replace(tzinfo=dt.timezone.utc)
            newest = max(newest, published) if newest else published
            age = (at - published).total_seconds() / 86400
            if age < a.days:
                young.append((published, label, age))

    for published, label, age in sorted(young):
        print(f"  TOO YOUNG  {published:%Y-%m-%d}  {age:5.1f} days  {label}")
    for line in unchecked:
        print(f"  UNCHECKED  {line} (not from a registry, or not an exact version)")
    for line in errors:
        print(f"  ERROR      {line}")
    if newest:
        ok_from = newest + dt.timedelta(days=a.days)
        print(f"Everything dated is at least {a.days} days old from {ok_from:%Y-%m-%d %H:%M} UTC.")
    print(f"{len(young)} too young, {len(unchecked)} unchecked, {len(errors)} lookup errors.")
    sys.exit(2 if errors or unchecked else 1 if young else 0)


if __name__ == "__main__":
    main()
