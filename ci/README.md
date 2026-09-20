# 静态验证、文档门禁与发布脚本

[文档导航](../docs/README.md) · [开发指南](../docs/development.md) · [部署手册](../ops/deploy/README.md)

## 本地与 CI 入口

| 范围 | 本地入口 | 实现或工作流 |
| --- | --- | --- |
| 全部默认静态验证 | `just verify` | [Justfile](../Justfile) |
| 文档、导航与笔记 | `just docs-check` | [check-docs.sh](check-docs.sh)、[Documentation 工作流](../.github/workflows/docs.yml) |
| 项目规范、布局、配置、i18n | 四项 `*-check` 配方 | [check-policy.sh](check-policy.sh) |
| 格式、工作区、后端/缓存 feature、Clippy、WASM | `just fmt/check/feature-check/lint/wasm-check` 分别调用 | [Rust 工作流](../.github/workflows/ci.yml) |
| MSRV | `cargo +1.94.1 check --locked --workspace --all-targets --all-features` | Rust 工作流独立 job |
| Web payload | `just web-build` | [web-bundle.sh](web-bundle.sh) |
| API-only/embedded 与 archive | `just build-api/build-release/verify-artifacts/release-check` 分别调用 | [release.sh](release.sh) |
| 可选单文件服务 | `just build-single`、`just release-check --single-binary` | [release-single.sh](release-single.sh) |

带斜杠的配方说明表示多个独立命令，不是一个可执行命令名。Cargo 检查使用 `--locked`。
默认图检查、独立 PostgreSQL/SQLite/Redis feature 检查及 all-features Clippy 分别覆盖不同依赖图。
Rust 工作流使用仓库固定工具链；MSRV 编译不等于功能或长期支持承诺。

## 文档检查器

[ci/docs/main.rs](docs/main.rs)装配三个静态 owner：

- [scan.rs](docs/scan.rs)：发现正式 Markdown、检查 Cargo 包 README、扫描代码围栏外的 ATX 标题和行内链接。
- [links.rs](docs/links.rs)：核对本地目标和 Markdown 标题锚点，构建从根 README 可达的文档图。
- [notes.rs](docs/notes.rs)：检查活跃笔记路径、日期形状、状态、必备章节、Git 忽略规则及 implemented 禁用计划标题。

`check-docs.sh` 先对工具执行 rustfmt 检查，再用 `rustc --edition=2024 -D warnings` 编译到被忽略的 target 目录。
工具只依赖 Rust 标准库，不使用 Node、npm、Python 库或网络，也不启动任何服务。

正式范围是根 README/AGENTS，以及 docs、apps、crates、ops、migrations、ci 和 `.agents/notes` 下的 Markdown。
本地工具目录、构建产物及机器日志不属于项目文档。所有正式文档必须能从根 README 通过本地链接到达。
当前支持行内相对链接、目录 README 和普通 ATX 标题；引用式链接需先扩展解析器，外链不联网检查。
笔记正文为中文，格式章节使用技能的英文固定名称；当前没有归档笔记，首次归档前须增加 manifest 哈希与只增不改验证。

检查器不证明源码描述、数据库事务或运维命令的业务正确性。文档重建时仍需人工核对源码、模板、CLI 和证据口径。

## 仓库策略

`check-policy.sh` 检查契约文件的 Git 规则、src 根入口和职责目录、命名与同前缀聚集；
配置示例要求每个赋值字段前紧邻中文“可选/必填”说明；五语言 Fluent 必须文件齐全、键集一致、
无重复或点号，并覆盖 Rust 中的字面量引用。

## 发布边界

普通静态 CI 不安装 dx，不构建或启动浏览器产物。Public/Admin 的 `allow_missing` 保证没有 Web 文件时也能编译；
真实内嵌资源、隔离与哈希由独立发布门禁检查，不能从 cargo check 推导。

`web-bundle.sh` 按 surface 清理中间目录，用 `wasm-release` profile 构建并固定权限/mtime，生成 payload 哈希标记。
`release.sh` 分别构建 API-only 和显式 `web-embed` 的 PostgreSQL 制品，检查实际二进制标记、对端资源缺失及 Worker 依赖，
随后按白名单、权限、统一时间与两次相同 archive 装包，保存内外 SHA-256 和依赖摘要。
源码身份在构建前冻结并在阶段边界复核，正式 package 拒绝脏工作树。工具依赖和实际参数以脚本为准。

默认发布包的可执行文件仅包含 `auction-public-api`、`auction-admin-api`、`auction-worker` 和
`auctionctl`，并携带 PostgreSQL 五字段敏感模板。首次部署通过 Admin 自带的独立设置入口生成配置，
再由 `auctionctl deployment-check --strict-permissions` 预检，通过 `auctionctl deployment start` 在
三角色停机门禁通过后执行迁移、顺序启动并检查 readiness；也可先用受保护的 `auctionctl migrate` 单独迁移。
完整参数见[部署手册](../ops/deploy/README.md#启动顺序)。这是分体总控的显式运维行为，不是角色普通启动或
合并 `auction-server` 普通启动的隐式迁移；CI 不启动设置页或连接真实数据库。

`--single-binary` 变体先通过原分体门禁，再验证 `auction-server` 同时包含两个 payload 标记、
独立 `auctionctl` 不含 Web 且不进入 server 依赖图。合并归档只含这两个二进制及合并 unit，
输出 `target/release-package-single/`，仍受源码指纹、权限、白名单与干净工作树限制。
`just build-single` 仅构建及核验，可在开发工作树执行，不将其当作正式归档。

## 验证限制

所有上述检查均不运行单元、集成、功能、端到端或浏览器测试，已有测试只在 all-targets 检查时编译。
默认不扩展或执行行为测试，也不运行手工冒烟。授权恢复测试后，SQLite 只验证 Lite 范围；
PostgreSQL 的行锁、隔离与并发必须在相应后端验证。剩余运行证据见[状态页](../docs/status.md)。
