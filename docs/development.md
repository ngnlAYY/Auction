# 开发与静态验证

[文档导航](README.md) · [协作规范](../AGENTS.md) · [部署](../ops/README.md) · [CI](../ci/README.md)

## 工具与依赖来源

准备 Linux x86_64 GNU/glibc 主机、just、Bash、rustfmt、Clippy 与 `wasm32-unknown-unknown` target；
页面另需配套 dx CLI。版本、MSRV 和 profile 见[技术栈](tech-stack.md)。
检查/发布使用 `--locked`，不手工清理条件平台或未启用可选依赖的锁记录。
不设全局 `build.target`，保留 `target/release` 路径；浏览器显式选 WASM target，通用制品禁用 `target-cpu=native`。

## 默认检查

```bash
just verify
```

分项顺序见 [Justfile](../Justfile)，实现及 CI/MSRV 范围见[CI 说明](../ci/README.md)。
`cargo check --all-targets` 只编译已有目标；文档门禁只核验链接、锚点、导航和包 README，不证明业务正确。
未经明确要求，不新增或运行单元、集成、功能、端到端、浏览器测试及手工冒烟；保留已有测试。

依赖审计独立联网获取 RustSec，不启动产品或修改锁图：

```bash
cargo install cargo-audit --locked --version 0.22.2
cargo install cargo-machete --locked --version 0.9.2
just dependency-audit dependency-unused
```

漏洞及全部警告均阻断，不设置公告忽略项。cargo-machete 检查未用直接依赖，不代替 feature、可达性或许可评审。
历史结果见[状态页](status.md)，零公告不等于没有未知风险。

## 选择制品 feature

| 目标 | feature | 条件 |
| --- | --- | --- |
| PostgreSQL | postgres | 唯一运行后端；先准备配置与迁移 |
| Redis | cache-redis | 配置选择 Redis 并提供敏感 URL |
| 内嵌页面 | Public/Admin 的 web-embed | 先构建对应 payload |
| API-only | 不启用 web-embed | 页面路由 503，API 仍可使用 |
| 合并 supervisor | server 的 postgres；首次向导另需 web-embed | Linux 单主机，auctionctl 独立 |

apps/* 和 auction-storage 默认 feature 为空。Justfile 的 `run-*`、`db-migrate` 显式选择 `postgres`；
Public/Admin 的运行配方另启用 `web-embed`。发布配方固定 PostgreSQL。
例如 `cargo check --locked -p auction-public-api --features postgres` 只编译所选路径。

## 配置准备与显式运行

按[配置手册](../ops/config/README.md)准备公共、角色和私有敏感 TOML，核对相对路径与权限。
以下配置检查只输出安全投影、不连接数据库；示例占位值必须先替换为有效私有配置：

```bash
AUCTION_CONFIG_FILE=ops/config/examples/public.toml cargo run --locked -p auctionctl -- check-config public
```

建库、停机迁移及分体/合并启动按[部署手册](../ops/README.md)操作，本页不授予启动、迁移或功能验证授权。
普通启动不隐式迁移；例外仅限网页确认后的专用空库初始化、严格停机门禁后的部署迁移。

## 浏览器与发布构建

```bash
cargo install dioxus-cli --version 0.7.10 --locked
just web-build

# 干净工作树上的完整分体归档。
just release-check

# 可选合并服务，auctionctl 仍独立。
just build-single
just release-check --single-binary
```

Web 使用 `wasm-release`，脚本生成稳定内嵌目录与 payload 清单；dx 的 esbuild 仅供构建，不增加 Node/npm 运行链。
资源隔离、干净树及 `--allow-dirty` 限制见[发布手册](../ops/README.md#发布制品)。
[AMD KVM 候选](../ops/build/README.md)仅叠加目标白名单/并发，先独立构建 Web；
本机微基准不能替代目标机、PostgreSQL 并发或代表性 PGO 负载。

## 常见边界

| 现象 | 优先核对 |
| --- | --- |
| 配置数据库但驱动不可用 | 二进制是否启用 postgres feature |
| API 可用、页面 503 | 对应 payload 与 web-embed |
| runtime 保存但未生效 | restart_required 与受影响角色重启 |
| 配置示例被拒绝 | 占位凭据、权限、相对路径；不得放松 loader |

## 文档维护

文档按唯一 owner 维护，不为同一事实另建说明页、笔记目录或技能模板：

| 位置 | 职责与更新时机 |
| --- | --- |
| 根 README / docs/README | 产品与文件用途 / 按任务导航；新增、移动、删除时更新入口 |
| architecture / tech-stack | 系统设计、模块及能力边界 / 依赖版本、取舍与升级 |
| contracts / table-layouts | HTTP、事件、schema、不变量 / 业务表格列宽；随契约同步 |
| player-guide / 两端 DESIGN | 玩家操作流程 / 各自界面设计；不替代后端契约或合并 Public/Admin 外壳 |
| apps、crates README | 本模块职责、源码入口、限制与排错；不复制完整路由或配置表 |
| ops、migrations、ci | 配置部署、迁移、静态门禁的前提、命令、结果和失败边界 |
| status | 源码能力、历史验证及未闭合事项；不得将静态通过写成运行验收 |

实现事实核对源码、Cargo.lock、迁移和脚本；用户与 AGENTS.md 的规范不得因实现缺口被静默放宽。
系统/技术取舍、配置部署与安全限制写入对应 owner，其他文档链接，不复制平行清单。
版本描述仓库固定值，不写成永久有效的“最新 stable”；命令的 feature、配置、权限与破坏性确认须匹配真实入口。

先明确 owner、能力档位及不变量，再修改领域/端口/DTO、适配器与 HTTP/Web/Worker；同步所属契约和模块说明。
不兼容 wire 变更新增版本，迁移只追加。敏感运行证据留在受控目录，Git 保存变更历史；
历史日期、失败/成功结果和证据边界不得改写，局部排版无需新增记录。

`just docs-check` 仅检查本地 Markdown 链接、标题锚点、从根 README 可达的正式文档及 Cargo 包 README，
不访问网络、不启动产品、不执行行为测试。人工仍须核对 CLI、字段 owner、能力范围和历史证据；
解析范围见[CI 说明](../ci/README.md#文档检查器)。
