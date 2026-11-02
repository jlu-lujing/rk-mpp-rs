#!/usr/bin/env python3
"""Write or verify a deterministic SHA-256 manifest for a decoder corpus.

    python3 scripts/check_corpus_manifest.py --write CORPUS
    python3 scripts/check_corpus_manifest.py --check CORPUS

The check validates container magic as well as the manifest. It deliberately
does not invoke a decoder, so it is safe to run before touching the board's VPU.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from collections import Counter
from pathlib import Path

MANIFEST_NAME = "corpus-manifest.json"
SCHEMA = 1

IVF_CODECS = {
    b"VP80": "vp8",
    b"VP90": "vp9",
    b"AV01": "av1",
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def classify(path: Path) -> tuple[str, str]:
    suffix = path.suffix.lower()
    with path.open("rb") as source:
        prefix = source.read(12)
    if suffix == ".ivf":
        if not prefix.startswith(b"DKIF") or len(prefix) < 12:
            raise ValueError("missing IVF DKIF header")
        fourcc = prefix[8:12]
        try:
            return IVF_CODECS[fourcc], "ivf"
        except KeyError as error:
            raise ValueError(f"unknown IVF FourCC {fourcc!r}") from error
    if suffix in (".264", ".h264"):
        if not (prefix.startswith(b"\x00\x00\x01") or prefix.startswith(b"\x00\x00\x00\x01")):
            raise ValueError("missing H.264 Annex-B start code")
        return "h264", "annex-b"
    if suffix in (".265", ".h265", ".hevc"):
        if not (prefix.startswith(b"\x00\x00\x01") or prefix.startswith(b"\x00\x00\x00\x01")):
            raise ValueError("missing H.265 Annex-B start code")
        return "h265", "annex-b"
    if suffix == ".m2v":
        if not prefix.startswith(b"\x00\x00\x01\xb3"):
            raise ValueError("missing MPEG-2 sequence header")
        return "mpeg2", "elementary"
    if suffix == ".m4v":
        if prefix[4:8] == b"ftyp":
            raise ValueError("MP4 container found where a raw MPEG-4 stream is required")
        if not prefix.startswith(b"\x00\x00\x01") or prefix[3] not in (0xB0, 0xB5):
            raise ValueError("missing MPEG-4 Visual elementary-stream header")
        return "mpeg4", "elementary"
    if suffix == ".263":
        if not prefix.startswith(b"\x00\x00\x80"):
            raise ValueError("missing H.263 picture start code")
        return "h263", "elementary"
    if suffix in (".jpg", ".jpeg"):
        if not prefix.startswith(b"\xff\xd8"):
            raise ValueError("missing JPEG SOI marker")
        return "jpeg", "jpeg"
    return "opaque", "opaque"


def entries(root: Path, manifest_path: Path) -> list[dict[str, object]]:
    result: list[dict[str, object]] = []
    errors: list[str] = []
    for path in sorted(root.rglob("*")):
        if path == manifest_path or not path.is_file() or path.is_symlink():
            continue
        relative = path.relative_to(root).as_posix()
        try:
            codec, container = classify(path)
        except ValueError as error:
            errors.append(f"{relative}: {error}")
            continue
        result.append(
            {
                "name": relative,
                "bytes": path.stat().st_size,
                "sha256": sha256(path),
                "codec": codec,
                "container": container,
            }
        )
    if errors:
        raise ValueError("\n".join(errors))
    return result


def write_manifest(root: Path, manifest_path: Path) -> list[dict[str, object]]:
    files = entries(root, manifest_path)
    manifest = {
        "schema": SCHEMA,
        "algorithm": "sha256",
        "kind": "decoder-corpus",
        "files": files,
    }
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    return files


def check_manifest(root: Path, manifest_path: Path) -> list[dict[str, object]]:
    manifest = json.loads(manifest_path.read_text())
    if manifest.get("schema") != SCHEMA or manifest.get("algorithm") != "sha256":
        raise ValueError(f"{manifest_path}: unsupported manifest schema")
    expected = manifest.get("files")
    if not isinstance(expected, list):
        raise ValueError(f"{manifest_path}: missing files list")
    actual = entries(root, manifest_path)
    expected_names = [entry.get("name") for entry in expected]
    actual_names = [entry["name"] for entry in actual]
    if expected_names != actual_names:
        missing = sorted(set(expected_names) - set(actual_names))
        extra = sorted(set(actual_names) - set(expected_names))
        raise ValueError(f"file set differs; missing={missing}, extra={extra}")
    for want, got in zip(expected, actual):
        if want != got:
            raise ValueError(
                f"{got['name']}: manifest mismatch (expected {want}, actual {got})"
            )
    return actual


def summary(files: list[dict[str, object]]) -> str:
    counts = Counter(str(entry["codec"]) for entry in files)
    codecs = ", ".join(f"{codec}={count}" for codec, count in sorted(counts.items()))
    total = sum(int(entry["bytes"]) for entry in files)
    return f"{len(files)} files, {total} bytes, {codecs}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--write", action="store_true")
    action.add_argument("--check", action="store_true")
    parser.add_argument("corpus", type=Path)
    parser.add_argument("--manifest", type=Path)
    args = parser.parse_args()

    root = args.corpus.resolve()
    if not root.is_dir():
        parser.error(f"not a directory: {root}")
    manifest_path = (args.manifest or root / MANIFEST_NAME).resolve()
    try:
        files = (
            write_manifest(root, manifest_path)
            if args.write
            else check_manifest(root, manifest_path)
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"FAIL {root}: {error}", file=sys.stderr)
        return 1
    print(f"PASS {root}: {summary(files)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
