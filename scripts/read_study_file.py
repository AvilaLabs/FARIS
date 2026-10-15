#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""Independent reader for FARIS study files (.faris), written from docs/STUDY_FILE.md alone (INT-001).

Standard library only; no FARIS code. It applies the reading rules of the specification (refusing what it
cannot interpret), checks the SHA-256 of every blob and embedded evidence-store blob, and prints the contents.

    read_study_file.py FILE               list entries and blobs (default)
    read_study_file.py FILE --summary     JSON summary two readers can compare
    read_study_file.py FILE --records     JSON list of the records the manifest holds
    read_study_file.py FILE --manifest    the manifest, as stored
    read_study_file.py FILE --blob SHA    the bytes of one verified blob, to standard output

Exit status: 0 verified, 1 refused (reason printed), 2 cannot decompress (zstd entries need Python 3.14+).
"""
import argparse
import hashlib
import json
import lzma
import re
import sys
import zipfile

MIMETYPE = b"application/vnd.avila-labs.faris-study"
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
SAFE_NAME = re.compile(r"[A-Za-z0-9._-]{1,128}\Z")
BLOB_ENTRY = re.compile(r"blobs/([0-9a-f]{64})\Z")
STORE_BLOB_ENTRY = re.compile(r"evidence-store/blobs/([0-9a-f]{2})/(\1[0-9a-f]{62})\.xz\Z")
MAX_MANIFEST = 64 << 20
MAX_BLOB = 1 << 30


class Refused(Exception):
    """The file breaks a reading rule; the message names the rule and the entry."""


class CannotDecompress(Exception):
    """The file is not refused, but this Python cannot read its compression."""


def safe_name(text) -> bool:
    return isinstance(text, str) and text not in (".", "..") and bool(SAFE_NAME.match(text))


def safe_path(text) -> bool:
    parts = text.split("/") if isinstance(text, str) else []
    return 1 <= len(parts) <= 4 and all(safe_name(p) for p in parts)


def digest(text) -> bool:
    return isinstance(text, str) and bool(DIGEST.match(text))


class Study:
    def __init__(self, path: str):
        try:
            self.zip = zipfile.ZipFile(path)
        except (zipfile.BadZipFile, OSError) as error:
            raise Refused(f"not a readable zip container: {error}")
        self.infos = self.zip.infolist()
        self.check_entries()
        self.manifest = self.read_manifest()
        self.blobs = self.check_blob_table()
        self.check_references()
        self.check_store_entries()

    def read(self, info: zipfile.ZipInfo, limit: int) -> bytes:
        if info.compress_type == 93 and not hasattr(zipfile, "ZIP_ZSTANDARD"):  # method 93 is zstd
            raise CannotDecompress(f"entry {info.filename} is zstd-compressed; Python {sys.version.split()[0]} "
                                   "cannot decompress it. Use Python 3.14 or newer.")
        try:
            with self.zip.open(info) as handle:
                data = handle.read(limit + 1)
        except (zipfile.BadZipFile, RuntimeError, NotImplementedError, OSError, EOFError) as error:
            raise Refused(f"entry {info.filename} cannot be read: {error}")
        if len(data) > limit:
            raise Refused(f"entry {info.filename} is longer than the {limit} bytes allowed")
        return data

    def check_entries(self):
        if not self.infos or self.infos[0].filename != "mimetype":
            raise Refused('the first entry must be "mimetype"')
        if self.infos[0].compress_type != zipfile.ZIP_STORED:
            raise Refused("the mimetype entry must be stored")
        if self.read(self.infos[0], len(MIMETYPE)) != MIMETYPE:
            raise Refused("this is not a FARIS study file (wrong mimetype)")
        self.by_name = {}
        for info in self.infos:
            if info.filename in self.by_name:
                raise Refused(f'duplicate entry "{info.filename}"')
            self.by_name[info.filename] = info
            name = info.filename
            if not (name in ("mimetype", "manifest.json", "preview.png", "evidence-store/store.json")
                    or BLOB_ENTRY.match(name) or STORE_BLOB_ENTRY.match(name)):
                raise Refused(f"unexpected entry name {name!r}")
            if info.flag_bits & 1:
                raise Refused(f"entry {name} is encrypted")
        if "manifest.json" not in self.by_name:
            raise Refused("the file has no manifest.json")

    def read_manifest(self) -> dict:
        try:
            manifest = json.loads(self.read(self.by_name["manifest.json"], MAX_MANIFEST))
        except ValueError as error:
            raise Refused(f"manifest.json is not JSON: {error}")
        fmt = manifest.get("format") if isinstance(manifest, dict) else None
        if not isinstance(fmt, str):
            raise Refused("the manifest names no format version")
        if not re.match(r"faris-study/1(\.[0-9]+)*\Z", fmt):
            raise Refused(f"unsupported format version {fmt!r}; this reader opens faris-study/1 and its minors")
        if not isinstance(manifest.get("blobs"), list):
            raise Refused("the manifest has no blob table")
        return manifest

    def check_blob_table(self) -> dict:
        blobs = {}
        entries = {m.group(1): name for name in self.by_name if (m := BLOB_ENTRY.match(name))}
        for record in self.manifest["blobs"]:
            sha = record.get("sha256") if isinstance(record, dict) else None
            if not digest(sha):
                raise Refused(f"blob table holds an invalid digest {sha!r}")
            if record.get("encoding") != "verbatim":
                raise Refused(f"blob {sha} has unknown encoding {record.get('encoding')!r}; refusing rather than skipping it")
            size = record.get("bytes")
            if not isinstance(size, int) or isinstance(size, bool) or not 0 <= size <= MAX_BLOB:
                raise Refused(f"blob {sha} declares an invalid size")
            if sha in blobs:
                raise Refused(f"blob {sha} is listed twice")
            if sha not in entries:
                raise Refused(f"blob {sha} is listed in the manifest but missing from the file")
            blobs[sha] = record
        for sha in entries:
            if sha not in blobs:
                raise Refused(f"blob {sha} is in the file but not listed in the manifest")
        return blobs

    def verify_blob(self, sha: str) -> bytes:
        data = self.read(self.by_name[f"blobs/{sha}"], self.blobs[sha]["bytes"])
        if len(data) != self.blobs[sha]["bytes"]:
            raise Refused(f"blob {sha} is {len(data)} bytes, the manifest says {self.blobs[sha]['bytes']}")
        if hashlib.sha256(data).hexdigest() != sha:
            raise Refused(f"blob {sha} does not match its SHA-256")
        return data

    def bundle_sets(self):
        arrangements = self.manifest.get("arrangements") or {}
        for role in ("port", "control"):
            if arrangements.get(role):
                yield role, arrangements[role].get("bundles", [])
        yield "sweep", self.manifest.get("sweep", [])

    def check_references(self):
        def known(sha):
            if not digest(sha) or sha not in self.blobs:
                raise Refused(f"the manifest refers to blob {sha!r}, which the file does not hold")
        arrangements = self.manifest.get("arrangements") or {}
        for role in ("port", "control"):
            record = arrangements.get(role) or {}
            for sha in ([record["scenario"]] if record.get("scenario") else []) + record.get("physics", []):
                known(sha)
        for group, bundles in self.bundle_sets():
            names = set()
            for bundle in bundles:
                if not safe_name(bundle.get("name")) or bundle["name"] in names:
                    raise Refused(f"bundle name {bundle.get('name')!r} in {group} is unsafe or repeated")
                names.add(bundle["name"])
                for sha in bundle["files"].values():
                    known(sha)
        if self.manifest.get("assumptions"):
            known(self.manifest["assumptions"])
        keys = set()
        for record in self.manifest.get("ensembles", []):
            known(record["blob"])
            key = json.dumps(record["key"], sort_keys=True)
            if key in keys:
                raise Refused("the manifest lists one history ensemble key twice")
            keys.add(key)
        layers = self.manifest.get("layers") or {}
        if layers.get("evidence") and layers.get("evidence_store"):
            raise Refused("the manifest records Core evidence both as archives and as store trees")
        archives = layers.get("evidence") or {}
        roles = set()
        for archive in archives.get("archives", []):
            role = (archive["arrangement"], archive["allocation"], archive["kind"])
            if (archive["arrangement"] not in ("port", "control") or not safe_name(archive["allocation"])
                    or not safe_path(archive["file_name"]) or not digest(archive["sha256"]) or role in roles):
                raise Refused(f"evidence record for {archive.get('file_name')!r} is invalid or repeated")
            roles.add(role)
            if archives["mode"] == "packed":
                known(archive["sha256"])
                if self.blobs[archive["sha256"]]["bytes"] != archive["bytes"]:
                    raise Refused(f"evidence archive {archive['file_name']} disagrees with its blob about size")

    def check_store_entries(self):
        embedded = [n for n in self.by_name if n.startswith("evidence-store/")]
        layer = (self.manifest.get("layers") or {}).get("evidence_store")
        if layer is None or layer["mode"] != "packed":
            if embedded:
                raise Refused("the file holds an evidence store, but its manifest does not record packed store evidence")
            return
        files = {}
        for tree in layer["trees"]:
            for f in tree["files"]:
                files[f["sha256"]] = f["bytes"]
        wanted = {f"evidence-store/blobs/{sha[:2]}/{sha}.xz" for sha in files} | {"evidence-store/store.json"}
        if set(embedded) != wanted:
            raise Refused("the file's evidence store holds different blobs than the trees the manifest lists")

    def verify_store(self):
        layer = (self.manifest.get("layers") or {}).get("evidence_store")
        if layer is None or layer["mode"] != "packed":
            return
        index = json.loads(self.read(self.by_name["evidence-store/store.json"], 64 << 20))
        listed = {t["name"]: sorted((f["path"], f["sha256"], f["bytes"]) for f in t["files"]) for t in index["trees"]}
        for tree in layer["trees"]:
            have = sorted((f["path"], f["sha256"], f["bytes"]) for f in tree["files"])
            if listed.get(tree["tree"]) != have:
                raise Refused(f"store tree {tree['tree']} differs from the listing in the manifest")
        sizes = {f["sha256"]: f["bytes"] for t in layer["trees"] for f in t["files"]}
        for sha, size in sizes.items():
            raw = self.read(self.by_name[f"evidence-store/blobs/{sha[:2]}/{sha}.xz"], size + 1024 + size // 1000)
            decoder = lzma.LZMADecompressor(lzma.FORMAT_XZ)
            try:
                data = decoder.decompress(raw, size + 1)
            except lzma.LZMAError as error:
                raise Refused(f"store blob {sha} is not valid xz: {error}")
            if not decoder.eof or decoder.unused_data or len(data) != size or hashlib.sha256(data).hexdigest() != sha:
                raise Refused(f"store blob {sha} does not match its recorded size and SHA-256")

    def verify(self) -> int:
        for sha in self.blobs:
            self.verify_blob(sha)
        self.verify_store()
        return len(self.blobs)

    def records(self) -> list:
        out = []
        for group, bundles in self.bundle_sets():
            out += [{"kind": "bundle", "group": group, **b} for b in bundles]
        out += [{"kind": "ensemble", **e} for e in self.manifest.get("ensembles", [])]
        layers = self.manifest.get("layers") or {}
        out += [{"kind": "evidence-archive", **a} for a in (layers.get("evidence") or {}).get("archives", [])]
        out += [{"kind": "evidence-store-tree", **t} for t in (layers.get("evidence_store") or {}).get("trees", [])]
        return out

    def evidence_mode(self) -> str:
        layers = self.manifest.get("layers") or {}
        for key, label in (("evidence", "archives"), ("evidence_store", "store")):
            if layers.get(key):
                return f"{label}-{layers[key]['mode']}"
        return "none"

    def summary(self) -> dict:
        return {"format": self.manifest["format"], "created_by": self.manifest.get("created_by", ""),
                "blob_count": len(self.blobs), "blob_sha256": sorted(self.blobs),
                "entries": sorted(self.by_name), "record_count": len(self.records()),
                "evidence_mode": self.evidence_mode(), "has_preview": "preview.png" in self.by_name}


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("file")
    what = parser.add_mutually_exclusive_group()
    for flag in ("summary", "records", "manifest"):
        what.add_argument("--" + flag, action="store_true")
    what.add_argument("--blob", metavar="SHA256")
    args = parser.parse_args(argv)
    try:
        study = Study(args.file)
        count = study.verify()
        if args.summary:
            print(json.dumps(study.summary(), indent=2))
        elif args.records:
            print(json.dumps(study.records(), indent=2))
        elif args.manifest:
            print(json.dumps(study.manifest, indent=2))
        elif args.blob:
            if args.blob not in study.blobs:
                raise Refused(f"the file holds no blob {args.blob}")
            sys.stdout.buffer.write(study.verify_blob(args.blob))
        else:
            print(f"{args.file}: verified, format {study.manifest['format']}, {count} blobs, "
                  f"evidence {study.evidence_mode()}, {len(study.records())} records")
            for sha, record in study.blobs.items():
                print(f"  blob {sha}  {record['bytes']:>10} bytes  {record['media_type']}")
    except CannotDecompress as error:
        print(f"CANNOT DECOMPRESS: {error}", file=sys.stderr)
        return 2
    except Refused as error:
        print(f"REFUSED: {error}", file=sys.stderr)
        return 1
    except (KeyError, TypeError, AttributeError) as error:
        print(f"REFUSED: the manifest does not have the shape the specification requires ({error!r})", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
