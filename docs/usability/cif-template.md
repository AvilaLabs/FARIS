<!-- kind: template -->
# Usability study: [study name]

Copy this file to `docs/usability/reports/[date]-[study-name].md` and fill it in. It follows the Common Industry
Format for usability test reports (ISO/IEC 25062): title page, executive summary, introduction, method, results,
appendices (requirement UX-009). `python3 scripts/usability_report.py` checks that every report under
`docs/usability/reports/` has these headings, in this order, and that no field is left as a placeholder.
Do not delete a heading; write "none" under it when nothing applies.

## Title page

- Study: [name]
- Report date: [YYYY-MM-DD]
- FARIS version and commit tested: [version, commit]
- Protocol hash (SHA-256 of the protocol file, fixed before the first session): [64 hex digits]
- Task script hash (UX-010): [64 hex digits]
- Analysis plan hash (UX-074): [64 hex digits]
- Written by: [name]

## Executive summary

Three to six sentences: what was tested, with whom, the headline measures (success rate, time on task, SUS),
and the main finding. Every number here appears again in Results with its interval.

## Introduction

### Product tested

The part of FARIS that was used, the platform, and the build.

### Purpose of the test

What the study was meant to find out, in the words of the protocol.

## Method

### Participants

How many, how they were recruited, their experience (the intended user groups of UX-001), and any excluded
under the protocol's exclusion rules, with the rule that excluded them.

### Context of product use

The environment and equipment: machine, screen, operating system, the study files supplied, whether a facilitator
was present, and how sessions were recorded.

### Tasks

The tasks of the task script (T1 to T5), each with its written success criterion. Any task changed after session 1
is listed with the reason and the date; the original is never edited.

### Procedure

The order of sessions, the instructions given, the time limit per task and what the facilitator may say.

### Measures

Effectiveness (task success), efficiency (time on task), satisfaction (SUS), and any others the protocol names,
each with how it was computed (the analysis plan of UX-074).

## Results

### Data analysis

Which analysis ran, on which data file (hash), and every drop of data with the protocol rule that allowed it.

### Effectiveness

Success rate per task, adjusted-Wald 95 % interval.

### Efficiency

Time on task per task: geometric mean, median and P90 with bootstrap 95 % intervals.

### Satisfaction

SUS score with its t-interval.

### Findings and recommendations

Numbered problems, each with the task, the number of participants who met it, its severity and a proposed change.

## Appendices

### Protocol and task script

The files and their hashes.

### Raw data

Where the raw data are kept and how to verify them.

### Deviations from the protocol

Each deviation with its reason, in the order it happened. "None" if there were none.
