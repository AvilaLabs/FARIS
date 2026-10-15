<!-- kind: how-to -->
# Security advisories and CVEs

This page is the maintainers' process for a reported or discovered vulnerability (requirements SEC-073 and
SEC-074). The reporter-facing policy is `SECURITY.md` at the repository root.

## Steps

1. **Receive.** A report arrives as a private GitHub security advisory in draft. Acknowledge it within 3 business
   days. Record it in the advisory log below with its date, the reporter's wish for credit, and a severity.
2. **Severity.** Use CVSS v3.1 or v4.0 and write the vector in the advisory. Critical and high follow the fix
   targets in `SECURITY.md`. A vulnerability that can make FARIS show a false verdict or accept a tampered result
   as intact (SEC-007, SEC-013) is at least high, because it defeats the purpose of the tool.
3. **Fix in private.** Use the advisory's private fork. The fix carries a regression test (QA-085); if the flaw is
   in a parser, the input becomes a seed in the fuzz corpus (SEC-022, `docs/FUZZING.md`).
4. **Publish.** Release the fixed version, then publish the advisory in the same hour. Request a CVE through
   GitHub's advisory form (GitHub is a CVE numbering authority) when the flaw is in released code that other people
   run. No CVE is requested for a flaw that never reached a release.
5. **Tell users.** The release notes (`docs/RELEASE_NOTES_TEMPLATE.md`) list the advisory under **Security**, with
   the affected versions, the fixed version, and what a user should do. The changelog repeats it. The defect log
   (`docs/defects/faris-defects.json`) gets an entry with the regression test.
6. **Look back.** Within 30 days, write down in the advisory log why the flaw was not caught earlier and whether a
   test, a lint or a fuzz target would have found it.

## RustSec advisories in dependencies

CI fails on any advisory against `Cargo.lock` (`scripts/check_audit.py`). A finding that cannot be fixed at once is
waived in `scripts/audit_allowlist.json` with an owner, a reason, and an expiry within 90 days; an expired waiver
fails CI. Dependencies on the sign-in path (SEC-039) are fixed within 7 days for critical and 30 days for high.

## Advisory log

| Date | Advisory | Severity | Affected | Fixed in | CVE |
| --- | --- | --- | --- | --- | --- |
| (none yet) | | | | | |

An empty log means no vulnerability report has been confirmed. It does not mean none exists; no independent
security review has been done yet (SEC-077).
