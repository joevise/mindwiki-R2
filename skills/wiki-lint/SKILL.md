---
name: wiki-lint
description: Lint and repair an existing WikiOps Wiki. Use when checking structural or semantic Wiki health, investigating stale or unsupported knowledge, or applying selected maintenance repairs.
---

# Wiki Lint

Work at the root of the Wiki, which is both an Obsidian Vault and a standalone local Git repository.

## Establish the lint run

1. Resolve the Wiki root and any health concern the user wants emphasized. If the Wiki is ambiguous, ask for it.
2. Read `schema.md`, `index.md`, and `log.md` when present. Inspect the Wiki tree and note missing contract elements for the structural report rather than filling them immediately.
3. Confirm that the Git root is exactly the Wiki root. Record the initial worktree state, but allow the read-only lint investigation to continue when it is dirty. Leave existing changes untouched; repair has a separate clean-worktree gate.

**Done when:** the target and repository checks are complete, its local Schema and navigation are understood when present, and its initial worktree state is known.

Read and follow [LINT-WIKI.md](LINT-WIKI.md).

## Mutation boundary

- Keep lint read-only through investigation and repair selection. A run with no selected persistent result changes no files and creates no Log entry or commit.
- The user's choice of an exact repair scope authorizes that Wiki change and one local commit. Apply only that scope and never push.
