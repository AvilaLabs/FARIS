<!-- kind: reference -->
# Threat model

Requirement SEC-076. This is a STRIDE model of FARIS for five areas: files, adapters, sign-in, updates and data.
It lists what an attacker could do, and for each threat either the requirement that closes it or an accepted risk
with its reason. Whether a requirement is met today is in `docs/requirements/SCORECARD.md`; this page does not
repeat it, so a mitigation named here is a commitment, not a claim that it is finished.

Review this page within 30 days of every major release, and whenever a new input format, adapter or network
feature is added. Last reviewed: 2026-10-15 (first version, by the maintainer; no independent review, SEC-077).

## What FARIS is, for this model

A native desktop app and command line that read scenario, study and design files, start other programs
(OpenMC, ACTINV, Avila Core, a CAD Python) to compute, and write results with hashes. It runs with the user's own
privileges on the user's own machine. It has no server component of its own. An optional sign-in talks to the
Avila Labs account service. The users are engineers who open files that colleagues, vendors or the internet send
them. The assets worth protecting are: the integrity of a result and its verdict, the user's other files, the
sign-in token, and the user's confidential designs.

Trust boundaries: (1) file bytes from outside, (2) the output of a program FARIS started, (3) the network,
(4) the release download.

## Files

| ID | STRIDE | Threat | Closed by |
| --- | --- | --- | --- |
| F1 | Tampering | A study or bundle is edited so a result looks intact. | SEC-010, SEC-011, SEC-013 |
| F2 | Spoofing | A study claims a signer or a verdict it did not earn. | SEC-007, SEC-012 |
| F3 | Elevation of privilege | A file carries code or a command that FARIS runs. | SEC-001, SEC-005 |
| F4 | Tampering | A path in an archive or study writes outside the workspace (path traversal). | SEC-002 |
| F5 | Denial of service | A compressed or deeply nested file exhausts memory or time. | SEC-003, SEC-006 |
| F6 | Tampering | A parser guesses at ambiguous input (duplicate keys, unknown encoding) and reads different data than another tool. | SEC-004, SEC-020 |
| F7 | Denial of service | A parser crashes or hangs on malformed bytes. | SEC-021, SEC-022, SEC-023 |
| F8 | Information disclosure | A file marked restricted is shared by accident. | SEC-070, SEC-071 |
| F9 | Repudiation | A user cannot show which inputs produced a result. | Accepted risk AR-1 |

## Adapters (programs FARIS starts)

| ID | STRIDE | Threat | Closed by |
| --- | --- | --- | --- |
| A1 | Elevation of privilege | An argument or path is interpreted by a shell. | SEC-060 |
| A2 | Elevation of privilege | A planted executable on the path is run in place of the real tool. | SEC-060, SEC-061 |
| A3 | Information disclosure | The tool inherits secrets from the environment, or reaches the network. | SEC-061, SEC-062 |
| A4 | Tampering | Output of the tool is malformed or hostile and reaches the parsers. | SEC-063, SEC-020 |
| A5 | Tampering | A tool returns a plausible but wrong answer. | Accepted risk AR-2 |
| A6 | Denial of service | A tool runs forever or fills the disk. | SEC-061 |
| A7 | Elevation of privilege | A future plugin mechanism runs untrusted code. | SEC-064, SEC-065 |

## Sign-in

The sign-in is optional and sends no study content (`docs/guide/account.md`).

| ID | STRIDE | Threat | Closed by |
| --- | --- | --- | --- |
| S1 | Spoofing | A fake service receives the token. | SEC-042 |
| S2 | Information disclosure | The token is read from disk by another user or leaks into logs. | SEC-041, SEC-043 |
| S3 | Information disclosure | Study content, paths or results are sent to the service. | SEC-047, SEC-050, SEC-052 |
| S4 | Repudiation | Sign-out leaves a live token on the service. | SEC-044 |
| S5 | Denial of service | A slow or hostile service freezes or alters the application. | SEC-046, SEC-045 |
| S6 | Tampering | A revoked token leaves the app in a broken state. | SEC-048 |
| S7 | Elevation of privilege | A flaw in the TLS or HTTP client stack. | SEC-039, SEC-031 |
| S8 | Information disclosure | A user is tracked without knowing. | SEC-050, SEC-051 |
| S9 | Information disclosure | The token is readable by other programs of the same user. | Accepted risk AR-3 |

## Updates and distribution

FARIS has no updater. A new version is a download the user chooses (`docs/guide/install.md`).

| ID | STRIDE | Threat | Closed by |
| --- | --- | --- | --- |
| U1 | Tampering | A release archive is replaced on the download server. | SEC-036, SEC-037 |
| U2 | Tampering | The build is altered between source and binary. | SEC-035, SEC-037, SEC-032 |
| U3 | Tampering | A dependency or CI action is replaced with a malicious one. | SEC-032, SEC-033, SEC-034, SEC-031 |
| U4 | Spoofing | A future update check is answered by a fake server, or rolled back to an old version. | SEC-054 |
| U5 | Information disclosure | An update check reports the machine or the user. | SEC-050, SEC-054 |
| U6 | Repudiation | A user cannot tell what a release contains. | SEC-030 |
| U7 | Spoofing | The programs are not signed by the operating system's mechanism, so a user cannot distinguish them from a copy. | Accepted risk AR-4 |

## Data (nuclear data, geometry, results)

| ID | STRIDE | Threat | Closed by |
| --- | --- | --- | --- |
| D1 | Tampering | A nuclear-data library file is swapped for a modified one. | SEC-010, SEC-011 |
| D2 | Tampering | Geometry or mesh from outside has bad topology that corrupts a calculation. | SEC-006 |
| D3 | Information disclosure | FARIS bundles data that is access-controlled or may not be redistributed. | SEC-072 |
| D4 | Tampering | A result is shown with a verdict the evidence does not support. | SEC-007, SEC-013 |
| D5 | Information disclosure | Customer designs end up in the repository or in a release. | `AGENTS.md` rule that customer cases stay out of Git; `scripts/check_repository_size.py` |

## Accepted risks

- **AR-1.** A hash proves identity, not that the inputs were right or who chose them. FARIS records what was
  used; it does not authenticate the person. Accepted because FARIS is a single-user tool; signed studies
  (SEC-012) are the planned answer for shared work.
- **AR-2.** A correct-looking wrong answer from OpenMC, ACTINV or another tool cannot be detected by FARIS from the
  output alone. Accepted for tools FARIS does not control; the V&V programme (`docs/VALIDATION.md`) and, for the
  Avila Labs tools, their own verification stand in for it. A result is never labelled better than its evidence.
- **AR-3.** Until the token moves to the operating system's credential store (SEC-041), it sits in an owner-only
  file, which another program running as the same user can read. Accepted because such a program can also read
  every other file of that user, and the token is useful only to the account service, which holds no study content.
- **AR-4.** The 0.1.1 and 0.2.0 programs are not code-signed (`docs/guide/install.md`). A user verifies the SHA-256
  of the download instead. Accepted until signing certificates are bought; SEC-036 tracks the signed releases.

## Reviews

| Date | By | Scope | Result |
| --- | --- | --- | --- |
| 2026-10-15 | Maintainer | First version | Written; no independent review |
