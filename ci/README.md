# CI：检查与发布

[文档导航](../docs/README.md) · [开发指南](../docs/development.md) · [部署手册](../ops/README.md)

日常只需 `just verify`；构建发布用 `just release-check`。本目录保留五个实现文件，各有明确用途：

| 文件 | 用途 |
| --- | --- |
| [check.sh](check.sh) | 本地和 CI 共用的策略、Cargo 与依赖检查命令 |
| [docs.rs](docs.rs) | 仅标准库的文档检查器；扫描、链接逻辑内聚在同一文件，不另建 Cargo 包 |
| [web-bundle.sh](web-bundle.sh) | 按 Public/Admin 构建 WASM，生成固定权限、mtime 和 payload 哈希 |
| [release.sh](release.sh) | API-only / 内嵌档构建、资源隔离、身份校验和确定性归档 |
| [release-single.sh](release-single.sh) | 由 release.sh 加载的合并服务变体，复用分体门禁；不是独立执行入口 |

## 本地与 CI 入口

| 范围 | 本地命令 | check.sh 子命令 |
| --- | --- | --- |
| 默认静态验证 | `just verify` | 与 `bash ci/check.sh all rust` 等价 |
| 文档 | `just docs-check` | docs |
| 仓库 / 布局 / 配置 / 五语言 | 四项对应 `*-check` | repository / layout / config / i18n |
| 格式 / 编译 / 独立 feature / Clippy / WASM | fmt / check / feature-check / lint / wasm-check | fmt / check / feature / lint / wasm；合称 rust |
| RustSec 公告 / 未用依赖 | dependency-audit / dependency-unused | 同名子命令；不属于默认本地 verify |
| 页面与发布 | web-build / build-api / build-release / build-single / release-check | 不由 check.sh 执行，见上方专用脚本 |

check.sh 无参数仍执行五项策略门禁（all），支持一次传入多项；已有 Justfile 命令保留。
Cargo 检查使用 --locked；默认图、独立 PostgreSQL/Redis 图、all-features Clippy 和两端 WASM 分别核验。
MSRV job 单独用 1.94.1 编译全工作区/all-targets/all-features，不代表长期支持或业务验收。
依赖 job 固定 cargo-audit 0.22.2、cargo-machete 0.9.2；audit 联网读取 RustSec，漏洞和全部警告均阻断，无忽略项。
machete 只扫描直接依赖，不替代 feature、可达性和许可审查；审计不改锁图、不运行产品。

## 工作流调度

唯一工作流 [ci.yml](../.github/workflows/ci.yml) 接收所有 push、PR 默认事件及 `workflow_dispatch` 手动触发。
默认分支从 GitHub 仓库元数据读取，不把当前分支名写死。各 job 顺序如下：

| job | 前置条件 | 职责 |
| --- | --- | --- |
| docs | 无 | 去重判断；文档、仓库/布局/配置/i18n、格式等廉价静态门禁 |
| verify | docs 通过且本轮需检查 | 工作区、独立 feature、Clippy 和双端 WASM 编译，不重复格式/策略检查 |
| security-audit | 同上 | 固定版本工具安装与完整依赖审计 |
| msrv | 同上 | 独立 MSRV 编译，与 verify/security-audit 并行 |
| artifacts | 上述四项通过；默认分支 push 或手动运行 | 完整通用分体包构建、归档校验及上传 |

非默认分支 push 仅在 GitHub API 确认**同仓库、同源提交**已有开放 PR 时跳过重复检查，改由 PR 事件检查合并结果；
API 查询失败会阻断，不假定可跳过。尚无 PR、PR 元数据尚未更新、默认分支、标签和手动运行不走该跳过分支。
读取 PR 元数据只授予 docs job `pull-requests: read`；其他 job 仅 `contents: read`，checkout 不持久化凭据。

并发组按工作流、事件类型及完整 ref/PR 编号隔离，同组新运行取消旧运行；手动任务不被自动 push 取消，
不同 fork 的同名分支也不会互相取消。所有 job 均有超时，完整构建最多 90 分钟。
保留 docs、verify、security-audit、msrv 的检查名，新增 artifacts；原 Documentation 工作流已合并，
若分支保护绑定旧工作流名，须核对 required check。

## Runner 依赖与缓存

