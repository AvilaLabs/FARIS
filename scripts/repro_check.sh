#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Reproducible-build check for the Linux `faris` command-line binary (requirement QA-063).
#
#   scripts/repro_check.sh [--cargo CMD] [--work DIR] [--evidence FILE]
#       Builds `faris` twice with the same recipe, once from this checkout and once from a fresh checkout of
#       the same commit at another path, into two target directories under DIR, and compares SHA-256 hashes.
#       Exit 0 when they are equal, 1 when not, 2 on a setup problem.
#   scripts/repro_check.sh --env [--target DIR]
#       Prints the export lines of the recipe for this checkout: eval "$(scripts/repro_check.sh --env)".
#   scripts/repro_check.sh --compare BIN_A BIN_B [--evidence FILE]
#       Compares two binaries you built yourself under the recipe.
#
# The recipe: `cargo build --release --locked -p faris-cli` with the checkout, cargo home, rustup home and
# target directory remapped to fixed names in every path rustc embeds (panic locations, debug strings), and
# SOURCE_DATE_EPOCH set to the commit time for any tool that reads it. Nothing else varies: there is no build
# script or timestamp in the command-line crates, and the release profile (codegen-units = 1, thin LTO) is
# deterministic. Only the Linux binary of `faris` is covered; `faris-app` (graphics libraries, the macOS and
# Windows builds, signed archives) is built on a best-effort basis and is not claimed reproducible.
#
# The evidence file is what release_gate.py reads for --repro:
#   {"commit": SHA, "builds": [{"artifact": "faris", "sha256_a": ..., "sha256_b": ...}]}
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mode=build
cargo_cmd=cargo
work=""
evidence=""
target=""
bins=()

while [ $# -gt 0 ]; do
    case "$1" in
        --env) mode=env ;;
        --compare) mode=compare ;;
        --cargo) cargo_cmd="$2"; shift ;;
        --work) work="$2"; shift ;;
        --evidence) evidence="$2"; shift ;;
        --target) target="$2"; shift ;;
        -h|--help) sed -n '2,24p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) bins+=("$1") ;;
    esac
    shift
done

# The environment of the recipe for the checkout `$1` building into target directory `$2`.
recipe() {
    local tree="$1" target_dir="$2"
    local cargo_home="${CARGO_HOME:-$HOME/.cargo}" rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
    local flags="--remap-path-prefix=$tree=/build/faris --remap-path-prefix=$cargo_home=/build/cargo"
    flags="$flags --remap-path-prefix=$rustup_home=/build/rustup"
    [ -n "$target_dir" ] && flags="$flags --remap-path-prefix=$target_dir=/build/target"
    echo "export RUSTFLAGS='$flags'"
    echo "export SOURCE_DATE_EPOCH=$(git -C "$tree" log -1 --format=%ct)"
    echo "export CARGO_INCREMENTAL=0"
    echo "export TZ=UTC LC_ALL=C"
}

commit="$(git -C "$here" rev-parse HEAD)"

write_evidence() {
    local a="$1" b="$2"
    local ha hb
    ha="$(sha256sum "$a" | cut -d' ' -f1)"
    hb="$(sha256sum "$b" | cut -d' ' -f1)"
    echo "faris build A: $ha  $a"
    echo "faris build B: $hb  $b"
    if [ -n "$evidence" ]; then
        printf '{"commit": "%s", "builds": [{"artifact": "faris", "sha256_a": "%s", "sha256_b": "%s"}]}\n' \
            "$commit" "$ha" "$hb" > "$evidence"
    fi
    if [ "$ha" = "$hb" ]; then
        echo "reproducible: the two builds are bit for bit identical"
    else
        echo "NOT reproducible: the two builds differ (compare with diffoscope, or cmp -l and strings)" >&2
        return 1
    fi
}

case "$mode" in
    env)
        recipe "$here" "$target"
        ;;
    compare)
        [ ${#bins[@]} -eq 2 ] || { echo "usage: $0 --compare BIN_A BIN_B [--evidence FILE]" >&2; exit 2; }
        write_evidence "${bins[0]}" "${bins[1]}"
        ;;
    build)
        work="${work:-$(mktemp -d)}"
        mkdir -p "$work"
        work="$(cd "$work" && pwd)"
        if [ -n "$(git -C "$here" status --porcelain --untracked-files=no)" ]; then
            echo "repro_check: the checkout has uncommitted changes; the second build is of the commit $commit" >&2
            exit 2
        fi
        git -C "$here" worktree add --detach "$work/checkout-b" "$commit" >/dev/null
        trap 'git -C "$here" worktree remove --force "$work/checkout-b" >/dev/null 2>&1 || true' EXIT
        for side in a b; do
            tree="$here"
            [ "$side" = b ] && tree="$work/checkout-b"
            target_dir="$work/target-$side"
            (
                cd "$tree"
                eval "$("$tree/scripts/repro_check.sh" --env --target "$target_dir")"
                CARGO_TARGET_DIR="$target_dir" $cargo_cmd build --release --locked -p faris-cli
            )
        done
        write_evidence "$work/target-a/release/faris" "$work/target-b/release/faris"
        ;;
esac
