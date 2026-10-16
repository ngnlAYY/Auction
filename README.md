# Auction Rust

面向论坛生态的全 Rust 拍卖系统：公开竞价、管理后台，以及处理生命周期、论坛和 MoogleMail 的 Worker。
浏览器为 Dioxus CSR，Public/Admin WASM 分别内嵌；数据库仅 PostgreSQL，Redis 默认关闭。
原生程序仅支持 **Linux x86_64**，正式发布 GNU/glibc；浏览器 WASM 保留。
业务、接口、数据及部署语义由本仓库独立定义。

## 从哪里开始

| 任务 | 入口 |
| --- | --- |
| 玩家参与拍卖、寄售、结算 | [中文玩家指南](docs/player-guide.md) |
| 理解系统、接口或页面设计 | [文档导航](docs/README.md)、[设计与架构](docs/architecture.md)、[契约](docs/contracts/README.md) |
| 开发、编译、静态检查 | [开发指南](docs/development.md)、[CI 说明](ci/README.md) |
| 配置、部署、备份恢复、排错 | [运维入口](ops/README.md) |
| 判断实现与验收边界 | [状态页](docs/status.md) |
| 参与修改与文档维护 | [AGENTS.md](AGENTS.md)、[文档维护](docs/development.md#文档维护) |

## 目录和根文件有什么用

| 位置 | 用途 |
| --- | --- |
| `apps/` / `crates/` | 程序装配 / 按 owner 划分的实现，见[架构](docs/architecture.md#crate-所有权) |
| [docs/](docs/README.md) | 玩家指南、设计、开发、契约与验证状态 |
| [ops/](ops/README.md) | 配置示例、部署模板、恢复及诊断操作 |
| [ci/](ci/README.md) / [.github/workflows/ci.yml](.github/workflows/ci.yml) | 检查和发布实现 / 唯一 CI 工作流 |
| [migrations/](migrations/README.md) | 追加式数据库迁移；历史文件不能清理 |
| [Cargo.toml](Cargo.toml) / [Cargo.lock](Cargo.lock) | 工作区成员、直接依赖、MSRV、构建 profile / 精确依赖解析；锁文件不能手工瘦身 |
| [rust-toolchain.toml](rust-toolchain.toml) | 固定开发工具链、组件及 WASM 目标，与 MSRV 含义不同 |
| [rustfmt.toml](rustfmt.toml) / [.editorconfig](.editorconfig) | Rust 格式 / 跨编辑器格式；Clippy 的 MSRV 读取 Cargo 包元数据 |
| [Justfile](Justfile) | 开发命令入口，`just` 列出全部任务；检查参数统一在 ci/check.sh |
| [.gitignore](.gitignore) / [AGENTS.md](AGENTS.md) | 排除产物与私密文件 / 协作和交付约束 |

依赖版本、升级理由和历史审计集中在[技术栈](docs/tech-stack.md)，不再单设 clippy.toml 或根 DESIGN.md。

## 运行边界

| 程序 | 职责 | 默认监听 |
| --- | --- | --- |
| [auction-public-api](apps/public-api/README.md) | Public API、会话、实时通知和公开页面 | `127.0.0.1:8090` |
| [auction-admin-api](apps/admin-api/README.md) | 管理 API、会话、后台及独立设置入口 | `127.0.0.1:8091` |
| [auction-worker](apps/worker/README.md) | 生命周期、论坛、邮件与履约 | 无业务 TCP HTTP；可选 Unix control |
| [auctionctl](apps/auctionctl/README.md) | 配置、迁移、systemd 总控与受控灾备 | 无监听 |
| [auction-server](apps/server/README.md) | 可选单文件 supervisor，自管三角色子进程 | 沿用 Public/Admin 两个入口 |

默认分体发布；合并模式仅用于 Linux/PostgreSQL 单主机，auctionctl 仍独立。
角色文件用 AUCTION_CONFIG_FILE 选择；合并服务用 --config-dir，拒绝该环境变量。
配置分层、权限、首次设置及停机迁移统一见[运维手册](ops/README.md)。普通启动不隐式迁移。
新论坛主题目录每帖最多 150 件并连续分页，系统更新楼位于目录之后；历史主题保持原楼层。
楼层和错误提示见[玩家指南](docs/player-guide.md#论坛出价与处理结果)。
支持 Cloudflare 橙云 → Caddy HTTPS → Public/Admin Unix socket；仅静态资源允许 CDN 缓存，
API、私有数据与 HTML 禁止缓存。证书、可信代理及缓存规则见[部署约束](ops/README.md#cloudflare-整站代理)。

## 构建与检查

从仓库根目录执行，工具准备见[开发指南](docs/development.md)：

```bash
just verify                       # 完整静态检查，不运行行为测试
just build-api                    # PostgreSQL API-only 制品
just build-release                # 包含两端页面，需要配套 dx CLI
just verify-artifacts             # 资源、依赖和隔离门禁
just release-check --single-binary # 可选合并归档，要求干净工作树
```

apps 默认不含数据库驱动或 Web；自行调用 cargo 时须显式选 postgres，需要页面还须先构建并选 web-embed。
后端 release 保留 Fat LTO、单 CGU、panic 展开与通用 CPU 基线，WASM 优先体积；参数不是性能验收。
默认分支推送或手动运行通过 CI 后，可从 Actions 下载含双端页面的通用 Linux x86_64 四程序包；
触发条件、缓存和下载位置见[CI 制品说明](ci/README.md#下载-actions-制品)，不自动部署。

## 当前交付边界

路由、源码和静态编译不构成生产验收；真实外部服务、非空生产数据、完整浏览器流程及恢复仍需证据。
[状态页](docs/status.md)登记源码能力、历史结果和缺口；修复不会自动更正既有异常数据。
除非用户明确要求，不新增或执行单元、集成、功能、端到端、浏览器测试及手工冒烟；已有测试保留。