全部 job 固定 `ubuntu-24.04`，避免 `ubuntu-latest` 切换发行版造成工具或 ABI 漂移。
docs 在运行门禁前显式安装 `ripgrep` 和 Perl；不能假设 runner 预装。缺少 `rg` 会在布局检查阶段退出 127，
尚未进入 Rust 编译；不得跳过门禁或忽略退出码。
Actions 固定完整 SHA，工具版本与取舍见[技术栈](../docs/tech-stack.md#版本与升级)。

编译和工具缓存使用固定版本的 [rust-cache](https://github.com/Swatinem/rust-cache)：

- verify、msrv、artifacts 采用 `v1-auction-build` 前缀，按 job、平台、Rust 工具链、Cargo 清单/锁图及构建配置隔离。
  缓存依赖和工作区编译产物，不跨工具链混用，也不缓存 Cargo 工具二进制。
  artifacts 使用独立 release 键，递归清理 `target` 下原生 API-only/embedded 与 WASM 缓存；不复用发布校验结论。
- security-audit 沿用含工具版本的键，缓存 Cargo 工具二进制及安装元数据。
  分别核对两个工具的精确版本；缺失、执行失败或版本不符才用 `cargo install --locked --version ... --force` 重装，
  不能仅凭整体缓存命中跳过安装判断。
- 四个 Cargo job 均启用失败时保存缓存，保留已完成的编译或工具安装；已有同键缓存不会被覆盖。
  docs 不额外引入 Cargo 缓存。
- artifacts 另缓存官方 dx 0.7.10 Linux x86_64 压缩包，避免从源码编译 CLI；下载及缓存恢复后均核对固定 SHA-256。
  此缓存只含工具归档，不缓存运行配置、凭据或下载制品。
- 缓存缺失不改变检查范围；命中后仍执行全部静态检查及审计，RustSec 仍联网更新，不缓存“通过”结论。
  release Cargo 并发固定为 2，保留 Fat LTO、单 CGU 和通用 CPU 基线，不把原生 CPU 参数传给 WASM。

缓存命中率、耗时和 runner 内存边界须以实际 Actions 运行确认，不能由 YAML 静态检查推导。

## 下载 Actions 制品

默认分支 push 或手动运行通过检查后，artifacts job 调用既有 `bash ci/release.sh release-check`，
重建双端页面、API-only 对照及 embedded 四程序，执行资源隔离、源码身份和确定性归档门禁，再上传结果。
不另建构建参数或归档白名单；feature 沿用 release.sh 的 PostgreSQL 和双端 web-embed，未额外启用 Redis。
PR、非默认分支和标签的自动运行只检查、不发布制品；手动可选择要构建的 ref，但仍须通过全部检查。

在 GitHub 的 **Actions → Rust → 对应运行 → Artifacts** 下载
`auction-linux-x86_64-<commit>-<attempt>`，运行摘要也提供链接。私有库下载需要相应仓库权限。
下载包包含 `.tar.gz`、外层 `SHA256SUMS` 和 `DEPENDENCIES.md`；先校验外层清单，再解开 tar 包。
tar 内的 `auction-rust-<version>-<commit>/bin/` 包含且仅包含：

```text
auction-public-api
auction-admin-api
auction-worker
auctionctl
```

Public/Admin 各自内嵌页面；其余配置示例、部署模板和清单沿用标准分体包布局。
先 tar 再上传保留可执行权限，Actions 外层压缩级别为 0，避免重复压缩已有 gzip；找不到上传文件时报错。
artifact 名包含提交和运行尝试编号，重跑不覆盖旧尝试；保留期 14 天，不代替长期发布存档。

这是 Ubuntu 24.04 构建的通用 Linux x86_64 GNU/glibc 包，不采用 `ops/build/ryzen-7950x.toml`，也不生成合并服务。
部署机仍须满足实际 ELF/glibc 依赖；通用 CPU 不等于任意旧 Linux 均兼容。
构建、上传不启动程序、不连接业务数据库、不自动部署，静态通过不代表生产验收。

## 文档检查器

check.sh docs 先用 rustfmt 检查，再以 rustc edition 2024、-D warnings 编译 docs.rs 至忽略的 target。
正式范围为根 README/AGENTS 及 docs、apps、crates、ops、migrations、ci 下 Markdown，全部须从根 README 可达且 Cargo 包有 README。
支持代码围栏外 ATX 标题、行内相对链接及目录 README；引用式链接需扩展解析器，外链不联网检查。
它只证明结构/链接，不能核验源码语义、CLI 或历史证据。

## 发布边界

检查 job 不安装 dx、不构建页面；只有满足上述条件的 artifacts job 安装 dx 并构建完整包。
allow_missing 只保证无页面也可编译，运行缺资源返回明确 503。
web-bundle.sh 使用 wasm-release；dx 按需下载 esbuild 仅供构建，不增加 Node/npm 运行链。
release.sh 在构建前冻结源码身份，各阶段复查；核验实际二进制标记、对端资源缺失与 Worker 依赖，
再校验白名单、权限、统一时间、两次相同 archive、内外哈希和依赖摘要。正式 package 要求干净工作树。

分体包为 Public、Admin、Worker、auctionctl，仅两端内嵌各自页面。
--single-binary 先通过分体门禁，再核对 server 双 payload 和独立无 Web 的 auctionctl，归档只含这两个程序及合并 unit。
目录和配置模板路径/权限保持不变；包内 systemd/README.md 改为导航，安装说明统一在 docs/DEPLOYMENT.md。
输出目录、安装、配置、迁移与 --allow-dirty 限制见[部署手册](../ops/README.md#发布制品)。

默认仅静态检查与获准构建；all-targets 只编译已有测试，不新增或执行行为测试、服务、数据库或手工冒烟。
授权后才能开展 PostgreSQL 行锁、隔离、并发等运行验证，缺口见[状态页](../docs/status.md)。
