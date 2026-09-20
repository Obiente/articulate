#!/usr/bin/env python3
"""Collect notices from a platform-filtered Cargo metadata dependency graph.

Usage: collect-license-notices.py METADATA_JSON OUTPUT_DIRECTORY
The metadata must include a resolve graph generated with --filter-platform.
Missing registry notices are fetched from the crate's exact published VCS commit.
Build and dev dependency edges are excluded. No local source paths are emitted.
"""

import argparse
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import sys
import tarfile
import urllib.parse
import urllib.request
import zipfile


MIT_TERMS = """Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
"""


def is_notice(path):
    name = PurePosixPath(str(path).replace("\\", "/")).name.lower()
    return (
        any(word in name for word in ("license", "licence", "copying", "copyright", "notice"))
        or name in ("ofl", "ofl.txt", "ufl", "ufl.txt")
        or ("fonts" in PurePosixPath(str(path).replace("\\", "/")).parts and name.endswith(".txt"))
    ) and not name.endswith((".rs", ".py", ".html", ".svg", ".json", ".toml"))


def runtime_packages(metadata):
    graph = metadata.get("resolve")
    if not graph or not graph.get("root"):
        raise ValueError("Metadata must have a resolved root package")
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in graph["nodes"]}
    pending = [graph["root"]]
    seen = set()
    while pending:
        identity = pending.pop()
        if identity in seen:
            continue
        seen.add(identity)
        for dependency in nodes[identity]["deps"]:
            if any(kind["kind"] is None for kind in dependency["dep_kinds"]):
                pending.append(dependency["pkg"])
    return sorted(
        (packages[identity] for identity in seen if identity != graph["root"]),
        key=lambda package: (package["name"], package["version"]),
    )


def read_registry_notices(package):
    root = Path(package["manifest_path"]).parent
    notices = {}
    for file in root.rglob("*"):
        if file.is_file() and is_notice(file.relative_to(root)):
            notices[file.relative_to(root).as_posix()] = file.read_bytes()
    if package.get("license_file"):
        file = (root / package["license_file"]).resolve()
        if not file.is_relative_to(root.resolve()):
            raise ValueError(f"{package['name']}: license_file leaves its published crate")
        notices[file.relative_to(root.resolve()).as_posix()] = file.read_bytes()
    return notices


def upstream_notices(package, cache):
    root = Path(package["manifest_path"]).parent
    vcs = json.loads((root / ".cargo_vcs_info.json").read_text(encoding="utf-8"))
    commit = vcs.get("git", {}).get("sha1", "")
    if not re.fullmatch(r"[a-fA-F0-9]{40}", commit):
        raise ValueError("Published crate has no pinned VCS commit")
    repository = urllib.parse.urlparse(package.get("repository") or "")
    parts = repository.path.strip("/").split("/")
    if len(parts) < 2 or repository.scheme != "https":
        raise ValueError("Published crate has no supported HTTPS repository")
    owner, name = parts[0], parts[1].removesuffix(".git")
    if repository.hostname == "github.com":
        url = f"https://codeload.github.com/{owner}/{name}/zip/{commit}"
    elif repository.hostname == "codeberg.org":
        url = f"https://codeberg.org/{owner}/{name}/archive/{commit}.tar.gz"
    else:
        raise ValueError("Pinned license retrieval supports GitHub and Codeberg repositories")
    if url not in cache:
        request = urllib.request.Request(url, headers={"User-Agent": "Articulate-License-Collector/1"})
        with urllib.request.urlopen(request, timeout=90) as response:
            archive = response.read(100_000_001)
        if len(archive) > 100_000_000:
            raise ValueError("License source archive exceeds size limit")
        found = {}
        if url.endswith(".tar.gz"):
            with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as tar:
                for member in tar.getmembers():
                    if member.isfile() and is_notice(member.name) and member.size <= 2_000_000:
                        stream = tar.extractfile(member)
                        if stream is not None:
                            found[member.name.partition("/")[2]] = stream.read()
        else:
            with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
                for member in zipped.infolist():
                    if not member.is_dir() and is_notice(member.filename) and member.file_size <= 2_000_000:
                        found[member.filename.partition("/")[2]] = zipped.read(member)
        cache[url] = found
    crate_path = PurePosixPath(vcs.get("path_in_vcs", ""))
    ancestors = {str(parent) for parent in (crate_path, *crate_path.parents)}
    selected = {}
    for path, data in cache[url].items():
        relative = PurePosixPath(path)
        # Include repository/workspace notices and the crate's own bundled assets.
        if str(relative.parent) in ancestors or relative.is_relative_to(crate_path):
            selected[f"upstream/{path}"] = data
    return selected, url


