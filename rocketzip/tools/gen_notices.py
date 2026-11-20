#!/usr/bin/env python3
"""Generate THIRD-PARTY-NOTICES.txt for the Windows build of RocketZip.

Walks the dependency graph Cargo resolves for x86_64-pc-windows-msvc (normal
dependencies only, so build tools and test helpers are skipped) and copies every
license / copying / notice file each crate ships, including those of the C and C++
libraries vendored inside -sys crates (zlib, zlib-ng, xz, zstd, bzip2, UnRAR).

Run from the workspace root:  python tools/gen_notices.py
"""
import json, os, re, subprocess, sys

TARGET = "x86_64-pc-windows-msvc"
OUT = "THIRD-PARTY-NOTICES.txt"
NAME_RE = re.compile(r"^(licen[cs]e|copying|notice|unrar.*licen[cs]e|copyright)([-_.].*)?$", re.I)
SKIP_DIRS = {".git", "target", "tests", "test", "examples", "benches", "doc", "docs",
             "fuzz", "testdata", "contrib"}
MAX_DEPTH = 3


def license_files(root):
    found = []
    for dirpath, dirs, files in os.walk(root):
        depth = os.path.relpath(dirpath, root).count(os.sep) + (dirpath != root)
        dirs[:] = sorted(d for d in dirs if d.lower() not in SKIP_DIRS and depth < MAX_DEPTH)
        for f in sorted(files):
            if NAME_RE.match(f) and not f.lower().endswith((".rs", ".py", ".c", ".h", ".sh")):
                found.append(os.path.join(dirpath, f))
    return found


def main():
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", TARGET]
        + sys.argv[1:]))
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    pkgs = {p["id"]: p for p in meta["packages"]}
    seen, stack = set(), [i for i, p in pkgs.items() if p["source"] is None]
    while stack:
        i = stack.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            if any(k["kind"] is None for k in d["dep_kinds"]):
                stack.append(d["pkg"])
    shipped = sorted((pkgs[i] for i in seen if pkgs[i]["source"]),
                     key=lambda p: (p["name"], p["version"]))

    out = [
        "RocketZip - Third-party notices",
        "=" * 72,
        "",
        "RocketZip includes the open-source components listed below. Each is",
        "distributed under its own license, reproduced in full after the summary.",
        "Where a component offers a choice of licenses, RocketZip uses it under",
        "the MIT license when MIT is one of the choices.",
        "",
        "UnRAR: RocketZip uses the UnRAR source code (via the unrar_sys crate) only",
        "to extract RAR archives. Per the UnRAR license, this code may not be used",
        "to re-create the RAR compression algorithm. RocketZip cannot create RAR",
        "archives.",
        "",
        "Summary",
        "-" * 72,
    ]
    for p in shipped:
        out.append(f"{p['name']} {p['version']}  -  {p.get('license') or 'see license file'}")
    out += ["", ""]
    missing = []
    texts = {}  # normalised text -> (label, first owner)
    for p in shipped:
        root = os.path.dirname(p["manifest_path"])
        files = license_files(root)
        out.append("=" * 72)
        out.append(f"{p['name']} {p['version']}")
        if p.get("repository"):
            out.append(p["repository"])
        if p.get("authors"):
            out.append("Authors: " + ", ".join(p["authors"]))
        out.append(f"License: {p.get('license') or 'see below'}")
        out.append("=" * 72)
        if not files:
            missing.append(p["name"])
            out.append("(No license file shipped in the crate; license identifier above applies.")
            out.append(" Standard texts of MIT and Apache-2.0 are included at the end.)")
        for f in files:
            rel = os.path.relpath(f, root).replace(os.sep, "/")
            try:
                text = open(f, encoding="utf-8", errors="replace").read().strip()
            except OSError:
                continue
            key = " ".join(text.split())
            if key in texts:
                out += ["", f"--- {rel}: identical to {texts[key]} above ---"]
                continue
            texts[key] = f"{p['name']} {p['version']} {rel}"
            out += ["", f"--- {rel} ---", "", text]
        out += ["", ""]
    std = os.path.join(os.path.dirname(os.path.abspath(__file__)), "standard-licenses.txt")
    if os.path.exists(std):
        out += ["=" * 72, "Standard license texts", "=" * 72, "",
                open(std, encoding="utf-8").read().strip(), ""]
    with open(OUT, "w", encoding="utf-8", newline="\n") as fh:
        fh.write("\n".join(out))
    print(f"{OUT}: {len(shipped)} components, {os.path.getsize(OUT)//1024} KB")
    if missing:
        print("no license file in crate (identifier only):", ", ".join(missing))


if __name__ == "__main__":
    main()
