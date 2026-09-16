# 文档维护与决策记录

[文档导航](README.md) · [协作规范](../AGENTS.md)

## 文档职责

| 位置 | 唯一职责 | 更新触发 |
| --- | --- | --- |
| 根 README | 产品、运行边界、最短阅读入口 | 产品范围或入口变化 |
| docs/README | 全部正式文档的导航 | 新建、移动或删除文档 |
| architecture / tech-stack | owner、依赖方向、能力档位与技术取舍 | 跨模块或依赖策略变化 |
| contracts | 路由、事件、schema 与不变量 | 对外契约或持久化语义变化 |
| apps/*、crates/* README | 本模块入口、行为边界、适配与排错 | 本模块目录或实现变化 |
| ops、migrations、ci README | 操作前提、命令、结果和失败边界 | 配置、部署、迁移或门禁变化 |
| status | 实现/验证口径和未闭合事项 | 新增可追溯运行证据 |
| .agents/notes | 决策原因、真实备选与后果 | 非平凡决定或理由翻转 |

实现事实用源码、Cargo.lock、迁移和脚本核对；用户及 AGENTS.md 的规范约束不能因源码暂未实现就静默放宽。
发现二者不一致时，明确记下实现缺口。文档描述仓库固定版本，避免把快照写成长期有效的“最新 stable”。
命令中的数据库驱动、Web feature、角色配置、权限与破坏性确认必须与真实入口一致。

## 决策笔记

本项目使用 `write-notes-like-deepseek` 的路径与格式，重要改动必带笔记；机械排版、错别字等局部修改不新建笔记。
先检索，已有决定只修事实；决定或理由翻转时新建并互链，不能覆盖旧理由。

```bash
rg --hidden --glob '!.agents/notes/archived/**' '关键词' .agents/notes/
```

笔记位于 `.agents/notes/{lifecycle}/{class}/yyyy-mm-dd-topic.md`。
生命周期为 `proposed`、`implemented`、`rejected`、`archived`；分类为 `feature`、`bug-fix`、
`simplification`、`architecture`、`process`、`testing`。只创建用到的目录，不建笔记 INDEX。

前三行固定为标题、空行、状态；正文使用中文，格式标题保留技能要求的名称：

```markdown
# Agent Note: 决策标题

Status: implemented

## Problem
问题与约束。

## Decision
当前采用的边界与理由。

## Alternatives considered
真实权衡过的选项、各自优势与未采用原因。

## Consequences
收益、代价和仍缺少的验证。
```

新建前按关键词审计全部活跃笔记，将命中项分类为无关、部分重叠、完全吸收或过时提案，并在新笔记记录结果。
部分重叠互链；过时提案 reject 或删除，不进入归档。落地时同时迁移路径、状态和正文为现在时，移除计划性标题。
归档仅用于已落地且参考价值降低的笔记，必须冻结内容并建立可核验的封印；当前仓库尚无归档笔记。

本轮决定见[全库文档重建](../.agents/notes/implemented/process/2026-09-16-documentation-rebuild.md)。

## 静态检查与人工核对

`just docs-check` 使用仓库 Rust 工具检查本地 Markdown 链接及标题锚点、从根 README 可达的文档覆盖、
Cargo 包 README，以及笔记路径/状态/必备章节。它不访问网络，不启动产品，不执行行为测试。
脚本与检查限制见 [ci/README.md](../ci/README.md)。

人工审阅仍须核对：命令是否符合实际 CLI，字段是否来自正确 owner，Lite 是否误称完整档，历史记录是否误称当前证据，
备选理由是否真实，后果是否同时写出代价和收益。格式检查不能证明这些语义。
