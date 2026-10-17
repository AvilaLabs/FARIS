#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Recover a .faris study file with standard shell tools only (requirement INT-002): unzip and zipinfo for the entry
# table, dd to cut each entry out of the container, zstd to decompress it, sha256sum and jq to check it. No FARIS
# code and no Python. Info-ZIP unzip cannot decompress zip method 93 (zstd) itself, so the compressed bytes are cut
# out by the offsets zipinfo prints and handed to zstd; a stored entry is copied as it is.
#
#   scripts/shell_extract_study.sh FILE.faris OUTDIR
#
# Checks: the first entry is `mimetype`, stored, holding exactly the type string, which therefore sits at byte
# offset 30 + 8 of the file; every other entry is stored or zstd; manifest.json is JSON of format faris-study/1;
# every `blobs/<sha256>` entry hashes to its name and is listed in the manifest's blob table with the same size.
# Writes OUTDIR/mimetype, OUTDIR/manifest.json and OUTDIR/blobs/<sha256>. Exit 0 when all hold, 1 and a reason if not.
set -euo pipefail

TYPE="application/vnd.avila-labs.faris-study"
fail() { echo "shell extract: $*" >&2; exit 1; }

[ $# -eq 2 ] || fail "usage: $0 FILE.faris OUTDIR"
file=$1
out=$2
[ -f "$file" ] || fail "no such file: $file"
for tool in unzip zipinfo zstd dd od jq sha256sum awk; do
    command -v "$tool" > /dev/null || fail "needs $tool on the path"
done
mkdir -p "$out/blobs"

# 1. mimetype is the first entry, stored, and the type string sits right after its 30-byte header and 8-byte name.
first=$(unzip -Z1 "$file" | head -n 1)
[ "$first" = "mimetype" ] || fail "first entry is '$first', not mimetype"
[ "$(head -c $((30 + 8 + ${#TYPE}))  "$file" | tail -c ${#TYPE})" = "$TYPE" ] || fail "the type string is not at byte offset 38"
[ "$(unzip -p "$file" mimetype)" = "$TYPE" ] || fail "mimetype does not hold the type string"

# 2. The entry table: name, method, local header offset, compressed size, one tab-separated line per entry.
table=$(zipinfo -v "$file" | awk '
    /^Central directory entry #/ { if (name != "") print name "\t" method "\t" offset "\t" size; name = ""; want = 1; next }
    want && NF && name == "" && $0 !~ /:/ && $0 !~ /^-+$/ { sub(/^ +/, ""); name = $0; next }
    /offset of local header from start of archive:/ { offset = $NF == "bytes" ? $(NF - 1) : $NF }
    /compression method:/ { method = ($0 ~ /stored/) ? "stored" : ($0 ~ /\(93\)/) ? "zstd" : "other" }
    /^  compressed size:/ { size = $3 }
    END { if (name != "") print name "\t" method "\t" offset "\t" size }')
[ -n "$table" ] || fail "no entries found"
[ "$(printf '%s\n' "$table" | awk -F'\t' '{print $1}' | sort | uniq -d | wc -l)" -eq 0 ] || fail "an entry name is repeated"

# Local header field at byte position $2 of length 2, little endian.
le16() { od -An -tu1 -j "$2" -N 2 "$1" | awk '{print $1 + 256 * $2}'; }

extract() { # name method offset size dest
    local name=$1 method=$2 offset=$3 size=$4 dest=$5
    local nlen elen start
    nlen=$(le16 "$file" $((offset + 26)))
    elen=$(le16 "$file" $((offset + 28)))
    start=$((offset + 30 + nlen + elen))
    case "$method" in
        stored) dd if="$file" of="$dest" iflag=skip_bytes,count_bytes skip="$start" count="$size" status=none ;;
        zstd) dd if="$file" iflag=skip_bytes,count_bytes skip="$start" count="$size" status=none | zstd -dcq > "$dest" \
            || fail "$name: zstd could not decompress the entry" ;;
        *) fail "$name: compression method is neither stored nor zstd" ;;
    esac
}

[ "$(printf '%s\n' "$table" | head -n 1 | cut -f2)" = "stored" ] || fail "mimetype is not stored"
blobs=0
while IFS=$'\t' read -r name method offset size; do
    case "$name" in
        mimetype) extract "$name" "$method" "$offset" "$size" "$out/mimetype" ;;
        manifest.json) extract "$name" "$method" "$offset" "$size" "$out/manifest.json" ;;
        blobs/*)
            digest=${name#blobs/}
            [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || fail "$name: not a lowercase SHA-256 name"
            extract "$name" "$method" "$offset" "$size" "$out/blobs/$digest"
            [ "$(sha256sum "$out/blobs/$digest" | cut -d' ' -f1)" = "$digest" ] || fail "$name: the bytes do not hash to the name"
            blobs=$((blobs + 1)) ;;
        preview.png | evidence-store/*) ;; # optional entries, not in the blob table
        *) fail "$name: not an entry a study file has" ;;
    esac
done <<< "$table"

# 3. The manifest is JSON of the right format, and its blob table and the blob entries agree.
[ "$(cat "$out/mimetype")" = "$TYPE" ] || fail "extracted mimetype differs"
format=$(jq -r '.format' "$out/manifest.json") || fail "manifest.json is not JSON"
case "$format" in faris-study/1 | faris-study/1.*) ;; *) fail "manifest format is '$format'" ;; esac
listed=$(jq -r '(.blobs // [])[] | "\(.sha256) \(.bytes)"' "$out/manifest.json")
count=0
while read -r digest bytes; do
    [ -n "$digest" ] || continue
    [ -f "$out/blobs/$digest" ] || fail "blob $digest is in the manifest but not in the file"
    [ "$(wc -c < "$out/blobs/$digest")" -eq "$bytes" ] || fail "blob $digest has a different size from the manifest"
    count=$((count + 1))
done <<< "$listed"
[ "$count" -eq "$blobs" ] || fail "the manifest lists $count blobs, the file holds $blobs"
echo "shell extract: $file: manifest and $blobs blob(s) recovered"
