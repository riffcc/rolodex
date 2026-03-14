#!/usr/bin/env python3
import argparse
import json
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
META = ROOT / ".github" / "rolodex" / "release.json"


def git_short_sha() -> str:
    return subprocess.check_output(["git", "rev-parse", "--short=7", "HEAD"], cwd=ROOT, text=True).strip()


def load_meta() -> dict:
    with META.open("r", encoding="utf-8") as f:
        return json.load(f)


def stable_version(meta: dict) -> str:
    return f"{meta['upstream_version']}-riff{meta['riff_iteration']}"


def prerelease_version(meta: dict, sha: str) -> str:
    return f"{stable_version(meta)}-alpha-{sha}"


def debian_version(meta: dict, sha: str, prerelease: bool) -> str:
    if prerelease:
        return f"{meta['upstream_version']}+riff{meta['riff_iteration']}~alpha.{sha}"
    return f"{meta['upstream_version']}+riff{meta['riff_iteration']}"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--format", choices=["stable", "prerelease", "debian-stable", "debian-prerelease", "json"], required=True)
    parser.add_argument("--sha", default=None)
    args = parser.parse_args()

    meta = load_meta()
    sha = args.sha or git_short_sha()

    values = {
        "stable": stable_version(meta),
        "prerelease": prerelease_version(meta, sha),
        "debian_stable": debian_version(meta, sha, False),
        "debian_prerelease": debian_version(meta, sha, True),
        "sha": sha,
        "package_name": meta["package_name"],
        "display_name": meta["display_name"],
        "debian_architecture": meta["debian_architecture"],
        "upstream_version": meta["upstream_version"],
        "riff_iteration": meta["riff_iteration"],
    }

    if args.format == "json":
        print(json.dumps(values))
        return

    mapping = {
        "stable": values["stable"],
        "prerelease": values["prerelease"],
        "debian-stable": values["debian_stable"],
        "debian-prerelease": values["debian_prerelease"],
    }
    print(mapping[args.format])


if __name__ == "__main__":
    main()
