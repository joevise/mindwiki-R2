# Lint the Wiki

## 1. Run the structural report

Run the bundled read-only helper from the Skill's installed directory:

```bash
python3 "<wiki-lint-skill-directory>/scripts/lint_wiki.py" --wiki-root "<wiki-root>"
```

Retain every JSON finding with its code, Wiki-relative path, optional line, and message. The helper reports missing skeleton elements, missing page metadata, Source identity and locator failures, broken or ambiguous Wikilinks, unstable pathful Source Record links, Unintegrated Sources, basename collisions, and ordinary Wiki Pages that are unreachable through the directed Retrieval Map. It uses page types and Source Record Evidence layout from the Schema when they can be read, but does not lint Schema wording, headings, template structure, Source Record section names, or filename style. Only unambiguous links from the Index or reachable ordinary Wiki Pages establish reachability; Source Records and their backlinks do not. An Unintegrated Source indicates an incomplete ingest, not proof of corruption.

Confirm that the helper created no file and that the worktree state still matches the recorded initial state.

**Done when:** the deterministic report is captured unchanged and the Wiki has not been modified.

## 2. Investigate semantic health

Start from `index.md`, then inspect pages directly relevant to the user's concern or implicated by structural findings. Look for actionable problems such as:

- materially contradictory accounts that are presented as settled;
- stale or misleading routes that prevent finding the relevant knowledge;
- claims whose cited Evidence appears missing or materially inconsistent with the prose.

Follow citations into exact Source fragments only when support is in question. Keep unresolved accounts visible rather than choosing one silently, and distinguish user-attributed insights from Source-backed claims. Do not broaden a lint run into a general content review without a concrete concern.

For a dangling Evidence anchor, search all backlinks and relevant Git history, then compare candidate Evidence entries and their Source Locators. Record a migration as certain only when the intended Evidence is clear. Treat multiple plausible targets or changed meaning as ambiguous.

**Done when:** each concrete concern has enough evidence to report or dismiss, and every candidate anchor migration is classified as certain or ambiguous.

## 3. Present selectable findings

Present machine-observed structural facts separately from agent judgments about semantic recall quality. Number each independently selectable repair and state:

- what is wrong and the Evidence for that conclusion;
- the exact files and links that would change;
- the intended result, the nearest appropriate reachable page for each repaired incoming route, and how descendant reachability is preserved;
- whether the root Index changes and the top-level route change that warrants it;
- any provenance, uncertainty, identity, or Schema consequence.

For a repaired incoming route, prefer the most specific reachable ordinary page whose described coverage includes the repaired subject. When multiple candidate routes remain semantically plausible, present their different retrieval effects and exclude the route repair until the user chooses one.

Do not offer an automatic identity change that would rewrite an integrated Source Bundle. Classify a new page type, directory boundary, Source Record Template, or other change to the Schema itself as Schema evolution, explain that it is outside lint repair, and exclude it from the selectable repair scope. For every ambiguous Evidence anchor migration, show the plausible mappings and ask the user which meaning is intended; include no guessed mapping in the selectable repair scope.

Ask which repairs, if any, to apply. State that choosing an exact scope also authorizes one local commit and no push. If the user selects nothing, or there is no repair or important finding worth persisting, verify the worktree still matches its initial state and finish with no Index change, Log entry, or commit.

After the user selects an exact persistent result, read and follow [REPAIR-WIKI.md](REPAIR-WIKI.md).

**Done when:** lint has ended read-only, or the user has selected a fully specified repair or durable finding and its local commit.
