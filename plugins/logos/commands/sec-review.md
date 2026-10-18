---
description: Review the security findings this branch introduced or touched. Fixes only what is safe to fix, proves each fix by a targeted re-scan, and escalates secrets, dependencies and the unclear.
argument-hint: "[--base <ref>]"
---

Run the plugin's `logos-sec-review` skill (the `logos:logos-sec-review` skill) with the
Skill tool, in this conversation, with these arguments: $ARGUMENTS

No arguments means the default branch is the base. `--base <ref>` names the base.

The skill gives every introduced or touched security finding one verdict (`fix`,
`escalate-secret`, `escalate-dependency`, `false-positive` or `untrusted`), edits only
the `fix` code, and proves each fix by a re-scan with the same scanner. Show the user the
report as the skill returns it. For each `FIXED` line say which files changed so they can
look at the diff, for each `ROTATE` line say that the credential has to be rotated by a
person, and for each `SUPPRESS` line say that the ignore-file entry is waiting for their
decision. Do not commit anything, and do not record scan consent: if a scan needs it, ask
the user to run `logos security scan --consent`.
