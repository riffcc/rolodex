#!/usr/bin/env python3
import argparse
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CARGO = ROOT / "codex-rs" / "Cargo.toml"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("version")
    args = parser.parse_args()

    text = CARGO.read_text(encoding="utf-8")
    new_text, count = re.subn(
        r'(?ms)(\[workspace\.package\]\n(?:.*?\n)*?version = ")([^"]+)(")',
        rf'\g<1>{args.version}\3',
        text,
        count=1,
    )
    if count != 1:
        raise SystemExit("failed to update workspace.package version")
    CARGO.write_text(new_text, encoding="utf-8")


if __name__ == "__main__":
    main()
