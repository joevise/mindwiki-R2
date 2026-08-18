# Wiki Contract

This is the source of truth for the common structure copied into each Wiki's local `schema.md`. Copy its rules and templates rather than linking back to this Skill, leaving the generated Schema self-contained.

## Required layout

- `index.md` is the Index: the root retrieval entry point for the Wiki.
- `log.md` is the chronological record of operations with durable results.
- `schema.md` records the Wiki's topic, purpose, page types, organization, frontmatter, naming and evidence conventions, and Page Templates.
- `sources/<source_id>/` contains immutable Source Bundles identified by full UUID v4 Source IDs.
- `wiki/` contains agent-maintained Wiki Pages. Root control documents are not Wiki Pages and need no page frontmatter.
- `wiki/source-records/` contains Source Records.
- Ordinary Wiki Pages remain directly under `wiki/`; initialization creates no domain directories, and page types remain independent of paths.

Every Markdown page under `wiki/` has a Schema-defined `type` in YAML frontmatter. Ordinary pages have no other universal fields. A Source Record additionally has the full UUID v4 `source_id` and a `source_path` relative to `sources/<source_id>/` that identifies its Primary Document.

Each integrated Source ID has exactly one Source Record.

## Retrieval Map

The Index states the Wiki's topic and purpose and contains a small set of semantically described top-level routes when knowledge exists. Each route uses readable link text and nearby context to explain the knowledge it covers. Initialization records that no knowledge has been integrated yet; it creates no speculative routes, Wiki Pages, hubs, or Source Record catalog.

The Index and contextual Wikilinks among ordinary Wiki Pages form the directed Retrieval Map. Every ordinary Wiki Page must be reachable from the Index through unambiguous Wikilinks originating in the Index or another reachable ordinary Wiki Page. A directly exhaustive Index remains valid, but direct Index links are not required when an appropriate progressive route exists.

Source Records belong to the evidence layer. They need no direct Index route, do not route between ordinary Wiki Pages, and their backlinks do not establish reachability. Retrieval needs no universal routing metadata, dedicated page type, or fixed directory structure beyond the required layout.

## Source Record names and citations

Name a Source Record `<semantic-slug>--<first-8-source-id-hex-chars>.md`. If that prefix collides, extend only the new record to 12 characters. Its basename is immutable and Vault-unique; the record may move between directories.

References to Source Records use pathless Wikilinks. Wiki claims cite semantic Evidence headings with `[[<source-record-basename>#<Evidence heading>|<readable citation>]]`.

## Page Templates

Keep Page Templates inside `schema.md`, with no separate template directory or engine. Give each domain type only stable sections justified by the Wiki's needs, and fix its template's `type` to that type's exact name.

Use this common base for new Source Records:

```markdown
---
type: source-record
source_id: "<full-uuid-v4>"
source_path: "<bundle-relative-primary-document>"
---

# <Source title>

## Summary

<The Source's durable contribution and material scope or limitations.>

## Evidence

### <Semantic evidence heading>

- Source: `<bundle-relative-file>`
- Lines: `<start>-<end>`
- Notes: <What this Evidence supports, qualifies, or challenges.>

## Related Wiki Pages

- [[<Wiki Page>|<display text>]]: <How the Evidence informed it.>
```

`Source` is required for every Evidence entry. `Lines` is required for text Evidence and omitted for non-text attachments. `User Insights` and `Open Questions` appear only when populated; User Insights identify the user as their source rather than presenting them as Source-backed claims.
