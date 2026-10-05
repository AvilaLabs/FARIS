# Reliability

How FARIS stays up, never loses work, reads damaged files safely and degrades honestly when a part is missing. Numbers for memory admission, cancellation and soak timing live in the performance file (PERF-041, PERF-024, PERF-042, PERF-043); this file refers to them and adds the failure behaviour. Crash and loss targets are measured by injected faults and soak runs first, and by user reports only second, because users who crash often decline to report. See the [index](README.md#reference-hardware-and-models) for RL, RW and the reference models RM-S, RM-M and RM-L.

## Stability

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| REL-001 | FARIS shall achieve a crash-free session rate on release candidates. | At least 99.9 % of scripted and soak sessions end without a crash or hang (provisional: reference is mobile practice; confirm before 1.0). | Soak and scripted-session harness counting sessions, not users. | Android vitals user-perceived crash bad-behaviour threshold 1.09 % [V](https://developer.android.com/topic/performance/vitals); Crashlytics 99.9–99.95 % "excellent" [U] | F1 | Unmeasured |
| REL-002 | FARIS shall report mean time between failures from soak data. | At least 200 hours of scripted use per crash (provisional: no reference; confirm before 1.0). | Soak statistics per release. | ISO 25023 MTBF measure [U] | F2 | No |
| REL-003 | A panic in a worker, kernel or adapter thread shall not take down the application. | 100 % of injected panics leave the UI alive, mark the job failed with a stable error code and keep the study intact. | Fault-injection test on every job type. | R5 RL-10 | F1 | Partial: jobs run off the UI thread; panic isolation not injected |
| REL-004 | A panic on the UI thread shall write a local crash record, attempt an emergency autosave and exit without corrupting any file. | 100 % of injected UI panics produce a record and a recoverable autosave; 0 damaged study files. | Fault injection. | Mozilla crash reporter practice [U] | F1 | No |
| REL-005 | Production code shall not panic on user data. | 0 unwrap, expect or unchecked indexing on data-reading paths, enforced by clippy deny lints. | CI lint over reader, parser and import code. | R5 TP-20 | F1 | No: 473 unwrap or expect occurrences in crates/*/src (tests included, not triaged by path) |
| REL-006 | An external tool that crashes or hangs shall be contained: detected, killed and reported. | Watchdog default 60 s without progress; kill and report within 2 s; 0 orphan processes (see PERF-024). | Kill and hang test for every adapter. | R5 RL-11 | F1 | Partial: job runner uses process groups, resource limits and cancel; no hang watchdog |
| REL-007 | Every user-facing error shall have a stable code, a plain explanation and a next step. | 100 % of errors listed in a catalogue; none shows a raw panic or debug string. | Error catalogue test plus UI automation of injected errors. | R5 RL-17; house rule: Unknown must explain itself | F1 | Partial: study-file errors name the cause (for example unsupported version); no catalogue |
| REL-008 | FARIS shall hold an error budget per release: crashes and data-loss defects found in the field consume it and block feature work when it is spent. | Budget set at 0.1 % of sessions; a release is blocked when the last 30 days exceed it. | Release gate script on crash and soak counts. | Site-reliability practice [U] | F4 | No |

## Autosave, recovery and atomic saves

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| REL-010 | Saving shall be atomic: a study file is never left partly written, whatever interrupts the save. | 0 damaged files in 10,000 randomised kill-during-save trials, including power-loss emulation; previous file intact on every failure. | Fault injection with kill -9 and a filesystem that drops unsynced writes. | SQLite atomic commit [U]; R5 RL-03 | F1 | Partial: writer uses a temporary file in the destination directory, fsync, then rename, and a test checks no temporary files remain; no kill campaign; directory fsync not confirmed |
| REL-011 | FARIS shall autosave recoverable state so that at most 30 seconds of edits are lost after a crash or kill -9. | Loss at most 30 s of edits in 100 % of 1,000 injected kills at random times (provisional: Blender default is 2 min [U]; confirm before F1 gate). | Kill -9 test during scripted editing. | R6 reliability row; Blender autosave 2 min [U] | F1 | No: no autosave; a study can only be saved by the user |
| REL-012 | Autosave shall never overwrite the user's saved file and shall itself be written atomically. | 0 changes to the saved file by autosave; 0 damaged autosave files in 10,000 kill trials. | Fault injection. | R5 trap 15 | F1 | No |
| REL-013 | After a crash, FARIS shall offer to recover on next start and restore the study, view and unsaved edits. | Recovery offered within 5 s of start in 100 % of injected crashes; at least 99 % restore with at most 30 s of work lost. | Fault injection and scripted restart. | R5 RL-05/RL-14 | F1 | No |
| REL-014 | Completed Monte Carlo batches shall never be lost: every finished batch is written to disk before it is reported done. | 0 completed batches lost in 100 kill -9 trials during a run; a resumed run loses at most 1 batch of work. | Kill and resume test on an OpenMC adapter run. | R6 reliability row; OpenMC restart from statepoint [V](https://docs.openmc.org/en/stable/usersguide/settings.html), fixed-source scope [U] | F2 | Partial: the demo's runs write recorded bundles on completion; mid-run batches are not checkpointed |
| REL-015 | Long external runs shall resume from the last checkpoint. | Resume after kill with at most 1 batch lost, and the resumed result is statistically consistent (PRV-023). | Kill and resume test. | R5 RL-18 | F2 | No |
| REL-016 | Saving shall keep rolling backups of the previous versions. | At least 3 versions kept by default; count configurable; restore reproduces the old hash (COL-031). | Test. | R5 RL-06 | F1 | No |
| REL-017 | Undo and redo shall survive autosave recovery. | At least 100 steps; history present after recovery in 100 % of tests. | Fault injection after 100 edits. | R5 RL-13 | F2 | No |
| REL-018 | Disk full, read-only folders and permission errors shall fail the save safely with a plain message, and leave the previous file intact. | 100 % of injected errors leave the old file intact; message names the cause and the free space needed. | Fault injection on a size-limited filesystem. | R5 RL-12 | F1 | Partial: atomic write protects the old file; messages untested |

## Damaged files and compatibility

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| REL-020 | FARIS shall read damaged study files fail closed: a damaged file is refused with a message naming the damaged item, and nothing partial is shown as if complete. | 100 % of single-byte corruptions detected over every byte of a fixture study; 0 panics; 0 results displayed from an unverified blob. | Exhaustive byte-flip test on a small study; sampled on a large one. | R5 RL-07; docs/STUDY_FILE.md reading rules | F1 | Met: every blob hashed on read, mismatch refuses the file and names the blob; unknown major and unknown encodings refused (crates/faris-study tests); no exhaustive byte-flip campaign |
| REL-021 | FARIS shall refuse or flag, never skip, anything it cannot interpret. | 0 silent skips: unknown encoding, unknown major version, duplicate entries, missing or extra entries all give a named refusal. | Negative corpus test. | docs/STUDY_FILE.md | F1 | Met: reader refuses unknown encodings, versions, repeated entry names, zip64, over-size blobs and unsafe paths |
| REL-022 | A damaged study shall offer a read-only salvage view that shows the undamaged parts and lists the damaged ones by name, clearly marked as incomplete and never usable for verdicts. | All undamaged blobs recovered in 100 % of corruption tests; 0 verdicts from a salvaged study. | Corruption test. | R5 RL-08; fail-closed house rule | F2 | No |
| REL-023 | Studies shall keep working across releases: every archived fixture from every released version shall open (see PRV-053). | 100 % of fixtures open and round-trip without numeric change. | Fixture archive in CI. | R5 PD-14/PD-16 | F1 | Partial: format versioning and minor-version acceptance tested; no fixture archive |
| REL-024 | Preferences and caches shall be disposable: deleting them shall never prevent a study from opening. | 100 % of studies open with a fresh configuration directory. | Test. | FARIS choice | F1 | No |
| REL-025 | Upgrading shall never alter a study file in place: a migrated study is written as a new file and the original is kept. | 0 changes to the original bytes in 100 % of migration tests. | Hash-before-and-after test. | R5 PD-16 | F1 | No |

## Memory, degradation and soak

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| REL-030 | FARIS shall refuse a job that would exceed the memory budget, before it starts, with an explanation and an alternative; the numbers are owned by PERF-041 and PERF-043. | 0 out-of-memory kills in the soak of PERF-043; refusals state projected and free memory. | Stress test inside a cgroup. | Three out-of-memory kills on the reference laptop [internal]; PERF-041 | F1 | Partial: runs are capped by external scripts, not by FARIS |
| REL-031 | When memory runs short mid-run, FARIS shall save what it can and stop the job cleanly rather than be killed. | 100 % of induced low-memory events end in a clean stop with the study intact. | Cgroup shrink test. | FARIS choice | F2 | No |
| REL-032 | FARIS shall still open and show cached results when a GPU, adapter or external tool is missing, and name what is missing. | 100 % of adapters and the GPU tested "missing": no panic, a degraded-mode banner naming the part, cached results shown. | Negative tests per adapter and a no-GPU virtual machine. | R5 RL-09; R5 PD-03 | F1 | Partial: the study opens without OpenMC (recorded bundles); GPU-absent behaviour untested |
| REL-033 | Degraded modes shall be honest: a result computed in a degraded mode (software renderer, reduced precision, missing adapter) carries that fact in its receipt and label. | 100 % of degraded runs labelled. | Test per degraded mode. | House rule: kind labels | F2 | No |
| REL-034 | An 8-hour interactive soak (open, edit, run, cancel, export, close, in loops) shall show no crash, hang or leak. | 0 crashes, 0 frames over 50 ms from the UI thread (PERF-014), resident memory drift at most 5 % after warm-up (PERF-042). | Nightly soak on RL. | R5 RL-02 | F1 | No |
| REL-035 | A 72-hour mixed-job soak shall show 0 crashes, 0 out-of-memory kills and 0 orphan processes. | As stated. | Weekly soak on RL. | R5 RL-02; PERF-043 | F2 | No |
| REL-036 | Cancelling, closing or killing FARIS shall leave no running solver, temporary workspace over 100 MB or half-written output. | 0 orphans after 1,000 randomised cancel, close and kill trials; workspace removed on next start if the app was killed. | Process and disk audit after each trial. | PERF-024; docs/STUDY_FILE.md: workspace removed on exit | F1 | Partial: workspace removed on normal exit; kill case and orphan audit untested |
| REL-037 | Network loss shall never affect a calculation, a save or an export. | 0 failures in these operations with the network cut at a random moment, signed in or not. | Fault injection in a network namespace. | Offline-first (SEC-050) | F1 | Partial: sign-in poller runs in its own thread; no fault-injection test |
| REL-038 | Interrupted downloads of data libraries shall resume and verify (PLAT-050) and shall never leave a partly valid library in place. | 100 % of interrupted downloads resumed or cleanly discarded; hash checked before use. | Interrupt test. | PLAT-050 | F2 | No |

## Crash reports and incidents

| ID | Requirement | Target | Verification | Basis | Phase | Now |
| --- | --- | --- | --- | --- | --- | --- |
| REL-040 | FARIS shall store crash records locally and send nothing unless the user chooses to send a reviewed report. | 0 outbound requests with reports off or pending; upload only after a click on a screen that shows the full content. | UI test plus network trace. | R5 RL-16; Mozilla crash reporter [U] | F1 | No |
| REL-041 | A crash report shall contain no study content, paths, names or geometry. | Fields limited to an allow-list (version, platform, stack, error code); a scanner finds 0 paths or study strings in 1,000 injected crashes. | Schema test and string scan. | R5 trap 17: design geometry is commercially sensitive | F1 | No |
| REL-042 | Crash records shall purge automatically. | Deleted after 30 days by default; "clear now" button; 0 left after clearing. | Test. | R5 OB-09 | F1 | No |
| REL-043 | A diagnostics bundle shall be one click, reviewable and redacted. | Produced in at most 10 s, at most 20 MB, with a manifest of contents and 0 secrets or paths found by the redaction scan. | Test. | R5 OB-06 | F1 | No |
| REL-044 | Data loss shall be a zero-tolerance incident: any report of lost or corrupted study data is investigated and blocks the next release until a regression test exists. | 0 open data-loss defects at release; each closed defect has a regression test in CI. | Release checklist; defect log (see QA-002 for tracing). | House rule: honest, fail-closed rigour | F1 | No |
| REL-045 | A release shall publish its known open reliability defects with severity. | 0 open S1 and 0 S2 without a documented waiver. | Release gate. | R5 VV-14 | F1 | No |

## Traps

- Crash-free percentage depends on the denominator (sessions or users) and on who reports. Use soak and fault injection as primary evidence, field reports as secondary.
- Autosave that is not atomic can corrupt the autosave itself, and a recovery path that is never exercised does not work. Test with kill -9 and unsynced-write emulation, not by hope.
- Fail-closed reading loses its value if a salvage view shows partial data as if complete. Mark it and block verdicts.
- Atomic rename does not make the data durable. Sync the file and the directory, then test.
- Soak tests that only idle prove little. Mix open, edit, run, cancel and export, and check for orphans and temporary files, not only memory.
- An error budget that is never spent usually means the crashes are not being counted.
- A "graceful" degraded mode that silently changes numbers is a data-quality failure. Label it in the receipt.
