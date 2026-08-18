# Repair the Wiki

## 1. Fix and check the repair plan

Require an empty Git index before making any change. If changes are staged, leave them untouched and stop repair so the user can resolve them; retain the read-only findings. Existing unstaged and untracked content does not block repair and must remain untouched. Re-read `schema.md`, every selected target, all backlinks to changed pages or headings, each affected Retrieval Map branch and its descendants, and the relevant portions of `index.md` and `log.md`.

Resolve every selected repair under the current Schema. Validate each selected retrieval route against the reread branch and account for every descendant affected by changing it. Schema evolution excluded during repair selection remains outside this commit. Before renaming an Evidence heading, enumerate every incoming citation and fix the exact old-to-new mapping. Return to the user before editing if the mapping remains ambiguous or if validation reveals an unlisted file effect, unsupported claim, Source identity change, or wider repair.

**Done when:** the selected pages, links, Evidence mappings, Index effect, and Log entry form one exact Schema-conforming change with no ambiguous provenance or hidden file effects.

## 2. Apply only the selected repairs

Make one coherent maintenance change:

1. Update only the selected agent-maintained Wiki Pages and control documents.
2. Preserve each page's Schema-defined `type`. Keep unresolved Evidence conflicts qualified and preserve durable User Insights as explicitly user-attributed.
3. Apply an approved Evidence anchor rename and all of its backlinks together. Point each citation only to Evidence whose Source fragment supports the prose; never repurpose an old anchor for unrelated Evidence.
4. Apply the selected retrieval route plan, including its descendant-preservation and Index effects. If the user selected an important finding for persistence rather than an immediate repair, record it in the relevant durable Wiki content instead of storing the lint conversation.
5. Append one dated `lint` entry to `log.md`, using the Schema-defined Log format, naming the repaired findings or persisted concern, affected pages, and material provenance effects.

Do not add repairs that merely became convenient while editing. If the selected scope produces no persistent result, restore no files because none should have changed, and create no Log entry or commit.

**Done when:** the repair changes contain all and only the selected repair, an accurate Index when needed, and one durable lint Log entry.

## 3. Verify and commit

Rerun the structural helper and compare its JSON with the initial report. Verify all of the following before staging:

- each selected structural finding is resolved and no new structural finding was introduced;
- each changed claim is supported by its located Evidence or clearly qualified, with competing Evidence and User Insights kept distinct;
- every approved heading migration updated all backlinks to the intended Evidence;
- every changed page follows the current Schema and every changed Wikilink resolves;
- the selected retrieval route plan is fully applied and repaired knowledge and every affected descendant remain reachable;
- the selected Index effect is exact and `log.md` records this lint operation exactly once;
- the repair changes are distinct from pre-existing worktree content, and include no Source Bundle, Schema, unselected file, or unrelated change.

Stage only the selected repair changes, inspect the staged diff, and create one local commit named `Repair wiki: <concise scope>`. The repair selection already authorized this commit, so ask for no further Git confirmation. Never push.

Report the repaired findings, affected pages, remaining findings, and commit identifier. Verify that the Git index is empty and that pre-existing unstaged and untracked content remains untouched; the final worktree need not be clean.

**Done when:** one local commit contains the selected repair and nothing else, no push occurred, the Git index is empty, and pre-existing unstaged and untracked content remains untouched.
