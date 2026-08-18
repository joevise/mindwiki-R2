# Mind Wiki R2

> 企业级安全 AI 知识库 —— Rust 单二进制

从采集到使用，全程只有系统在看。端到端加密 · 密钥闸门 · 知识可溯源 · 随时带走。

## 架构（六层，层间 trait 隔离）

```
L5 接口层  CLI · Web · (桌面 App)
L4 服务层  axum API · 会话管理
L3 运行时  r2-core 嵌入（LLM + 工具循环 + seccomp 沙箱）
L2 知识库  wiki-engine（init/ingest/query/lint + skills）  ← 可独立更新
L1 安全层  crypto-core + key-gateway（密钥闸门）            ← 可独立更新
L0 数据层  vault-store（加密容器 + Git 版本 + 原子写）
```

## Quick Start

```bash
cargo build --release
./target/release/mindwiki init          # 创建加密知识库
./target/release/mindwiki serve         # 启动 Web 界面
```

## 依赖

上游组件通过 git 依赖引入，clone 本仓库后 `cargo build` 自动拉齐：

- [r2-agent](https://github.com/joevise/r2-agent) — Agent 运行时
- [besureAI](https://github.com/joevise/besureAI) — 加密内核（AES-256-GCM + Argon2id）

## 安全承诺

- 密钥只在内存（zeroize），永不落盘
- 解密只在受控会话，用完即焚
- 密钥闸门：一键关闭（本地/远程），关闭后密文为数学噪声
- Agent 运行于 seccomp 系统调用白名单沙箱
- 埋点只记元数据，永不记内容；默认本地，远程上报需显式开启
