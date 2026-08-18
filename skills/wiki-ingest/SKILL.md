---
name: wiki-ingest
description: Ingest a new user-supplied file or directory into an existing WikiOps Wiki by discussing and integrating it as a Source.
---

# Wiki Ingest

Work at the root of the Wiki, which is both an Obsidian Vault and a standalone local Git repository.

## Prepare the Ingest

1. Resolve the Wiki root and exactly one existing input file or self-contained directory. If the Wiki or input is ambiguous, ask for it.
2. Read `schema.md` in full. Read `index.md`, inspect the Wiki tree, and confirm `log.md`, `sources/`, and `wiki/source-records/` exist. Treat missing contract elements as structural findings for `wiki-lint` rather than repairing them during ingest.
3. Confirm that the Git root is exactly the Wiki root and require an empty Git index before import. A dirty worktree does not block ingest.

**Done when:** the Wiki and new Source input are known, the required Wiki structure exists, the Wiki is its own Git root, and the Git index is empty.

Read and follow [INGEST-SOURCE.md](INGEST-SOURCE.md).
