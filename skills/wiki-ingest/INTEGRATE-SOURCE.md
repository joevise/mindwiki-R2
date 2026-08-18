# Integrate a Source

## 1. Fix the provenance plan

Apply the local Schema already read. Search Source Records for the full Source ID and proceed only when none exists. Refresh a relevant Wiki Page or Retrieval Map route only if it changed since it was read.

Choose a concise semantic slug and append the first four hexadecimal characters of the Source ID after removing hyphens: `<semantic-slug>--<source-id-prefix>.md`. Search all Markdown files in the Vault for that basename and stop without overwriting if it already exists. Place the record in the Schema's dedicated Source Record area and keep this basename immutable.

Plan an Evidence Map with semantic headings that remain useful citation anchors. Every text Evidence entry names its Bundle-relative file and a valid line range. Every non-text Evidence entry names its Bundle-relative file and omits a line range. Optional excerpts and headings are reading aids rather than replacements for these locators. When the Primary Document has source-authored frontmatter, plan the Schema's optional Source Metadata section with its locator and useful identity, attribution, publication, or retrieval fields.

Identify every Wiki Page to create or revise by compiling the Source's reusable conclusions and relationships for likely future questions. Keep long-tail details in the Source until they have durable reuse value. For each new ordinary Wiki Page, choose a semantically appropriate incoming route from the nearest relevant reachable ordinary page. Use the Index only when the page introduces a genuinely new top-level subject. Source-backed prose cites the relevant Evidence heading with a pathless Wikilink in this form:

```markdown
[[<source-record-basename>#<Evidence heading>|<readable citation>]]
```

Account for unresolved Evidence Conflicts with competing citations and qualified synthesis. Keep durable User Insights explicitly attributed to the user.

**Done when:** the provenance, reusable synthesis, conflict treatment, and reachable local routes are determined under the local Schema.

## 2. Apply the Integration

Confirm that the Git index is still empty. Worktree changes do not block Integration. Then make one coherent Integration:

1. Create exactly one Source Record using every field and section required by the local Source Record Template. Its frontmatter includes only the Schema's Source Record fields, including `type: source-record`, the full UUID v4 `source_id`, and the Bundle-relative Primary Document as `source_path`. Summarize the Source's durable contribution and material scope or limitations. Represent Source Metadata, the Evidence Map, related Wiki Pages, durable User Insights, and open questions in the template's current structure, omitting optional content when empty.
2. Create or update every affected Wiki Page under the current organization and page type definitions. Synthesize reusable knowledge into ordinary Wiki prose, keeping the Source Record focused on Source-level evidence routing. Preserve a Schema-defined `type` in each page's frontmatter and cite Source Record Evidence anchors rather than copying Source Locators into prose.
3. Link the Source Record to every affected Wiki Page with readable Wikilinks that resolve unambiguously under the local organization. Keep each page's incoming Source Record citations pathless as shown above.
4. Add each new ordinary Wiki Page's planned incoming route from the nearest relevant reachable ordinary page, or from the Index for a genuinely new top-level subject. Change `index.md` only when the Integration introduces or changes a top-level route; adding knowledge beneath an existing branch changes only nearby routes. Append one dated `ingest` entry to `log.md`, using the Schema-defined Log format, naming the Source, Source ID, Source Record, and durable page effects.
5. Remove `sources/.gitkeep` and `wiki/source-records/.gitkeep` when real content now retains those directories. Leave the imported Source Bundle unchanged and omit the raw Ingest Session transcript.

**Done when:** all planned Integration files are written without modifying the imported Bundle.

## 3. Verify and commit

Before staging, review the Integration once against the local Schema. Confirm that the Source Record identity, Primary Document, any Source Metadata, Evidence locators, and pathless citations resolve; changed Wiki Pages have defined types and reachable local routes; synthesis, conflicts, and User Insights match the provenance plan; and the Index and Log have only their warranted effects.

Stage only the Bundle and this Integration's changes, inspect the staged diff, confirm that it contains the complete Integration and nothing else, and create one local commit named `Integrate source: <Source title>`. The readiness response already authorized this commit, so ask for no further Git confirmation. Never push.

Report the Source ID, Source Record, affected Wiki Pages, and commit identifier.

**Done when:** one local commit contains the complete Integration and nothing else, and no push occurred.
