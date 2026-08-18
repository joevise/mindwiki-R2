# File an Answer

## 1. Fix and check the persistence plan

Require an empty Git index before making any change. If changes are staged, leave them untouched and stop filing so the user can resolve them; the read-only answer remains available. Existing unstaged and untracked content does not block filing and must remain untouched. Re-read `schema.md`, every proposed target page, the relevant Retrieval Map branches, and the relevant portions of `index.md` and `log.md`.

Prefer updating relevant Wiki Pages over creating a new page. Create one only when the answer has a distinct durable subject and an existing Schema-defined type fits it. Search related page titles, text, and Wikilinks so the plan does not duplicate existing knowledge. For every new ordinary Wiki Page, plan a semantically appropriate incoming route from the nearest relevant reachable ordinary page, or from the Index when the Filed Answer introduces or changes a genuinely top-level subject or route. Filing under the current Schema does not authorize Schema evolution. If no existing type or organization fits, keep the answer read-only and report the unmet Schema need.

Check every Source-backed claim that will be filed against its Source fragment, even when the read-only answer did not require a reread. Reuse an existing Evidence anchor only when its located fragment supports the prose. When a relevant fragment in an integrated Source lacks an Evidence entry, plan a semantic entry in that Source Record with its exact Bundle-relative `Source` path and, for text, valid `Lines`. Preserve competing Evidence for unresolved disagreements and keep User Insights explicitly attributed.

If these checks reveal an unlisted file effect, unsupported claim, or wider change, stop before editing and return to the filing proposal in `QUERY-WIKI.md` with the revised exact scope. Proceed only after the user chooses that proposal.

**Done when:** the authorized pages, optional Source Record Evidence additions, citations, nearest route changes, any top-level Index effect, and Log entry form one exact Schema-conforming change; every Source-backed claim has been checked against its located fragment; and every planned new ordinary page will remain reachable from the Index.

## 2. Apply the Filed Answer

Recheck that the Git index is empty, then make one coherent change without disturbing pre-existing unstaged or untracked content:

1. Create or update the proposed Wiki Pages using the current Schema-defined types and organization. Integrate the durable result rather than storing the chat answer verbatim.
2. Cite Source-backed prose through pathless Source Record Evidence Wikilinks. Add only the planned Evidence entries and related-page links when existing anchors do not provide the checked support.
3. Add each new ordinary Wiki Page's planned incoming route from the nearest relevant reachable ordinary page, or from the Index for a genuinely new top-level subject. Change `index.md` only for an altered top-level route; knowledge added beneath an existing branch changes only nearby routes.
4. Append one dated `query` entry to `log.md`, using the Schema-defined Log format, that identifies the durable result, affected pages, material provenance, files consulted, and the traversal, search, and verification scope without recording the conversation.

Keep every Source Bundle byte-for-byte unchanged. Keep `schema.md` unchanged and apply no file effect outside the authorized proposal.

**Done when:** the Filed Answer consists only of the integrated durable knowledge, any required Evidence Map maintenance, accurate local Retrieval Map routes, any warranted top-level Index change, and one Filed Answer Log entry, all distinguishable from pre-existing worktree content.

## 3. Verify and commit

Verify all of the following before staging:

- every created or changed Wiki Page has a current Schema-defined `type`;
- every Source-backed claim is supported by the checked Source fragment and cites the intended pathless Evidence heading;
- each added Evidence path exists, each text line range is valid, and non-text Evidence omits a line range;
- every changed Wikilink resolves, competing Evidence remains visible, and User Insights remain user-attributed;
- every new ordinary Wiki Page is reachable from `index.md` through a semantically appropriate, unambiguous route, with nearby routes preferred over a new root route;
- `index.md` changes only for an altered top-level route, and `log.md` records this Filed Answer exactly once using the Schema-defined format;
- no Source Bundle or Schema content changed, and the Filed Answer changes are distinct from any pre-existing unstaged or untracked content.

Stage only this Filed Answer's changes, including only its hunks when an affected path contains pre-existing edits. Inspect the staged diff and confirm that it contains the complete Filed Answer and nothing else, then create one local commit named `File answer: <concise topic>`. The filing choice already authorized this commit, so ask for no further Git confirmation. Never push.

Report the affected Wiki Pages, Evidence checked or added, and commit identifier. Verify that the Git index is empty and that pre-existing unstaged and untracked content remains untouched; the final worktree need not be clean.

**Done when:** one local commit contains the complete Filed Answer and nothing else, no push occurred, the Git index is empty, and pre-existing unstaged and untracked content remains untouched.
