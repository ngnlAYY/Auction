# CI：检查与发布

[文档导航](docs/README.md) · [开发指南](docs/guide/development.md) · [部署手册](ops/README.md)

日常只需 `just verify`；构建发布用 `just release-check`。本目录保留五个实现文件，各有明确用途：

| 文件 | 用途 |
| --- | --- |
| [check.sh](ci/checks/check.sh) | 本地和 CI 共用的策略、Cargo 与依赖检查命令 |
| [web-bundle.sh](ci/release/web-bundle.sh) | 按 Public/Admin 构建 WASM，生成固定权限、mtime 和 payload 哈希；输入未变时复用既有 payload |
| [release.sh](ci/release/release.sh) | API-only / 内嵌档构建、资源隔离、身份校验和确定性归档 |
| [release-single.sh](ci/release/release-single.sh) | 由 release.sh 加载的合并服务变体，复用分体门禁；不是独立执行入口 |
| [build-identity.sh](ci/release/build-identity.sh) | 被 release.sh 与 web-bundle.sh 加载的源码身份指纹和 `BUILD-INFO` 字段读取；只有一份实现 |

## 本地与 CI 入口

| 范围 | 本地命令 | check.sh 子命令 |
| --- | --- | --- |
| 默认静态验证 | `just verify` | 与 `bash ci/checks/check.sh all rust` 等价 |
| 仓库 / 布局 / 配置 / 五语言 | 无单独 recipe，直接调用 check.sh | repository / layout / config / i18n |
| 格式 / 编译 / 独立 feature / Clippy / WASM | fmt / check / feature-check / lint / wasm-check | fmt / check / feature / lint / wasm；合称 rust |
| RustSec 公告 / 未用依赖 | 无 recipe，直接调用 check.sh | dependency-audit / dependency-unused；不属于默认本地 verify |
| 页面与发布 | web-build / build-api / build-release / build-single / release-check | 不由 check.sh 执行，见上方专用脚本 |

check.sh 无参数仍执行五项策略门禁（all），支持一次传入多项；Cargo 检查使用 `--locked`，默认图、独立
PostgreSQL/Redis 图、all-features Clippy 和两端 WASM 分别核验。`ci/release/release.sh source-fingerprint` 只输出
源码身份指纹，供 CI 缓存键与人工核对复用判定。MSRV job 单独用 1.94.1 编译全工作区/all-targets/all-features，
不代表长期支持或业务验收。依赖 job 固定 cargo-audit 0.22.2、cargo-machete 0.9.2，audit 联网读取 RustSec，
漏洞和全部警告均阻断且无忽略项；machete 只扫描直接依赖，不替代 feature、可达性和许可审查。

## 工作流调度

唯一工作流 [ci.yml](.github/workflows/ci.yml) 接收所有 push、PR 默认事件及 `workflow_dispatch` 手动触发，
默认分支从 GitHub 仓库元数据读取而不写死分支名。

| job | 前置条件 | 职责 |
| --- | --- | --- |
| docs | 无 | 去重判断；仓库/布局/配置/i18n、格式等廉价静态门禁 |
| verify | docs 通过且本轮需检查 | 工作区、独立 feature、Clippy 和双端 WASM 编译，不重复格式/策略检查 |
| security-audit | 同上 | 固定版本工具安装与完整依赖审计 |
| msrv | 同上 | 独立 MSRV 编译，与 verify/security-audit 并行 |
| artifacts | 上述四项通过；默认分支 push 或手动运行 | 完整通用分体包构建、归档校验及上传 |

非默认分支 push 仅在 GitHub API 确认**同仓库、同源提交**已有开放 PR 时跳过重复检查，改由 PR 事件检查合并结果；
API 查询失败会阻断，尚无 PR、PR 元数据未更新、默认分支、标签和手动运行不走该分支。PR 元数据只授予 docs job
`pull-requests: read`，其他 job 仅 `contents: read`，checkout 不持久化凭据。并发组按工作流、事件类型及完整
ref/PR 编号隔离，同组新运行取消旧运行，手动任务不被自动 push 取消，不同 fork 的同名分支互不影响；所有 job
均有超时，完整构建最多 90 分钟。保留 docs、verify、security-audit、msrv 检查名并新增 artifacts（原
Documentation 工作流已合并），分支保护若绑定旧工作流名须核对 required check。

## Runner 依赖与缓存

