<!-- kind: how-to -->
# Deprecation policy

Requirement DOC-043. An item that users may depend on is not removed without notice. Items are command-line
commands and flags, file formats and fields, library functions, settings and output columns.

## The rule

1. **Announce.** The release that deprecates an item says so in the changelog under **Deprecated**, names the
   replacement, and states the release in which the item will be removed.
2. **Warn.** From that release the item still works and prints a warning that names the replacement:
   `warning: --old is deprecated and will be removed in 0.5.0; use --new instead.` A file field that is deprecated
   is read, and a note appears in the validation messages.
3. **Wait.** The item stays for at least two minor releases after the announcing release. Before 1.0, a breaking
   change that does not follow this rule needs a written reason in the changelog under **Changed**, and a
   migration guide.
4. **Remove.** The removal release lists the item under **Removed** and links the migration guide
   (`docs/MIGRATION_GUIDE_TEMPLATE.md` is the template).

Security fixes are the exception: an unsafe item may be removed at once, with the advisory explaining why
(`docs/SECURITY_PROCESS.md`).

## The register

Every deprecated item is listed here while it is deprecated. `scripts/check_deprecations.py` fails when an item's
removal release is earlier than the announcing release plus two minor versions, when a removal release is passed
but the item is still listed, or when an entry lacks its replacement.

<!-- deprecations:begin -->
| Item | Replacement | Announced in | Removal in |
| --- | --- | --- | --- |
<!-- deprecations:end -->

An empty register means nothing is deprecated at present.
