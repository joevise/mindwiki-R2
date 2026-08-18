#!/usr/bin/env python3
"""Allocate a Source ID and copy one file or directory into a Source Bundle."""

import argparse
import json
import shutil
import sys
import uuid
from pathlib import Path


def parse_args():
    parser = argparse.ArgumentParser(
        description=(
            "Copy one file, or the contents of one directory, verbatim into a new "
            "self-contained sources/<uuid>/ Source Bundle."
        )
    )
    parser.add_argument(
        "source",
        type=Path,
        help="file or directory to copy",
    )
    parser.add_argument(
        "--wiki-root",
        required=True,
        type=Path,
        help="root of an initialized WikiOps Wiki",
    )
    return parser, parser.parse_args()


def allocate_bundle(source_store):
    while True:
        source_id = str(uuid.uuid4())
        bundle = source_store / source_id
        if not bundle.exists():
            return source_id, bundle


def find_escaping_symlink(source):
    if not source.is_dir():
        return None

    source_root = source.resolve()
    for path in source_root.rglob("*"):
        if not path.is_symlink():
            continue

        target = path.readlink()
        if target.is_absolute():
            return path
        try:
            (path.parent / target).resolve().relative_to(source_root)
        except ValueError:
            return path
    return None


def copy_source(source, bundle):
    try:
        if source.is_dir():
            shutil.copytree(source, bundle, symlinks=True, copy_function=shutil.copy2)
        else:
            bundle.mkdir()
            shutil.copy2(source, bundle / source.name)
    except BaseException:
        if bundle.exists():
            shutil.rmtree(bundle)
        raise


def main():
    parser, args = parse_args()
    source = args.source.absolute()
    wiki_root = args.wiki_root.absolute()
    source_store = wiki_root / "sources"

    if not source.exists():
        parser.error(f"source does not exist: {source}")
    if not source.is_file() and not source.is_dir():
        parser.error(f"source must be one file or directory: {source}")
    if not source_store.is_dir():
        parser.error(f"Wiki Source Store does not exist: {source_store}")

    escaping_symlink = find_escaping_symlink(source)
    if escaping_symlink is not None:
        parser.error(
            "source must be self-contained; symlink resolves outside its directory: "
            f"{escaping_symlink}"
        )

    if source.is_dir():
        try:
            source_store.resolve().relative_to(source.resolve())
        except ValueError:
            pass
        else:
            parser.error("source directory cannot contain the Wiki Source Store")

    source_id, bundle = allocate_bundle(source_store)
    copy_source(source, bundle)
    print(
        json.dumps(
            {
                "source_id": source_id,
                "bundle_path": str(bundle.resolve()),
            },
            sort_keys=True,
        )
    )


if __name__ == "__main__":
    try:
        main()
    except OSError as error:
        print(f"import failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
