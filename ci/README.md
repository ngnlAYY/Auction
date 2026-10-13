# 静态验证、文档门禁与发布脚本

[文档导航](../docs/README.md) · [开发指南](../docs/development.md) · [部署手册](../ops/deploy/README.md)

## 本地与 CI 入口

| 范围 | 本地入口 | 实现/工作流 |
| --- | --- | --- |
| 全部默认静态验证 | `just verify` | [Justfile](../Justfile) |
| 文档链接、锚点和导航 | `just docs-check` | [check-docs.sh](check-docs.sh)、[docs.yml](../.github/workflows/docs.yml) |
| 仓库、布局、配置、i18n | 对应四项 `*-check` | [check-policy.sh](check-policy.sh) |
| 格式、编译、feature、Clippy、WASM | `just fmt`、`check`、`feature-check`、`lint`、`wasm-check` | [ci.yml](../.github/workflows/ci.yml) |
| 依赖安全公告 | `just dependency-audit` | 独立 security-audit job，cargo-audit 0.22.2 |
| MSRV | `cargo +1.94.1 check --locked --workspace --all-targets --all-features` | 独立 msrv job |
| Web payload | `just web-build` | [web-bundle.sh](web-bundle.sh) |
| 分体构建与归档 | `just release-check` | [release.sh](release.sh) |
| 可选合并服务 | `just build-single`、`just release-check --single-binary` | [release-single.sh](release-single.sh) |

Cargo 检查使用 --locked；默认图、独立 PostgreSQL/SQLite/Redis 图和 all-features Clippy 分开核验。
Rust job 使用固定工具链，MSRV 编译不表示功能验收或长期支持承诺。
依赖审计独立联网读取 RustSec 和 Cargo.lock，不改锁图、不运行产品；本地 verify 不含它。
`cargo audit --deny yanked --deny unsound` 阻断漏洞、撤回和 unsound，停维提示保留且无公告忽略列表。
历史 fxhash 维护债见[技术栈](../docs/tech-stack.md#版本与升级)。

## 文档检查器

[docs/main.rs](docs/main.rs)装配 [scan.rs](docs/scan.rs) 的 Markdown/包 README 扫描与
[links.rs](docs/links.rs) 的本地目标、锚点和可达图核验。
check-docs.sh 先检查 rustfmt，再以 rustc edition 2024、-D warnings 编译至忽略的 target；仅使用标准库，无网络/服务。

正式范围为根 README/AGENTS/DESIGN 及 docs、apps、crates、ops、migrations、ci 下 Markdown，
所有正式文档须从根 README 可达。本地工具、构建产物和机器日志不属于文档契约。
支持代码围栏外 ATX 标题、行内相对链接和目录 README；引用式链接需扩展解析器，外链不联网检查。
通过此门禁只证明结构与链接，源码描述、CLI 和历史证据仍需人工核对。

## 仓库策略

check-policy.sh 检查契约文件 Git 规则、src 根入口/职责目录、命名与同前缀聚集；
TOML 示例每个赋值字段须紧邻中文可选/必填注释，Fluent 五语言须文件/键集齐全、无重复或点号并覆盖字面量引用。

## 发布边界

普通 CI 不安装 dx、不构建页面。allow_missing 只保证缺 Web 时能编译，真实内嵌资源由发布门禁证明。
web-bundle.sh 按 surface 清理中间目录，以 wasm-release 构建，固定权限/mtime 并生成 payload 哈希。
release.sh 分别构建 API-only 和 web-embed 的 PostgreSQL 制品，核验实际二进制标记、对端资源缺失与 Worker 依赖。
源码身份在构建前冻结、阶段边界复查；归档校验白名单、权限、统一时间、两次相同 archive、内外哈希和依赖摘要。
正式 package 要求干净工作树。

分体可执行白名单为 Public、Admin、Worker、auctionctl，只有两端内嵌各自页面。
--single-binary 先通过分体门禁，再核对 server 的两个 payload 和独立无 Web 的 auctionctl，
归档只含这两个程序及合并 unit，输出 target/release-package-single/。
build-single 仅构建/核验，可在开发工作树执行。目录、安装、配置和迁移统一见[部署手册](../ops/deploy/README.md)。

## 验证限制

默认只进行静态检查；all-targets 只编译已有测试，不新增或执行单元、集成、功能、端到端、浏览器测试或手工冒烟。
获明确授权后才能恢复行为验证，Lite 不能替代 PostgreSQL 行锁、隔离和并发证据；当前缺口见[状态页](../docs/status.md)。
