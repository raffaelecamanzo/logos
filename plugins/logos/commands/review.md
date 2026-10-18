---
description: Review this branch symbol by symbol and fix only what is trusted, covered and worth it. Add "debt" for the where-to-start list instead.
argument-hint: "[--base <ref>] [debt]"
---

Start the `logos:logos-reviewer` subagent (the plugin's `logos-reviewer`) with the Agent tool, so the review's reads and
reruns stay out of this conversation. Give it this request:

> Run the logos-review skill. Arguments: $ARGUMENTS
> No arguments means branch mode against the default branch. `--base <ref>` names the
> base. The word `debt` means debt mode (the "where to start" list), not the branch.
> Return the report the skill describes.

When it returns, show the user the report as given, and for each `FIXED` line say which
files changed so they can look at the diff. Do not commit anything.
