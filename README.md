# Auction Rust

面向论坛生态的全 Rust 拍卖系统：公开竞价、管理后台，以及处理生命周期、论坛和 MoogleMail 的 Worker。浏览器为 Dioxus CSR，Public/Admin
WASM 分别内嵌；数据库仅 PostgreSQL，Redis 默认关闭。原生程序仅支持 **Linux x86_64**，正式发布 GNU/glibc；语义由本仓库独立定义。
## 从哪里开始
| 任务 | 入口 |
| --- | --- |
| 玩家注册、寄售、竞价、付款或返还 | [中文玩家指南](docs/guide/player-guide.md) |
| 系统设计、接口、契约与页面 | [文档导航](docs/README.md)、[设计与架构](docs/design/architecture.md)、[契约](docs/contracts/README.md) |
| 开发、编译、静态检查与发布 | [开发指南](docs/guide/development.md)、[CI 说明](ci/README.md) |
| 配置、部署、备份恢复、排错 | [运维入口](ops/README.md) |
| 实现范围、历史验证与未验收缺口 | [状态页](docs/status.md)、[AGENTS.md](AGENTS.md)、[文档维护](docs/guide/development.md#文档维护) |

## 目录和根文件有什么用
`apps/` 是薄装配根，`crates/` 按[owner](docs/design/architecture.md#crate-所有权)划分实现；[docs/](docs/README.md) 导航正式文档，
[ops/](ops/README.md) 管配置与部署，[ci/](ci/README.md) 管检查与发布，[migrations/](migrations/postgres/README.md) 只追加迁移。根文件
[Cargo.toml](Cargo.toml) / [Cargo.lock](Cargo.lock) 管工作区、依赖、MSRV 与 profile，[rust-toolchain.toml](rust-toolchain.toml) 固定工具链、WASM 目标，
[Justfile](Justfile) 是命令入口，[rustfmt.toml](rustfmt.toml)、[.editorconfig](.editorconfig) 定义格式，[AGENTS.md](AGENTS.md) 是协作约束，
唯一 CI 工作流是 [.github/workflows/ci.yml](.github/workflows/ci.yml)。
## 运行边界
| 程序 | 职责 | 默认监听 |
| --- | --- | --- |
| [auction-public-api](apps/auction-public-api/README.md) | Public API、会话、实时通知和公开页面 | `127.0.0.1:8090` |
| [auction-admin-api](apps/auction-admin-api/README.md) | 管理 API、会话、后台及独立设置入口 | `127.0.0.1:8091` |
| [auction-worker](apps/auction-worker/README.md) | 生命周期、论坛、邮件与履约 | 无业务 TCP HTTP；可选 Unix control |
| [auctionctl](apps/auctionctl/README.md) | 配置、迁移、systemd 总控与受控灾备 | 无监听 |
| [auction-server](apps/auction-server/README.md) | 可选单文件 supervisor，自管三角色子进程 | 沿用 Public/Admin 两个入口 |
默认分体发布；合并模式仅用于 Linux/PostgreSQL 单主机，auctionctl 仍独立。角色文件用 AUCTION_CONFIG_FILE 选择，合并服务用
--config-dir 并拒绝该环境变量；配置分层、权限、首次设置及停机迁移见[运维手册](ops/README.md)，普通启动不隐式迁移。论坛新主题目录
每帖最多 150 件并连续分页、更新楼位于目录之后，首帖倒计时图片由本站 Rust Public 服务生成且只展示计划时间；账户“竞价警告”默认
MM 或论坛 PM 并按领先周期跨渠道去重，受控升级须追加 `000042`；入口支持 Cloudflare 橙云 → Caddy HTTPS → Unix socket，仅静态资源
允许 CDN 缓存，细节见[玩家指南](docs/guide/player-guide.md)与[部署约束](ops/README.md#cloudflare-整站代理)及[技术栈](docs/design/tech-stack.md)。

## 构建与检查
从仓库根目录执行，工具准备见[开发指南](docs/guide/development.md)：
```bash
just verify                       # 完整静态检查，不运行行为测试
just build-api                    # PostgreSQL API-only 制品
just build-release                # 包含两端页面，需要配套 dx CLI
just verify-artifacts             # 资源、依赖和隔离门禁
just release-check --single-binary # 可选合并归档，要求干净工作树
```
apps 默认不含数据库驱动或 Web，自行调用 cargo 须显式选 postgres，需要页面还须先构建并选 web-embed；后端 release 保留 Fat LTO、
单 CGU、panic 展开与通用 CPU 基线，WASM 优先体积，参数不是性能验收。默认分支推送或手动运行通过 CI 后可下载四程序包，
见[CI 制品说明](ci/README.md#下载-actions-制品)。
## 当前交付边界
- 公开目录的稀有/融合分组、筛选和分页由[装备分组契约](docs/contracts/routes.md#p09p11-装备分组)定义；
  名称拆分、详情类型及历史快照兼容由[装备录入契约](docs/contracts/invariants.md#i-05a-admin-装备录入的敏感边界和原子集合)定义。
- 新场次软截止窗口/延长默认均为 900 秒，不改既有显式配置或冻结快照；
  老配置与草稿的处理见[截止升级说明](ops/README.md#截止规则与邮件发送隔离升级)。
- MoogleMail 在发送 fence 前只读检查草稿；Bot 内部履约依赖联合身份、正向库存、资金账本及统一现金发送门。
  启用前须[停机同版升级并显式对账](ops/README.md#bot-内部履约与资金保护升级)；旧未知投递仍人工核对，税与内部留存不自动再投资。

源码、路由和静态编译不构成生产验收。[状态页](docs/status.md)分别登记实现、历史证据与缺口；
真实服务、非空数据、并发、浏览器和恢复流程须另行授权验证。默认不新增或执行行为测试，已有测试保留。
