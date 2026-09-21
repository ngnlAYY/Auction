# Auction Rust

Auction Rust 是面向论坛生态的全 Rust 拍卖系统，包含公开竞价页面、管理后台，以及处理拍卖生命周期、
论坛同步和 MoogleMail 的 Worker。业务规则、接口、持久化格式与部署流程由本仓库独立定义。

浏览器使用 Dioxus 客户端渲染；Public、Admin 的 WASM 产物分别内嵌到各自后端。
PostgreSQL 提供完整档，SQLite 提供单主机、低并发 Lite 子集，Redis 缓存默认关闭。

## 从哪里开始

| 任务 | 文档 |
| --- | --- |
| 浏览全部项目文档 | [文档导航](docs/README.md) |
| 阅读系统设计总览 | [DESIGN.md](DESIGN.md) |
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

三个服务独立装配，领域和用例通过窄端口共享。浏览器只消费稳定 DTO，数据库记录不直接进入页面。
完整目录与 owner 见[架构](docs/architecture.md)。

## 构建与检查

仓库工具链固定在 [rust-toolchain.toml](rust-toolchain.toml)，依赖解析固定在 [Cargo.lock](Cargo.lock)。
从仓库根目录执行：

```bash
# 列出任务；以下验证不执行行为测试。
just
just docs-check
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

运行配置分为公共、角色和敏感 TOML；分体进程的 `AUCTION_CONFIG_FILE` 只选择角色文件。
合并 `auction-server` 使用 `--config-dir`，若检测到该环境变量则拒绝启动。
内嵌 PostgreSQL 制品可通过 `auction-admin-api setup --output-dir <绝对路径>` 打开本机首次设置页，
分别填写数据库主机、端口、账号、密码、库名，以及管理员和部署参数并生成配置；完成后按[部署手册](ops/deploy/README.md)执行严格预检、显式迁移和启动。
提交的示例包含占位值，使用前按[配置说明](ops/config/README.md)准备权限受限的真实敏感文件。
首次迁移、启动与停机要求见[部署手册](ops/deploy/README.md)，不在普通静态验证中自动执行。
可选 `auction-server --config-dir <配置目录>` 在目录不存在时进入同一网页向导，经明确确认后
初始化专用空 PostgreSQL 库并启动三角色；普通启动不迁移。显式远程 HTTPS 设置和合并部署维护
见 [auction-server](apps/server/README.md)，默认分体发布不变。

## 当前交付边界

源码已登记 Public 21 条、Admin 43 条 feature API，以及 Public 15 条、Admin 21 条浏览器路由。
PostgreSQL 完整档与 SQLite Lite 的具体差异见[能力矩阵](docs/architecture.md#部署档位与能力)。
这些数量描述注册范围，不证明运行验收。

项目处于功能与运行证据收口阶段。既有文档报告过部分隔离环境验证；真实外部服务、非空生产数据、
目标主机切换和完整浏览器流程仍需要可追溯证据。不能宣称生产验收完成。
当前文档核对范围、首次设置与迁移入口的实现缺口及证据口径见[实现与验证状态](docs/status.md)。

除非用户明确要求，不新增或执行单元、集成、功能、端到端、浏览器测试及手工功能冒烟；已有测试保留。
