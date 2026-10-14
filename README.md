# Auction Rust

Auction Rust 是面向论坛生态的全 Rust 拍卖系统，包含公开竞价页面、管理后台，以及处理拍卖生命周期、
论坛同步和 MoogleMail 的 Worker。业务规则、接口、持久化格式与部署流程由本仓库独立定义。

浏览器使用 Dioxus 客户端渲染；Public、Admin 的 WASM 产物分别内嵌到各自后端。
PostgreSQL 提供完整档，SQLite 提供单主机、低并发 Lite 子集，Redis 缓存默认关闭。

## 从哪里开始

| 任务 | 文档 |
| --- | --- |
| 玩家参与拍卖、寄售和结算 | [中文玩家使用指南](docs/player-guide.md) |
| 浏览全部项目文档 | [文档导航](docs/README.md) |
| 阅读系统与界面设计 | [系统总览](./DESIGN.md)、[Public 设计](crates/auction-web-public/DESIGN.md)、[Admin 设计](crates/auction-web-admin/DESIGN.md) |
| 理解进程、模块和数据库档位 | [架构](docs/architecture.md) |
| 准备工具链、编译与静态检查 | [开发指南](docs/development.md) |
| 选择配置与部署方式 | [配置说明](ops/config/README.md)、[部署手册](ops/deploy/README.md) |
| 对接或修改接口 | [契约导航](docs/contracts/README.md) |
| 判断当前交付与验收边界 | [实现与验证状态](docs/status.md) |
| 参与修改和维护文档 | [协作规范](AGENTS.md)、[文档维护](docs/documentation.md) |

## 运行边界

| 程序 | 职责 | 默认监听 |
| --- | --- | --- |
| [auction-public-api](apps/public-api/README.md) | Public API、会话、实时通知和公开页面 | `127.0.0.1:8090` |
| [auction-admin-api](apps/admin-api/README.md) | 管理 API、管理会话和后台页面 | `127.0.0.1:8091` |
| [auction-worker](apps/worker/README.md) | 持久化调度、生命周期、论坛、邮件和履约 | 无业务 TCP HTTP；可选 Unix control |
| [auctionctl](apps/auctionctl/README.md) | 配置诊断、迁移、systemd 总控与受控灾备 | 无监听 |
| [auction-server](apps/server/README.md) | 可选单文件 supervisor，自管上述三角色子进程 | 沿用 Public/Admin 两个入口 |

三个服务独立装配，通过应用端口共享用例；完整 owner 与数据库能力见[架构](docs/architecture.md)。

## 构建与检查

仓库工具链固定在 [rust-toolchain.toml](rust-toolchain.toml)，依赖解析固定在 [Cargo.lock](Cargo.lock)。
从仓库根目录执行：

```bash
# 完整静态验证，不执行行为测试；仅改文档可先运行 just docs-check。
just verify

# PostgreSQL 完整档的 API-only 制品。
just build-api

# 包含 Public/Admin 页面；需要 dx CLI，详见开发指南。
just build-release
just verify-artifacts

# 可选单文件发布：auction-server + 独立 auctionctl；正式归档要求干净工作树。
just release-check --single-binary
```

`apps/*` 的默认 Cargo feature 不包含数据库驱动或 Web 资源。自行运行 `cargo` 时必须明确选择
`postgres` 或 `sqlite`；需要页面时还要先构建前端并启用 `web-embed`。不能把无驱动编译成功当成
可连接数据库的部署制品。

运行前按[配置说明](ops/config/README.md)准备公共、角色和权限受限的敏感 TOML，替换示例占位值。
分体进程用 `AUCTION_CONFIG_FILE` 选择角色文件；合并服务用 `--config-dir`，拒绝该环境变量。
首次设置、严格停机预检、迁移及启动步骤见[部署手册](ops/deploy/README.md)；合并服务的确认后
空库初始化见 [auction-server](apps/server/README.md)。普通角色或合并服务启动不隐式迁移。

## 当前交付边界

项目处于功能与运行证据收口阶段。路由注册、源码和静态编译不构成生产验收；真实外部服务、
非空生产数据、完整浏览器流程及恢复仍需证据。[实现与验证状态](docs/status.md)统一登记
当前能力、历史检查结果、有限部署证据及未闭合事项。
生命周期、恢复、Worker 连续处理与灾备门禁的审计修复仍属源码交付；不自动修复既有异常数据，也不替代行为验收。

除非用户明确要求，不新增或执行单元、集成、功能、端到端、浏览器测试及手工功能冒烟；已有测试保留。
