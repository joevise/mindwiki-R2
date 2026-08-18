# Query the Wiki

## 1. Navigate accumulated knowledge

Start from `index.md`. Expand every semantically plausible top-level route needed by the question. When a cycle returns to an ordinary Wiki Page already read, continue without rereading it. Follow contextual Wikilinks through every relevant branch rather than stopping at the first partial answer.

Complement route traversal by searching ordinary Wiki Page titles, page types, prose, and Wikilinks for the question's terms, aliases, and likely related vocabulary. Limit this ordinary search to the generated knowledge layer: exclude `sources/` and Source Records by default. Read every relevant match and account for its existing Evidence citations without opening the cited Source Records or Source fragments yet.

For a question about a named Source, search Source Record titles, Summaries, and Evidence Maps directly. Treat those records as evidence-selection aids, not as intermediates for routing among ordinary Wiki subjects.

Judge whether the accumulated Wiki synthesis and its existing Evidence citations can responsibly settle the question. Record material gaps, disagreements, unusable provenance, and any plausible route or search that could not be completed.

**Done when:** the relevant Wiki synthesis, its Evidence citations, and any material gaps or disagreements are accounted for.

## 2. Verify only where warranted

Stay in the Wiki layer when its synthesis and existing usable Evidence citations are sufficient. Follow Evidence into a Source only when the user requests verification, exact Source language matters, the Wiki is uncertain or internally inconsistent, cited accounts materially disagree, provenance is unusable, or the Wiki cannot otherwise settle the claim responsibly.

For each claim that warrants checking:

1. Select the Source Record named by the relevant Wiki Evidence citation. Read its Summary and semantic Evidence Map only as needed to confirm the appropriate Evidence entry.
2. Read the record's full `source_id`, then resolve that entry's `Source` path under `sources/<source_id>/`.
3. For text Evidence, read the stated `Lines` plus only the adjacent context needed to interpret them. For non-text Evidence, open only the named attachment with the host's capabilities.
4. Compare the fragment with the proposed claim. Follow competing Evidence when a material disagreement exists.

If a relevant Wiki claim has no usable citation, search likely Source Record titles, Summaries, and Evidence Maps before widening the search. Do not broadly search or reread the Source Store. Report a missing record, heading, Source path, or line range as a verification limit rather than silently relying on the Wiki prose. Read a larger portion of the selected Source only when its located fragment lacks enough context to answer responsibly.

**Done when:** each material part of the answer is supported by a usable Evidence citation, clearly identified as synthesis or User Insight, or reported as uncertain; every requested check has reached the relevant Source fragment.

## 3. Answer with provenance

Answer the user's question directly. Cite Source-backed claims with the existing pathless Evidence target:

```markdown
[[<source-record-basename>#<Evidence heading>|<readable citation>]]
```

Keep competing citations beside qualified synthesis when Sources disagree. Distinguish Wiki synthesis, findings checked against Source fragments, user-attributed insights, and unresolved questions. Preserve usable Evidence citations when a Source reread was not warranted, and do not imply that cited Source fragments were opened or rechecked. State any material retrieval or verification limit.

**Done when:** the answer addresses the question, preserves the Evidence trail and material uncertainty, and has not changed the Wiki or Git history.

## 4. Decide whether to offer filing

Offer to file the answer only when it has durable reuse value, such as reusable cross-Source synthesis, a correction to stale knowledge, a newly established connection, or an open question worth retaining. Name why it is durable and propose the exact Wiki Pages and Source Records to create or update under the current Schema, the nearest affected Retrieval Map routes, any top-level routing effect on the Index, and the Log update, including the files consulted and the traversal, search, and verification scope. Do not propose changing the Index when the knowledge fits beneath existing top-level routes. State that choosing to file also authorizes one local commit and no push.

If the user already explicitly requested this answer be filed, that request supplies the choice once the exact proposal is known. Otherwise, ask whether to file it. A declined or non-durable answer remains read-only: verify the worktree state has not changed from its initial state, then create no Log entry or commit.

After the user chooses filing, read and follow [FILE-ANSWER.md](FILE-ANSWER.md).

**Done when:** the query has ended read-only, or the user has authorized an exact durable result and its local commit.
