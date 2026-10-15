<!-- kind: how-to -->
# Release notes

Requirements QA-074, PLAT-034 and LEG-052. The notes of a release are generated, not written by hand, so that they
cannot leave out the parts that matter:

```sh
python3 scripts/release_notes.py --version X.Y.Z --release-record release-record.json \
    --gate gate-output.txt --out notes.md
```

The inputs are the version's section of `CHANGELOG.md`, the release record from `python3 -m validation report`, the
archived output of `scripts/release_gate.py`, and the defect log. The generator refuses to write notes when any of
them is missing. Attach the notes, `gate-output.txt`, `release-record.json`, the V&V report and the two SBOM files
(`scripts/generate_sbom.py`) to the release.

## What the notes contain

1. **Changes that can alter a published number.** Listed first because they matter most to a reader who has already
   published a figure. The list is the changelog's `### Numerical changes` heading for the version. Each item names
   the result that moves, the cases it was measured on and the size of the change, or says "not measured".
2. **Security fixes**, from the changelog's `### Security` heading, with the advisory ID and the CVE if there is one
   (`docs/SECURITY_PROCESS.md`).
3. **Verification and validation status**: the report hash and every condition of the release gate with PASS or FAIL.
4. **Gate result.**
5. **Known issues**: the counts, and `docs/KNOWN_ISSUES.md`.
6. **Migration guides** that the changelog links (`docs/MIGRATION_GUIDE_TEMPLATE.md`).
7. **Export-control review status** of the code, the bundled data and the generated files: the line is always
   present, and says NOT REVIEWED until `docs/EXPORT_CONTROL.md` has been reviewed.
8. **Software bill of materials** files.

## What counts as a change that can alter a number

Any change that can make the same inputs give a different published value: a physics model or its constants, a
nuclear-data library or its version, a default, a tally or its normalisation, a random-number stream or seed rule,
a unit conversion, a rounding rule, an ordering that changes a floating-point sum, a dependency upgrade that
changes numerical results, and a change to what a verdict compares against. If you are not sure, list it.

The finding of such changes is meant to come from a golden-study comparison between the last release and the
candidate (requirement PRV-027). That comparison is not built yet, so for now the list is the author's account in the
changelog, and the generated notes say so when the list is empty.
