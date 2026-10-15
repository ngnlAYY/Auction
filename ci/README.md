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
唯一工作流 [ci.yml](../.github/workflows/ci.yml) 保留 docs、verify、security-audit、msrv 四个 job，
docs 独立报告，verify 不重复检查文档。原 Documentation 工作流已合并；若分支保护按工作流名绑定，须核对对应 required check。

Cargo 检查使用 --locked；默认图、独立 PostgreSQL/Redis 图、all-features Clippy 和两端 WASM 分别核验。
MSRV job 单独用 1.94.1 编译全工作区/all-targets/all-features，不代表长期支持或业务验收。
依赖 job 固定 cargo-audit 0.22.2、cargo-machete 0.9.2；audit 联网读取 RustSec，漏洞和全部警告均阻断，无忽略项。
machete 只扫描直接依赖，不替代 feature、可达性和许可审查；审计不改锁图、不运行产品。
Actions 固定完整 SHA，只读权限，checkout 不持久化凭据；完整版本与历史取舍见[技术栈](../docs/tech-stack.md#2026-10-15-全依赖审计)。

## 文档检查器

check.sh docs 先用 rustfmt 检查，再以 rustc edition 2024、-D warnings 编译 docs.rs 至忽略的 target。
正式范围为根 README/AGENTS 及 docs、apps、crates、ops、migrations、ci 下 Markdown，全部须从根 README 可达且 Cargo 包有 README。
支持代码围栏外 ATX 标题、行内相对链接及目录 README；引用式链接需扩展解析器，外链不联网检查。
它只证明结构/链接，不能核验源码语义、CLI 或历史证据。

## 发布边界

普通 CI 不安装 dx、不构建页面；allow_missing 只保证无页面也可编译，运行缺资源返回明确 503。
web-bundle.sh 使用 wasm-release；dx 按需下载 esbuild 仅供构建，不增加 Node/npm 运行链。
release.sh 在构建前冻结源码身份，各阶段复查；核验实际二进制标记、对端资源缺失与 Worker 依赖，
再校验白名单、权限、统一时间、两次相同 archive、内外哈希和依赖摘要。正式 package 要求干净工作树。

分体包为 Public、Admin、Worker、auctionctl，仅两端内嵌各自页面。
--single-binary 先通过分体门禁，再核对 server 双 payload 和独立无 Web 的 auctionctl，归档只含这两个程序及合并 unit。
目录和配置模板路径/权限保持不变；包内 systemd/README.md 改为导航，安装说明统一在 docs/DEPLOYMENT.md。
输出目录、安装、配置、迁移与 --allow-dirty 限制见[部署手册](../ops/README.md#发布制品)。

默认仅静态检查；all-targets 只编译已有测试，不新增或执行行为测试、服务、数据库或手工冒烟。
授权后才能开展 PostgreSQL 行锁、隔离、并发等运行验证，缺口见[状态页](../docs/status.md)。
