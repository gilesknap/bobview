"""Make switcher.json, the list of published docs versions, for gh-pages.

Keeps the versions already published (from the gh-pages branch, if any)
whose branch or tag still exists, adds the one being published, and orders
them main first, then tags newest first.
"""

import json
import re
import subprocess
from argparse import ArgumentParser
from pathlib import Path


def published(path: Path) -> list[str]:
    try:
        text = subprocess.check_output(
            ["git", "show", f"origin/gh-pages:{path.name}"],
            text=True,
            stderr=subprocess.DEVNULL,
        )
    except subprocess.CalledProcessError:
        return []
    return [entry["version"] for entry in json.loads(text)]


def refs() -> set[str]:
    out = subprocess.check_output(
        ["git", "ls-remote", "--heads", "--tags", "origin"], text=True
    )
    return {
        re.sub(r"^refs/(heads|tags)/", "", line.split()[1]).removesuffix("^{}")
        for line in out.splitlines()
    }


def order(version: str) -> tuple:
    if version == "main":
        return (0,)
    numbers = [int(n) for n in re.findall(r"\d+", version)]
    return (1, [-n for n in numbers], version)


def main():
    parser = ArgumentParser(description=__doc__)
    parser.add_argument("--add", required=True, help="version being published")
    parser.add_argument("repository", help="owner/name of the GitHub repository")
    parser.add_argument("output", type=Path, help="switcher.json to write")
    args = parser.parse_args()

    names = refs()
    versions = {v for v in published(args.output) if v in names} | {args.add}
    owner, name = args.repository.split("/")
    switcher = [
        {"version": v, "url": f"https://{owner}.github.io/{name}/{v}/"}
        for v in sorted(versions, key=order)
    ]
    args.output.write_text(json.dumps(switcher, indent=2) + "\n")
    print(json.dumps(switcher, indent=2))


if __name__ == "__main__":
    main()
