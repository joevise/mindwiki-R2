---
name: wiki-query
description: Query an existing WikiOps Wiki and optionally file a durable answer. Use when asking questions against accumulated Wiki knowledge, verifying claims against Source fragments, or persisting a useful query result.
---

# Wiki Query

Work at the root of the Wiki, which is both an Obsidian Vault and a standalone local Git repository.

## Establish the query

1. Resolve the Wiki root, the user's question, and any specific claims or Sources the user wants checked. If the Wiki or question is ambiguous, ask for it.
2. Read `schema.md` in full. Confirm the Git root is exactly the Wiki root and that `index.md`, `log.md`, `sources/`, and `wiki/source-records/` exist. Treat missing contract elements as structural findings for `wiki-lint` rather than repairing them during query.
3. Record the initial worktree state, but do not require it to be clean for a read-only query. Leave existing changes untouched. Filing requires an empty Git index but allows existing unstaged and untracked content.

**Done when:** the target and repository checks are complete, the question and verification need are clear, and the initial worktree state is known.

Read and follow [QUERY-WIKI.md](QUERY-WIKI.md).

## Mutation boundary

- Keep the query read-only unless the user chooses to file a proposed durable result. A normal query changes no files and creates no Log entry or commit.
- A user's choice to file the proposed result authorizes that exact Wiki change and its one local commit. Never push.
