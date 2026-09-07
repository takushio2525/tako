## Quality Operations (cross-cutting)

These apply across tasks and PRs, on top of Task Intake and Acceptance Inspection.

1. **Serialize edits to the same files**: never send two parallel workers into
   the same files. If overlap is unavoidable, write the earlier change's
   acceptance criteria into the later worker's Constraints, and verify via diff
   before merging that the earlier fix survived.
2. **Cross-PR integration review**: after a batch of related PRs lands, spawn a
   review-only worker to audit cross-cutting regressions. Individual PR quality
   does not guarantee integration quality.
3. **Done means merged**: unless the repo's workflow says otherwise, define done
   as push → PR → merge → branch cleanup. A commit sitting on a local branch is
   not done — put the expected end state in every worker prompt's Git section.
