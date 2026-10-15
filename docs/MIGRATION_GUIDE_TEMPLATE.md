<!-- kind: how-to -->
# Migration guides

Requirement DOC-041. A change that can break something a user has (a file format, a command or flag, a library
call, an output column, or the value of a result) comes with a guide. This page is the policy and the template.
A real guide is a copy of the template under `docs/migrations/FROM-to-TO.md`, named for the versions it covers.

## Policy

- Every entry under **Changed** or **Removed** in the changelog that breaks existing use links to its guide.
- A guide has a before and an after example. Where the example is a `faris` command, it is written in a fenced
  block so that `scripts/doc_samples.py` runs or checks it in CI.
- A change that alters a published number says so at the top of the guide, with the size of the change on the
  benchmark set when that is known, and says "not measured" when it is not.
- The release notes list every guide for the release (`docs/RELEASE_NOTES_TEMPLATE.md`).
- A file the older version wrote still opens, or the guide gives the exact conversion. A format with no
  conversion is said to have none, and why.

## Template

Copy from here. Replace every angle-bracketed field; a guide with one left in fails review.

```markdown
<!-- kind: how-to -->
# Moving from <old version> to <new version>

## What changed

<One paragraph in plain English. Say who is affected and who is not.>

## Does it change a number

<Yes or no. If yes: which results, by how much on which cases, or "not measured".>

## Before

<The command, file or call as it was.>

## After

<The same, as it is now.>

## Your existing files

<Whether older files open, and the conversion if they do not.>

## If something goes wrong

<The error message the user will see and what to do.>
```