全部 job 固定 `ubuntu-24.04`，避免 `ubuntu-latest` 切换发行版造成工具或 ABI 漂移。docs 在运行门禁前显式安装
`ripgrep` 和 Perl，不能假设 runner 预装；缺少 `rg` 会在布局检查阶段退出 127，不得跳过门禁或忽略退出码。
Actions 固定完整 SHA，工具版本与取舍见[技术栈](docs/design/tech-stack.md#版本与升级)。编译缓存使用固定版本
[rust-cache](https://github.com/Swatinem/rust-cache)，发布档另用 sccache：

- verify、msrv、artifacts 采用 `v1-auction-build` 前缀，按 job、平台、Rust 工具链、Cargo 清单/锁图及构建配置
  隔离；缓存依赖和工作区编译产物，不跨工具链混用，也不缓存 Cargo 工具二进制。artifacts 使用独立 release 键，
  递归清理 `target` 下原生 API-only/embedded 与 WASM 缓存，不复用发布校验结论。
- 四个 Cargo job 的 `save-if` 都要求事件不是 `pull_request`：PR 缓存作用域是合并结果，写入后主分支无法复用且挤占
  额度；PR 仍完整恢复缓存并执行全部检查，`cache-on-failure` 保留。
- 只有 artifacts 启用 sccache 0.18.0（Fat LTO、单 CGU 与三种 feature 变体是最贵编译面）；官方 musl 静态归档与 dx
  一样固定 SHA-256 后解包，命中缓存仍复校。编译对象写入 Actions 缓存服务，键含 sccache 版本前缀，升级即整体失效；
  凭据只存在于 action 上下文，由 `actions/github-script` 输出到 step outputs 并仅赋给构建步骤的
  `ACTIONS_RESULTS_URL`、`ACTIONS_RUNTIME_TOKEN`；`RUSTC_WRAPPER` 会写入 `BUILD-INFO`，与未启用包装器的产物不会互用。
- artifacts 另按源码身份指纹缓存 `target/dist/artifacts` 与 `target/dx`（rust-cache 保存时会清掉非 Cargo 目录），
  键来自 `bash ci/release/release.sh source-fingerprint`，命中即 `BUILD-INFO` 一致；未命中不恢复旧目录。
- security-audit 沿用含工具版本的键，缓存 Cargo 工具二进制及安装元数据，缺失、执行失败或版本不符才
  `cargo install --locked --version ... --force` 重装。细粒度 `actions/cache` 只缓存官方 dx 0.7.10 与 sccache 0.18.0
  压缩包，下载与恢复后均核对固定 SHA-256，不缓存运行配置、凭据或下载制品。
- 缓存缺失不改变检查范围，命中后仍执行全部静态检查及审计，RustSec 仍联网更新，不缓存“通过”结论；release Cargo
  并发固定为 2，保留 Fat LTO、单 CGU 和通用 CPU 基线，不把原生 CPU 参数传给 WASM。

缓存命中率、耗时和 runner 内存边界须以实际 Actions 运行确认，不能由 YAML 静态检查推导。

## 下载 Actions 制品

默认分支 push 或手动运行通过检查后，artifacts job 调用既有 `bash ci/release/release.sh release-check`，重建双端
页面、API-only 对照及 embedded 四程序，执行资源隔离、源码身份和确定性归档门禁，再上传结果；源码身份、锁图、feature
与构建环境（`RUSTFLAGS`、`RUSTC_WRAPPER`、`CARGO_PROFILE_*`）同既有 `BUILD-INFO` 一致时复用制品并跳过页面重建。
不另建构建参数或归档白名单；feature 沿用 release.sh 的 PostgreSQL、Redis（`cache-redis`）和双端 web-embed，
API-only 对照制品仍只启用 PostgreSQL。本机可选 `RUSTC_WRAPPER=sccache SCCACHE_DIR=target/sccache` 或
`RUSTFLAGS="-C link-arg=-fuse-ld=lld"` 前缀复用同一脚本，两者都会写入 `BUILD-INFO`，条件不同不会混用；
CI 只对 artifacts 启用 sccache，不启用 lld。PR、非默认分支和标签的自动运行只检查、
不发布制品。在 GitHub 的 **Actions → Rust → 对应运行 → Artifacts** 下载
`auction-linux-x86_64-<commit>-<attempt>`（私有库需相应仓库权限），包内含 `.tar.gz`、外层 `SHA256SUMS` 和
`DEPENDENCIES.md`，先校验清单再解包；tar 内的 `auction-rust-<version>-<commit>/bin/` 包含且仅包含：

```text
auction-public-api
auction-admin-api
auction-worker
auctionctl
```

Public/Admin 各自内嵌页面，其余配置示例、部署模板和清单沿用标准分体包布局。先 tar 再上传保留可执行权限，
Actions 外层压缩级别为 0；找不到上传文件时报错。artifact 名包含提交和运行尝试编号，重跑不覆盖旧尝试，
保留期 14 天，不代替长期发布存档。这是 Ubuntu 24.04 构建的通用 Linux x86_64 GNU/glibc 包，不采用
`ops/build/ryzen-7950x.toml`，也不生成合并服务；部署机仍须满足实际 ELF/glibc 依赖，通用 CPU 不等于任意旧
Linux 均兼容。构建、上传不启动程序、不连接业务数据库、不自动部署，静态通过不代表生产验收。

## 文档边界

文档没有自动检查器：本地链接、标题锚点、从根 README 的可达入口与 Cargo 包 README 均靠评审核对；移动或重命名
页面时必须同步全部引用，静态结构正确也不证明源码语义、CLI 或历史证据准确。

## 发布边界

检查 job 不安装 dx、不构建页面，只有满足上述条件的 artifacts job 安装 dx 并构建完整包。`allow_missing` 只保证
无页面也可编译，运行缺资源返回明确 503；web-bundle.sh 使用 wasm-release，dx 按需下载 esbuild 仅供构建。
payload 复用只在本脚本内生效：源码身份、dx/rustc 版本与已存清单全部一致才跳过 dx，否则整目录重建。
release.sh 在构建前冻结源码身份并在各阶段复查，核验实际二进制标记、对端资源缺失与 Worker 依赖，再校验白名单、权限、
统一时间、两次相同 archive、内外哈希和依赖摘要；正式 package 要求干净工作树。分体包为 Public、Admin、Worker、
auctionctl，仅两端内嵌各自页面；`--single-binary` 先通过分体门禁，再核对 server 双 payload 和独立无 Web 的
auctionctl，归档只含这两个程序及合并 unit。目录和配置模板路径/权限保持不变，包内 `systemd/README.md` 改为导航，安装
说明统一在 `docs/DEPLOYMENT.md`；输出目录、安装、配置、迁移与 `--allow-dirty` 限制见
[部署手册](ops/README.md#发布制品)。默认仅静态检查与获准构建，all-targets 只编译已有测试，不新增或执行行为测试、服务、
数据库或手工冒烟；授权后才能开展 PostgreSQL 行锁、隔离、并发等运行验证，缺口见[状态页](docs/status.md)。
