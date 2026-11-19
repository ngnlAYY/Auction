# HV Auction

HentaiVerse 装备拍卖服务：后端与默认前端用 Rust 实现，玩家在网站或 E-Hentai 论坛出价，通过游戏内 MoogleMail 寄售装备、注册账号、付款与收货；运营人员在管理后台处理场次、凭据、投递与结算。

- **后端**：三个可独立部署的进程——Public API、Admin API、Worker——加运维命令行 `auction-cli`；也可用 `auction-server` 单文件合并运行。
- **前端**：默认是 Public 与 Admin 两个 Dioxus 0.7 CSR（WASM）单页应用，内嵌在对应后端二进制中同源提供；五种界面语言。`web/` 下另有 TypeScript + SolidJS 实现（目前只有 Public），由 `AUCTION_WEB_STACK=solid` 选择、`web-solid` 内嵌；正式发布仍只产出 Rust 版，见 [web/README.md](web/README.md)。
- **存储**：PostgreSQL 18 是唯一事实来源。
- **平台**：原生程序仅支持 Linux x86_64。

## 从哪里开始

| 我想… | 阅读 |
| --- | --- |
| 了解怎么参与拍卖 | [docs/guide/player-guide.md](docs/guide/player-guide.md) |
| 理解系统设计 | [docs/architecture.md](docs/architecture.md)、[docs/business-rules.md](docs/business-rules.md) |
| 开发与提交代码 | [docs/development.md](docs/development.md)、[AGENTS.md](AGENTS.md)、[CONTRIBUTING.md](CONTRIBUTING.md) |
| 部署与运维 | [ops/README.md](ops/README.md)、[ops/config/README.md](ops/config/README.md)、[apps/auction-cli/README.md](apps/auction-cli/README.md)、[ops/deploy/RECOVERY.md](ops/deploy/RECOVERY.md) |
| 查接口与数据 | [docs/contracts/routes.md](docs/contracts/routes.md)、[docs/contracts/realtime.md](docs/contracts/realtime.md)、[docs/contracts/database.md](docs/contracts/database.md) |
| 浏览全部文档 | [docs/README.md](docs/README.md) |

## 程序

| 程序 | 职责 | 默认监听 |
| --- | --- | --- |
| [apps/auction-public-api/README.md](apps/auction-public-api/README.md) | 公开 API、竞价命令、会话、WebSocket 推送与公开页面 | `127.0.0.1:8090` |
| [apps/auction-admin-api/README.md](apps/auction-admin-api/README.md) | 管理 API、管理后台页面、首次设置 | `127.0.0.1:8091` |
| [apps/auction-worker/README.md](apps/auction-worker/README.md) | 生命周期、结算、履约、投递、邮件、论坛、恢复与告警 | 可选 Unix control socket（默认关闭） |
| [apps/auction-server/README.md](apps/auction-server/README.md) | 可选：单文件监管三个角色子进程 | 同 Public/Admin |
| [apps/auction-cli/README.md](apps/auction-cli/README.md) | 配置检查、部署编排、迁移、备份恢复与业务运维 | — |

普通启动从不隐式迁移数据库；升级迁移、备份、恢复与重建通过 `auction-cli` 显式执行。合并部署另有首次设置例外：网页明确确认后，`auction-server` 初始化专用空库；常规重启不会重试迁移。

## 仓库结构

| 路径 | 内容 |
| --- | --- |
| `apps/` | 五个可执行程序（薄装配根） |
| `crates/` | 领域、应用、适配器、契约与前端 crate，职责见[docs/architecture.md#分层](docs/architecture.md#分层) |
| `migrations/postgres/` | 只追加的数据库迁移 |
| `docs/` | 架构、业务规则、契约与指南 |
| `ops/` | 部署手册、配置示例、systemd/tmpfiles/Caddy 模板、保留政策 |
| `ci/` | 静态门禁、发布脚本与工具说明 |
| `mise.toml`、`mise.lock` | 工具版本与常用命令入口（[mise](https://mise.jdx.dev)） |
| `rust-toolchain.toml` | 固定工具链（1.100.0，含 wasm32 目标）；MSRV 与它相同 |

## 常用命令

```bash
mise install                 # 按 mise.toml 与 mise.lock 安装全部工具（首次会从源码编译 dx，较慢）
mise tasks                   # 列出全部命令入口
mise run verify              # 默认静态验证：策略门禁、fmt、check、clippy、wasm（不运行测试）
mise run test                # 行为变更：默认 feature 下的纯逻辑、单元与本地替身测试
mise run test-db             # 显式 PG18 集成测试（隔离数据库与角色，不并入 verify）
mise run web-prepare         # 安装 web/ 的 bun 依赖（Solid 构建或 web 门禁前执行）
mise run web-test            # web/ 的行为测试
mise run web-build           # 构建默认的 Dioxus 页面 payload（工具见 ci/tools/README.md）
AUCTION_WEB_STACK=solid mise run web-build  # 构建 Solid Public 页面 payload
mise run web-size            # 只核对两端 Web payload 体积预算（不构建，见 ci/README.md）
mise run run-public          # 按 AUCTION_WEB_STACK（缺省 dioxus）选择内嵌页面启动；run-worker 不内嵌页面
mise run db-migrate <配置目录>
mise run build-release       # 完整发布制品（Fat LTO，较慢）
mise run release-check       # 构建、门禁与装包
```

应用 crate 不含页面资源：需要页面时先 `mise run web-build` 再以对应内嵌 feature（`web-embed` 或 `web-solid`）构建；`AUCTION_WEB_STACK` 未设置时仍是 `web-embed`。PostgreSQL 驱动无条件编译。本地环境搭建见[docs/development.md#本地运行](docs/development.md#本地运行)。

## 发布

只在 `main` 上打 `v<版本>` tag（等于 `Cargo.toml` 的 workspace version）触发 CI 发布：tag 守卫、完整门禁、发布构建、SLSA 来源证明与 GitHub Release 资产在同一条流水线完成，Release 永久保留。正式发行只产出 Rust 版页面；部署只使用这些 Release 资产，部署前运行 `mise run verify-package` 核对提交可从发布 tag 到达。手动触发的 CI 候选构建可选择 `dioxus`（缺省）或 `solid`，Solid 候选 Actions artifact 名称带 `-solid`，但候选包不可部署。合并部署包（`auction-server` + `auction-cli`）用 `mise run release-check --single-binary` 构建。详见 [ci/README.md](ci/README.md)。

## 许可

本项目以 GNU General Public License 第 3 版（SPDX：`GPL-3.0-only`）发布，全文见 [LICENSE](LICENSE)。
