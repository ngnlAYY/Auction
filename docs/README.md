# 文档导航

[README.md](../README.md) · [AGENTS.md](../AGENTS.md) · [CONTRIBUTING.md](../CONTRIBUTING.md)

## 入门

| 文档 | 内容 |
| --- | --- |
| [docs/architecture.md](architecture.md) | 进程模型、分层与 crate 职责、共享 helper 归属、一致性模型、关键数据流、安全边界 |
| [docs/business-rules.md](business-rules.md) | 场次生命周期与排期、投稿、用户、竞价、代理、标记、结算与恢复摘要 |
| [docs/development.md](development.md) | 环境、常用命令与静态范围、本地运行、代码约定、变更流程 |

## 规划

| 文档 | 内容 |
| --- | --- |
| [docs/plans/architecture-improvement.md](plans/architecture-improvement.md) | 架构改进回顾：目标、原则、结果摘要与验证边界；用户决定见[decisions.md](plans/architecture-improvement/decisions.md) |

## 业务指南

| 文档 | 内容 |
| --- | --- |
| [docs/guide/player-guide.md](guide/player-guide.md) | 面向玩家：注册、出价、代理、寄售、付款 |
| [docs/guide/settlement.md](guide/settlement.md) | 金额计算、义务、履约、投递、恢复、Bot 内部履约与资金账户 |
| [docs/guide/mail-flows.md](guide/mail-flows.md) | MoogleMail 收件轮询、主题路由、投稿与起拍价指令、注册换绑、出站邮件 |
| [docs/guide/forum.md](guide/forum.md) | 论坛主题同步、回帖出价语法、身份、写入安全、私信提醒 |

## 契约

| 文档 | 内容 |
| --- | --- |
| [docs/contracts/routes.md](contracts/routes.md) | 响应外壳、错误码、Public/Admin/设置/Worker control 全部路径、浏览器页面 |
| [docs/contracts/realtime.md](contracts/realtime.md) | 事件流、注册表、HTTP 回放与 WebSocket 协议 |
| [docs/contracts/database.md](contracts/database.md) | schema 身份、表分组、共有约束与变更规则 |
| [migrations/postgres/README.md](../migrations/postgres/README.md) | 迁移文件规则与索引 |

## 运维

| 文档 | 内容 |
| --- | --- |
| [ops/README.md](../ops/README.md) | 部署形态、首次部署、systemd 服务与顺序控制、反向代理、升级、监控与日志 |
| [ops/hardening.md](../ops/hardening.md) | 主机加固基线：systemd 资源上限与沙箱、SSH、防火墙、监听、自动更新窗口、时间同步、journald、PostgreSQL 小版本升级 |
| [ops/dependencies.md](../ops/dependencies.md) | 依赖与工具链升级节奏、发布冻结日历、Dependabot 只作提示、门禁与窗口流程 |
| [ops/config/README.md](../ops/config/README.md) | 配置文件结构、各区块字段、runtime 接管 |
| [ops/keys/README.md](../ops/keys/README.md) | 四个密钥域、多 key 解密、离线重加密与回执、旧密钥 12 个月保留期、Cookie 轮换 |
| [ops/deploy/RECOVERY.md](../ops/deploy/RECOVERY.md) | 备份、校验、恢复与破坏性重建流程 |
| [apps/auction-cli/README.md](../apps/auction-cli/README.md) | 运维命令行完整说明 |
| [ci/README.md](../ci/README.md) | 静态门禁、CI 作业、发布构建与制品 |
| [ci/tools/README.md](../ci/tools/README.md) | 受控工具链与构建工具的安装与校验 |

## 程序与模块

程序：[apps/auction-public-api/README.md](../apps/auction-public-api/README.md) · [apps/auction-admin-api/README.md](../apps/auction-admin-api/README.md) · [apps/auction-worker/README.md](../apps/auction-worker/README.md) · [apps/auction-server/README.md](../apps/auction-server/README.md) · [apps/auction-cli/README.md](../apps/auction-cli/README.md)

领域与应用：[crates/auction-domain/README.md](../crates/auction-domain/README.md) · [crates/auction-application/README.md](../crates/auction-application/README.md) · [crates/auction-contracts/README.md](../crates/auction-contracts/README.md)

适配器：[crates/auction-storage/README.md](../crates/auction-storage/README.md) · [crates/auction-integrations/README.md](../crates/auction-integrations/README.md) · [crates/auction-http/README.md](../crates/auction-http/README.md) · [crates/auction-jobs/README.md](../crates/auction-jobs/README.md) · [crates/auction-setup/README.md](../crates/auction-setup/README.md)

基础设施：[crates/auction-config/README.md](../crates/auction-config/README.md) · [crates/auction-observability/README.md](../crates/auction-observability/README.md) · [crates/auction-process/README.md](../crates/auction-process/README.md)

前端：[crates/auction-web-shared/README.md](../crates/auction-web-shared/README.md) · [crates/auction-web-public/README.md](../crates/auction-web-public/README.md)（[crates/auction-web-public/DESIGN.md](../crates/auction-web-public/DESIGN.md)） · [crates/auction-web-admin/README.md](../crates/auction-web-admin/README.md)（[crates/auction-web-admin/DESIGN.md](../crates/auction-web-admin/DESIGN.md)）

## 文档约定

- 全部文档使用中文；代码标识、协议字段、命令保持原文。
- Markdown 链接目标相对当前文档所在目录，必要时使用 `../`，同页链接使用 `#标题锚点`，确保 GitHub 与本地渲染可点击。正文及链接显示文字中的仓库路径仍以仓库根为起点（如 `docs/contracts/routes.md`、`crates/auction-http/src/assets/routes.rs`），显示文字不必与链接目标相同。
- 代码块中的命令以仓库根目录为工作目录（运维手册中明确在发布包目录执行的安装命令除外）；部署相关的路径（如 `/etc/auction`、配置目录内的 `runtime/admin.toml`、发布包内的 `bin/`）描述的是目标机器或发布包中的位置，不是仓库路径。
- 每项事实只在一个文档中详细描述（通常是最接近代码的模块 README 或契约文档），其他文档摘要并链接。
- 修改代码时同步相关文档；`just verify` 包含 `markdown-links`，默认离线检查相对链接与同页、跨页锚点（口径见 [ci/checks/README.md](../ci/checks/README.md)），门禁通过不代表语义仍然正确，移动或重命名时仍须人工核对表述。
