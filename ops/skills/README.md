# 代理运维技能

[ops/README.md](../README.md) · [ops/deploy/MIGRATION-DRILL.md](../deploy/MIGRATION-DRILL.md) · [ops/deploy/RECOVERY.md](../deploy/RECOVERY.md) · [AGENTS.md](../../AGENTS.md)

本目录保存两份由实际运维沉淀下来的代理技能（`SKILL.md`，带 `name`/`description` 头）。它们记录本地重建与线上部署的操作顺序、证据要求和踩过的坑；规则本身仍以[ops/README.md](../README.md)、[ops/deploy/MIGRATION-DRILL.md](../deploy/MIGRATION-DRILL.md) 与[ops/deploy/RECOVERY.md](../deploy/RECOVERY.md) 为准，冲突时以这些文档为准并更新技能。

| 技能 | 用途 |
| --- | --- |
| [auction-local-rebuild](auction-local-rebuild/SKILL.md) | 把本地隔离环境的 Public/Admin 更新到指定提交：冻结源码、隔离 worktree 构建、恢复点、迁移、切换、验收与记录 |
| [auction-production-deploy](auction-production-deploy/SKILL.md) | 线上分体部署：取 GitHub Release 资产并校验来源、隔离迁移演练与回执、`auction-cli deploy`、部署后核对与记录 |

## 脱敏与本机副本

仓库是公开的。这里的版本已把主机地址、SSH 入口、域名、本机绝对路径、端口、运行账号与 systemd unit 名替换成占位符：

| 占位符 | 含义 |
| --- | --- |
| `<生产 SSH 入口>` | 登录生产主机的 SSH 地址与账号 |
| `<应用根>` | 生产主机上的应用根目录（含 `bin/`、`config/`、`state/`、`backups/`、`run/`） |
| `<运行账号>` | 三个角色的运行账号 |
| `<Worker unit>`、`<Admin unit>`、`<Public unit>` | 三个角色的 systemd unit 名 |
| `<Public 站点>`、`<Admin 站点>` | 两个对外 HTTPS 站点 |
| `<本地环境根>` | 本地隔离环境目录（含 `bin/`、`config/`、`state/`、`backups/` 与环境 `README.md`） |
| `<云端配置副本>` | 本机保存的线上配置副本，只供演练派生隔离配置 |
| `<演练根>` | 存放各轮 `deploy-drill-<时间戳>/` 目录的本机目录 |
| `<本地 PG>`、`<本地库>` | 本地隔离 PostgreSQL 18 的回环地址:端口与库名 |
| `<Public 端口>`、`<Admin 端口>`、`<局域网地址>` | 本地环境的入口端口与局域网地址 |

带真实值的原版只放在维护者本机：项目内的 `.agents/skills/`（已被 `.gitignore` 忽略，omp 与 Codex 在本项目中会自动加载）或用户级技能目录。修改流程时两份同步更新；真实值只改本机原版，不得写回本目录。
