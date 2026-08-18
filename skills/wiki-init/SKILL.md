---
name: wiki-init
description: Initialize a new WikiOps Wiki in the current directory.
---

# Wiki Init

Initialize the current directory as the root of a new WikiOps Wiki, Obsidian Vault, and local Git repository.

## 1. Establish the initial model

Check only whether these WikiOps-managed paths already exist:

```text
index.md
log.md
schema.md
sources/
wiki/
```

If one or more managed paths exist, list them all and stop without changing the directory.

Require an empty staging area if the current directory is already a Git root; otherwise, ignore any enclosing repository. Leave untracked and unstaged unrelated files untouched.

### Mind Wiki Host Context 自动初始化模式

当调用方在任务中提供了完整的 `Host Context` JSON，且明确说明“已由用户确认、无需追问”时，使用本模式替代下面的交互问答。Host Context 至少包含 `host_id`、`name`、`background`、`goal`、`expertise`、`build_rule` 中可获得的字段。

1. 将 `name` 与 `expertise` 提炼为 Wiki 的 Topic；将 `background`、`goal`、`expertise`、`build_rule` 共同提炼为 2-4 条具体 Purpose。
2. 把以下五个基础页面类型视为已确认，必须写入 `schema.md` 并各自生成最小模板：
   - `entity`：人物、组织、产品、地点或其他可识别对象。
   - `concept`：可复用的概念、主题、理论或框架。
   - `algorithm`：判断、决策、行动、分析或问题解决的方法。
   - `values`：价值取向、原则、偏好和长期信念。
   - `source-record`：不可省略的来源摘要、证据锚点与原文溯源记录。
3. 基于完整 Build Rule 额外推导 1-3 个 Host 专属页面类型。每个类型必须有清晰、稳定、可区分的定义和最小模板；不可为了凑数创建泛化或重复类型。
4. 将 Host Context 的摘要（不写入任何敏感认证信息）与上述类型的推导理由记录在 `schema.md`。以上内容均视为已确认，不得再次向用户提问。

没有提供上述 Host Context 时，继续使用原来的交互式流程：

Resolve unanswered items in sequence, asking only one question per turn and waiting for its answer before advancing:

1. Ask for the Wiki's topic.
2. Ask what the user wants the Wiki to help them understand or do. Infer two to four topic-specific purposes, recommend the most likely one, and present them as concise choices while allowing a custom answer.
3. Propose the smallest useful set of initial domain page types based on the agreed topic and purpose. Briefly explain that page types classify knowledge independently of folders, define each proposed type in one line, and ask the user to accept or revise the set.

Skip items already answered by the user's request while preserving the order of the rest. Add `source-record` as the required provenance type rather than a domain choice.

**Done when:** all managed paths are absent, any existing staging area is empty, and the topic, purpose, type names, and meaning of each type have been agreed in sequence.

## 2. Create the Wiki

Read [WIKI-CONTRACT.md](WIKI-CONTRACT.md). Recheck that all managed paths remain absent, initialize Git here if the current directory is not a Git root, and create exactly this skeleton without further confirmation:

```text
index.md
log.md
schema.md
sources/
  .gitkeep
wiki/
  source-records/
    .gitkeep
```

Use placeholders only to retain the required empty directories in Git. Build a self-contained `schema.md` by instantiating every contract rule and the common Source Record template. Record the agreed topic and purpose, define `source-record` and every agreed domain type, and include one minimal template for each domain type.

Write `index.md` as an accurate empty retrieval entry point containing the Wiki's topic, purpose, and a Retrieval Routes section that states no knowledge has been integrated yet. Do not add empty Wiki Page or Source Record catalogs. Write `log.md` with a dated Schema entry recording the initialization and agreed initial types. Create no speculative routes, Wiki Pages, hubs, or domain directories.

**Done when:** the current directory is a Git root and contains exactly the new WikiOps-managed skeleton and agreed initial Schema, while every unrelated path remains unchanged.

## 3. Commit

Confirm that the staging area still has no earlier changes. Stage only the five files in the fixed skeleton and create one local commit named `Initialize WikiOps wiki`. Never push.

Report the created paths, commit identifier, and any unrelated untracked or unstaged files that remain; the final worktree need not be clean.
