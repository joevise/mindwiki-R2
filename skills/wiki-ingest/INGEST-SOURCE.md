# Ingest a Source

## 1. Establish the stable snapshot

Supply exactly one existing file or self-contained directory. A Source with local attachments must be a directory containing its Primary Document and assets together. One supplied directory always becomes one complete Bundle: the helper does not divide a shared inbox or select attachments for individual documents. It preserves relative symlinks whose targets remain inside the directory and rejects links to external mutable content. Run it from the Skill's installed directory:

```bash
python3 "<wiki-ingest-skill-directory>/scripts/import_source.py" --wiki-root "<wiki-root>" "<input-path>"
```

On successful exit, treat the helper's JSON `source_id` and `bundle_path` as the stable snapshot. Continue from that Bundle without rechecking the copy. The helper performs no Primary Document selection, Markdown parsing, attachment discovery, or link rewriting.

Confirm which Bundle-relative file is the Primary Document. A single-file Bundle supplies that answer directly; choose among directory contents with the user when it is not evident.

**Done when:** the helper has established the stable Bundle and full Source ID, and the Primary Document is known.

## 2. Build shared understanding

Read the Primary Document and relevant attachments from the Bundle using the host's capabilities. Discuss what the Source says, what the user wants to understand, and the user's questions or observations. Let the conversation follow the Source rather than applying an automatic-summary checklist.

Follow relevant routes from `index.md`, then search ordinary Wiki Page titles, prose, and Wikilinks for the Source's subjects and likely future question vocabulary. Read the relevant matches before deciding whether knowledge belongs in an existing or new page; follow Evidence citations when relevant Source Records need inspection. Develop candidate reusable synthesis, Source-level Summary, Evidence, local route changes, durable User Insights, and Open Questions without writing the raw conversation into the Wiki.

Compare material claims with existing synthesis. When accounts disagree in a way that affects the resulting knowledge, follow the existing Evidence entries to their Source Locators, surface the Evidence Conflict before Integration, and discuss what remains unresolved. Preserve competing Source-backed accounts; a user's preference may be retained as a User Insight but does not resolve the Evidence Conflict. Attribute durable user interpretations, hypotheses, connections, and questions to the user so they remain distinct from Source-backed claims, including when a user question also appears among Open Questions.

**Done when:** the candidate Integration plan accounts for the Source's reusable contribution, relevant existing pages and routes, material Evidence Conflicts, and unresolved questions.

## 3. Judge Integration Readiness

Decide when the session has enough shared understanding to persist useful knowledge. At that point, ask whether the user has any further questions or additions. If they do, continue the discussion and reassess readiness. If they have none, their response begins Integration and authorizes its local commit; this is not a patch-review or separate Git-approval step.

Read and follow [INTEGRATE-SOURCE.md](INTEGRATE-SOURCE.md).

**Done when:** further questions and additions have been addressed and Integration has begun.
