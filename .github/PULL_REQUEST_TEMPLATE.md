## What this changes

<!-- One or two sentences, in plain English. -->

## Checks

- [ ] If this fixes a defect: I added a regression test that fails without the fix and passes with it, and the entry in `docs/defects/faris-defects.json` names it (QA-085). If no test is possible, I say why below.
- [ ] If this adds or changes a requirement's behaviour: the test carries a `Verifies: ID` comment, and I did not edit the requirement tables by hand.
- [ ] If this changes a command or option: I regenerated `docs/guide/cli-reference.md`.
- [ ] If this can alter a published number: it is listed under **Numerical changes** in `CHANGELOG.md`.
- [ ] If this breaks something users have (file format, command, option, result): it follows `docs/DEPRECATION_POLICY.md` and has a migration guide.
- [ ] If this adds a dependency: its licence is on `scripts/licence_policy.json` and its text is in `licenses/`.
- [ ] `cargo fmt --all --check`, clippy with `-D warnings`, and the tests of the crates I changed pass.
- [ ] Commits are signed off (`git commit -s`) if this comes from a fork.

## Notes for the reviewer

<!-- Anything that is not obvious: why no regression test, what you did not check, what a number moved by. -->
