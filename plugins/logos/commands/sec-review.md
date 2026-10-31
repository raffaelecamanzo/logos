---
description: Review the security findings this branch introduced or touched, or with --backlog the stored findings no one has judged. Fixes only what is safe to fix, proves each fix by a targeted re-scan, and escalates secrets, dependencies and the unclear.
argument-hint: "[--base <ref>] | --backlog [--limit <n>] [--rule <id>]... [--path <path>]..."
---

Run the plugin's `logos-sec-review` skill (the `logos:logos-sec-review` skill) with the
Skill tool, in this conversation, with these arguments: $ARGUMENTS

No arguments means the default branch is the base. `--base <ref>` names the base.

`--backlog` triages the stored findings no one has judged instead of a branch's delta (see
the skill for what it selects). `--limit <n>` takes the first n by severity, `--rule <id>`
keeps one rule id and `--path <path>` a file or directory (both repeatable). The report says
how many findings the run did not reach.

The skill gives every introduced or touched security finding one verdict (`fix`,
`escalate-secret`, `escalate-dependency`, `false-positive` or `untrusted`; a backlog has
`fix`, `false-positive` or `untrusted`), edits only the `fix` code, and proves each fix by a
re-scan with the same scanner. Show the user the report as the skill returns it. For each
`FIXED` line say which files changed so they can look at the diff, for each `ROTATE` line
say that the credential has to be rotated by a person, and for each `SUPPRESS` line say that
the ignore-file entry (and, with evidence, the `logos security reconfirm` command) is waiting
for their decision. Do not commit anything, and do not record scan consent: if a scan needs
it, ask the user to run `logos security scan --consent`.
