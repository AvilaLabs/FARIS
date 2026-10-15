# Security policy

FARIS is research screening software. Results are not a licensing, safety or design basis. This page is about
vulnerabilities in the software itself: a way to make FARIS run code it should not, read or write files it should
not, leak a sign-in token, accept a tampered file as intact, or show a verdict the evidence does not support.

## Reporting a vulnerability

Report privately. Do not open a public issue or pull request for a vulnerability.

1. Go to <https://github.com/AvilaLabs/FARIS/security/advisories/new> (the **Security** tab, then **Report a
   vulnerability**). This opens a private advisory that only you and the maintainers can see.
2. Say which version or commit you tested, what you did, what happened, and what you expected. A file that
   triggers the problem helps; do not include customer data or export-controlled material.

If GitHub's private reporting is unavailable to you, open a public issue that says only "I have a security report,
please give me a private channel" and nothing else about the problem.

## What to expect

These targets are provisional until the project has run a full release cycle with them (requirement SEC-073).

| Step | Target |
| --- | --- |
| We acknowledge your report | within 3 business days |
| We confirm or reject it, with our reasoning | within 10 business days |
| Fix released, critical severity | within 7 days of confirmation |
| Fix released, high severity | within 30 days |
| Fix released, medium severity | within 90 days |
| Low severity | the next regular release |

FARIS is maintained by one person, so these are goals and not a contract. If a target will be missed, we tell you
and say why. We ask that you keep the report private until a fix is out or 90 days have passed, whichever comes
first, and we will agree a date with you if a fix needs longer.

## Supported versions

Only the newest release receives security fixes. FARIS is before 1.0; a fix may arrive with a file-format or
command-line change, which the changelog lists under **Changed**.

## What happens after a fix

See the advisory process in [docs/SECURITY_PROCESS.md](docs/SECURITY_PROCESS.md): a published GitHub security
advisory, a CVE where one applies, a line in the release notes, and a regression test.

## Scope

In scope: the `faris` command line, the desktop app, the browser version's code in this repository, the file
readers (`.faris` studies, scenario and design files, evidence archives, transport bundles), the adapters that start
other programs, and the optional Avila Labs sign-in client. The threat model is in
[docs/THREAT_MODEL.md](docs/THREAT_MODEL.md).

Out of scope: vulnerabilities in OpenMC, ACTINV, Avila Core, CAD tools or nuclear-data providers (report those to
their projects; tell us if FARIS makes one worse), the account service itself (report to Avila Labs through the
same channel and say so), and findings that need an attacker who already controls your machine.

Good-faith research that stays inside this policy will not be met with legal action by the project.
