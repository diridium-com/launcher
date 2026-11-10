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
  - GitHub actions pinned by commit SHA in .github/workflows: the later of the
    commit's date and the publish date of the GitHub release named in the pin's
    comment (`@<sha> # v1.2.3`), after checking the SHA is that tag's commit. A
    commit more than a day newer than its release is an error: the tag was moved
    after the release, which is what a hijacked account does. Without such a
    release, the commit's date, with a warning (a committer can set that date).
    Set GH_TOKEN for a higher API rate limit
  - exact node-version values in .github/workflows (nodejs.org's release index)
  - exact Rust toolchain values in .github/workflows (static.rust-lang.org's
    release manifest)
  - jsign, pinned in scripts/sign-windows.sh (the jar's upload to Maven Central)
  - Microsoft.Windows.SDK.BuildTools, pinned in scripts/pack-msix.ps1 (NuGet)

Not covered: the NSIS and WiX that the Tauri CLI downloads (their versions are
fixed by the CLI's own version, which the npm check covers), and the launch job's
test tools in build-launcher.yml (tauri-driver, the Zulu JDK, the OIE tarball)
and cargo-audit, which are pinned and dated in the comments beside them.

Read-only: git and HTTPS GETs, nothing else. Run from the repo root:

    python3 scripts/dependency-ages.py
    python3 scripts/dependency-ages.py --since v2.4.0 --until master --at 2026-11-04

Exit status: 0 when everything new is old enough, 1 when something is too young,
2 when a lookup failed or a version couldn't be dated, 3 when the refs can't be
resolved. Anything but 0 is never read as a pass.
"""
import argparse
import datetime as dt
import email.utils
import gzip
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor

UA = {"User-Agent": "launcher dependency-ages (github.com/diridium-com/launcher)"}
SHA = re.compile(r"^[0-9a-f]{40}$")
WARNINGS = []  # appended from worker threads; list.append is atomic in CPython


def git(*args):
    r = subprocess.run(["git", *args], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


def show(ref, path):
    return git("show", f"{ref}:{path}")


def get_json(url, headers=None):
    req = urllib.request.Request(url, headers={**UA, **(headers or {})})
    with urllib.request.urlopen(req, timeout=60) as r:
        data = r.read()
        if r.headers.get("Content-Encoding") == "gzip":
            data = gzip.decompress(data)
        return json.loads(data)


def github(path):
    token = os.environ.get("GH_TOKEN") or os.environ.get("GITHUB_TOKEN")
    headers = {"Authorization": f"Bearer {token}"} if token else {}
    return get_json(f"https://api.github.com/{path}", headers)


def when(text):
    text = text.replace("Z", "+00:00")
    # Python 3.10's fromisoformat takes only 3 or 6 digits of fractional seconds.
    text = re.sub(r"\.(\d+)", lambda m: "." + (m.group(1) + "000000")[:6], text, count=1)
    return dt.datetime.fromisoformat(text)


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
    """(repo, sha, tag named in the comment or "") for SHA pins; (repo, version, "") otherwise."""
    pinned, unpinned = set(), set()
    for text in workflows(ref):
        for m in re.finditer(r"uses:\s*([^\s#]+)[ \t]*(?:#[ \t]*([^\s,]+))?", text):
            use = m.group(1)
            if use.startswith("./") or "@" not in use:
                continue
            name, version = use.rsplit("@", 1)
            repo = "/".join(name.split("/")[:2])
            tag = m.group(2) or ""
            (pinned if SHA.match(version) else unpinned).add((repo, version, tag))
    return pinned, unpinned


def rust_toolchains(ref):
    exact, other = set(), set()
    for text in workflows(ref):
        for m in re.finditer(r"toolchain:\s*['\"]?([^'\"\s#]+)", text):
            v = m.group(1)
            (exact if re.fullmatch(r"\d+\.\d+\.\d+", v) else other).add(("rust", v))
    return exact, other


def jsign_versions(ref):
    m = re.search(r"^JSIGN_VERSION=(\S+)$", show(ref, "scripts/sign-windows.sh") or "", re.M)
    return ({("jsign", m.group(1))} if m else set()), set()


def nuget_versions(ref):
    m = re.search(r"/microsoft\.windows\.sdk\.buildtools/([0-9][0-9.]*)/", show(ref, "scripts/pack-msix.ps1") or "")
    return ({("microsoft.windows.sdk.buildtools", m.group(1))} if m else set()), set()


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
    repo, sha, tag = item
    if tag:
        try:
            release = github(f"repos/{repo}/releases/tags/{urllib.parse.quote(tag)}")
        except urllib.error.HTTPError as e:
            if e.code != 404:
                raise
            release = None
        if release:
            tag_sha = github(f"repos/{repo}/commits/{urllib.parse.quote(tag)}")["sha"]
            if tag_sha != sha:
                raise ValueError(f"pinned {sha} is not {tag}, which is {tag_sha}")
            committed = github(f"repos/{repo}/commits/{sha}")["commit"]["committer"]["date"]
            if when(committed) > when(release["published_at"]) + dt.timedelta(days=1):
                raise ValueError(f"{tag}'s commit is dated {committed}, after its release "
                                 f"({release['published_at']}): was the tag moved?")
            return max(committed, release["published_at"], key=when)
    WARNINGS.append(f"action {repo} {sha}: no GitHub release for '{tag}', dated by its commit date")
    return github(f"repos/{repo}/commits/{sha}")["commit"]["committer"]["date"]


def rust_date(item):
    url = f"https://static.rust-lang.org/dist/channel-rust-{item[1]}.toml"
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60) as r:
        head = r.read(8192).decode("utf-8", "replace")
    return re.search(r'^date = "([0-9-]+)"', head, re.M).group(1)


def jsign_date(item):
    v = item[1]
    url = f"https://repo1.maven.org/maven2/net/jsign/jsign/{v}/jsign-{v}.jar"
    req = urllib.request.Request(url, headers=UA, method="HEAD")
    with urllib.request.urlopen(req, timeout=60) as r:
        return email.utils.parsedate_to_datetime(r.headers["Last-Modified"]).isoformat()


def nuget_date(item):
    name, v = item
    return get_json(f"https://api.nuget.org/v3/registration5-gz-semver2/{name}/{v}.json")["published"]


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
        print(f"cannot resolve the last release ref ({since or 'no v* tag found'}); pass --since", file=sys.stderr)
        sys.exit(3)
    at = (dt.datetime.strptime(a.at, "%Y-%m-%d").replace(tzinfo=dt.timezone.utc)
          if a.at else dt.datetime.now(dt.timezone.utc))
    print(f"New since {since} at {a.until}, judged at {at:%Y-%m-%d %H:%M} UTC, minimum {a.days} days")

    young, errors, unchecked, newest = [], [], [], None
    node_index = None
    kinds = [("npm", npm_packages, npm_date, True), ("crate", crates, crate_date, False),
             ("action", actions, action_date, True), ("node", node_versions, None, True),
             ("rust", rust_toolchains, rust_date, True), ("jsign", jsign_versions, jsign_date, True),
             ("nuget", nuget_versions, nuget_date, True)]
    for kind, pins, lookup, parallel in kinds:
        old_ok, old_other = pins(since)
        new_ok, new_other = pins(a.until)
        new = sorted(new_ok - old_ok)
        unchecked += [f"{kind} {i[0]} {i[1]}" for i in sorted(new_other - old_other)]
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
            try:
                published = when(d) if "T" in d else dt.datetime.fromisoformat(d).replace(tzinfo=dt.timezone.utc)
            except (TypeError, ValueError) as e:
                errors.append(f"{label}: can't read the date {d!r} ({e})")
                continue
            newest = max(newest, published) if newest else published
            age = (at - published).total_seconds() / 86400
            if age < a.days:
                young.append((published, label, age))

    for published, label, age in sorted(young):
        print(f"  TOO YOUNG  {published:%Y-%m-%d}  {age:5.1f} days  {label}")
    for line in unchecked:
        print(f"  UNCHECKED  {line} (not from a registry, or not an exact version)")
    for line in sorted(WARNINGS):
        print(f"  WARNING    {line}")
    for line in errors:
        print(f"  ERROR      {line}")
    if newest:
        ok_from = newest + dt.timedelta(days=a.days)
        print(f"Everything dated is at least {a.days} days old from {ok_from:%Y-%m-%d %H:%M} UTC.")
    print(f"{len(young)} too young, {len(unchecked)} unchecked, {len(errors)} lookup errors.")
    sys.exit(2 if errors or unchecked else 1 if young else 0)


if __name__ == "__main__":
    main()