def collect(metadata_file, output):
    metadata = json.loads(metadata_file.read_text(encoding="utf-8-sig"))
    packages = runtime_packages(metadata)
    cache = {}
    entries = []
    missing = []
    output.mkdir(parents=True, exist_ok=True)
    for package in packages:
        label = f"{package['name']}-{package['version']}"
        if not re.fullmatch(r"[A-Za-z0-9_.+-]+", label):
            raise ValueError("Unsafe package identifier")
        notices = read_registry_notices(package)
        origin = None
        # egui's published font crate omits three required font notices, even
        # though the emoji notice is present. Recover its pinned upstream files.
        if not notices or package["name"] == "epaint_default_fonts":
            try:
                upstream, origin = upstream_notices(package, cache)
                notices.update(upstream)
            except Exception as error:
                missing.append(f"{label}: {type(error).__name__} retrieving pinned notices")
        if not notices and label == "realfft-3.5.0" and package.get("license") == "MIT":
            # This exact release and its upstream commit declare MIT but omit a
            # license file. Preserve the actual author/declaration and supply
            # standard terms, explicitly without inventing a copyright notice.
            root = Path(package["manifest_path"]).parent
            vcs = json.loads((root / ".cargo_vcs_info.json").read_text(encoding="utf-8"))
            if vcs.get("git", {}).get("sha1") != "d0d4eee0525fd27c96c8a046d6d107acd5ed84a6":
                raise ValueError("Unexpected realfft source revision")
            notice = (
                "RealFFT 3.5.0\n"
                + "Published authors: " + "; ".join(package.get("authors", [])) + "\n"
                + "Published license declaration: MIT\n"
                + "Source: https://github.com/HEnquist/realfft/tree/d0d4eee0525fd27c96c8a046d6d107acd5ed84a6\n\n"
                + "This release contains no separate copyright or license text.\n"
                + "The following standard MIT terms accompany its published declaration;\n"
                + "they are not represented as an upstream license file.\n"
                + "Terms reference: https://spdx.org/licenses/MIT.html\n\n"
                + MIT_TERMS
            )
            notices["MIT-declaration-and-terms.txt"] = notice.encode("utf-8")
        if not notices:
            missing.append(f"{label}: no license text found ({package.get('license')})")
        if package["name"] == "epaint_default_fonts":
            required = {"Hack-Regular.txt", "OFL.txt", "UFL.txt", "emoji-icon-font-mit-license.txt"}
            if not required.issubset({PurePosixPath(path).name for path in notices}):
                missing.append(f"{label}: incomplete bundled font notices")
        files = []
        for relative, data in sorted(notices.items()):
            path = PurePosixPath(relative)
            if path.is_absolute() or ".." in path.parts or not data.strip():
                raise ValueError(f"{label}: invalid notice")
            destination = output / label / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
            files.append({"path": f"{label}/{relative}", "sha256": hashlib.sha256(data).hexdigest()})
        entry = {"name": package["name"], "version": package["version"],
                 "license": package.get("license"), "repository": package.get("repository"), "notices": files}
        if origin:
            entry["pinned_notice_source"] = origin
        entries.append(entry)
    (output / "index.json").write_text(json.dumps(entries, indent=2) + "\n", encoding="utf-8")
    lines = ["# Rust dependency notices", "", "Dependencies reachable through normal dependency edges in the Windows Cargo resolution.",
             "Build-only and test-only dependencies are excluded. Font notices are included.", "",
             "| Dependency | Version | Declared license |", "| --- | --- | --- |"]
    lines.extend(f"| {entry['name']} | {entry['version']} | {entry['license'] or 'See notice'} |" for entry in entries)
    (output / "README.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    if missing:
        raise ValueError("Missing notices; distribution must not proceed:\n" + "\n".join(missing))
    print(f"Collected notices for {len(entries)} resolved Windows dependencies.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("metadata", type=Path)
    parser.add_argument("output", type=Path)
    arguments = parser.parse_args()
    try:
        collect(arguments.metadata, arguments.output)
    except Exception as failure:
        print(f"License collection failed: {failure}", file=sys.stderr)
        sys.exit(1)
