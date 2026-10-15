# 实现与验证状态

[文档导航](./README.md) · [架构](architecture.md) · [默认验证范围](development.md#默认检查)

本页统一登记源码能力、既有验证结果与待补运行证据。当前有部分隔离验证声明、定向构建及限定部署证据，
尚不足以宣称完整业务或生产验收。文档修订不重跑历史流程，也不把当时失败改写为当时通过。

## 本页导航

- [能力范围](#能力范围)
- [证据口径](#证据口径)
- [本轮静态检查](#本轮静态检查)
- [源码核对发现的实现缺口](#源码核对发现的实现缺口)
- [仍需运行证据的事项](#仍需运行证据的事项)
- [未登记为现有能力](#未登记为现有能力)

## 能力范围

| 范围 | 源码中的实现边界 | 不能据此推导 |
| --- | --- | --- |
| HTTP 与页面 | Public 21、Admin 44 条 feature API；15/21 条浏览器入口 | 每条流程已经通过浏览器或部署验收 |
| PostgreSQL | 完整竞价、结算/恢复、论坛、邮件、凭据、运维 owner | 真实外部服务与生产数据正确性已获证明 |
| SQLite Lite | 共有竞价、会话、部分管理、持久调度和实时读取 | PostgreSQL 系统能力、行锁或并发语义对等 |
| 迁移 | PostgreSQL 39、SQLite 13 个追加迁移文件 | 任一目标数据库已经运行到该版本 |
| 发布 | API-only/embedded 隔离、payload 清单、确定性归档门禁 | 本次文档修改已经构建或部署了新制品 |
| 无配置首次启动 | 默认 loopback 设置页、五字段数据库输入；合并入口支持显式 HTTPS 代理及确认后空库初始化/启动 | 本轮已在真实主机完成设置、迁移、登录或浏览器验收 |
| 可选合并制品 | `auction-server` 自管三角色子进程，`auctionctl` 独立；默认分体包不变 | 已完成信号、父进程强杀、代理、迁移失败和升级运行验收 |
| 灾备 | 独立 backup/verify/rebuild/restore 与 canonical v28 接管流程 | 备份可用于未经验证的生产恢复或任意历史 schema |

注册数由[路由](contracts/routes.md)、[静态入口](../crates/auction-http/src/assets/routes.rs)及
[迁移目录](../migrations/README.md)核对。能力缺失应按契约返回明确错误，不能用空数据或示例数据替代。

## 证据口径

| 表述 | 必须有的依据 |
| --- | --- |
| 已实现 | 可定位的 owner、调用路径、契约与后端能力 |
| 静态检查通过 | 本次执行命令、退出结果、工具链及检查范围 |
| 隔离环境验证 | 明确环境、输入、目标版本、执行结果及保留的原始证据 |
| 生产验收完成 | 指定制品、真实配置/服务、生产数据边界、部署切换及恢复证据 |

既有文档使用过“局部 R1”描述 HTTP、竞价、结算、浏览器和 canonical PostgreSQL v28 接管/灾备的
部分隔离验证。本仓库正式文档没有提供足够完整的原始证据索引，本次也没有重跑这些流程。
因此这里保留为历史验证声明，不把它升级为本轮确认的运行结论。“R1”不能替代具体执行记录。

## 本轮静态检查

### 2026-10-14 Tokio LTS 与 OTel 配套升级

在 `3511a41` 的既有工作树改动上，Tokio 1.51.6 → 1.53.3，声明改为 `~1.53.3`，后续仅接收该 LTS
分支内补丁；上游公布支持期至 2027 年 9 月。OpenTelemetry API/SDK/HTTP/OTLP/proto 配套锁到 0.33.1，
tracing-opentelemetry 锁到 0.34.0。根依赖由装配/技术栈边界拥有，源码适配归 `auction-observability`。
定向锁图只升级七个包，未改变其余同版本包的依赖边；全工作区/all-features/all-target-platforms 活跃图
仍为 486 个节点，锁图仍为 535 个条目（外部包 515），不宣称体积或性能收益。

核对上游 changelog、exporter/processor trait、HTTP builder、retry policy 和现有装配后，唯一源码适配为
显式 `RetryPolicy::disabled()`：不接受 0.33 新增的默认三次重试，保持每批一次导出、失败计数和原配置预算。
自有有界队列、flush/shutdown、远端 context 边界、安全错误分类及 Prometheus 输出路径不变；未新增 TOML
字段、HTTP/事件字段、数据库迁移或运行时环境覆盖入口，未切换其他框架。上游新增的 64 MiB 请求体保护
保留，超限按导出失败处理；具体兼容边界见 [observability owner](../crates/auction-observability/README.md#实现布局与-tracing)。

Rust `1.98.0` 下 observability 独立 `--locked --all-targets` check 与 Clippy（`-D warnings`）通过；
完整 `just verify` 通过，覆盖 49 份文档、格式/策略/布局/配置/i18n、工作区全部目标、独立
PostgreSQL/SQLite/Redis、全部目标/特性 Clippy 与两端 WASM。MSRV `1.94.1` 的工作区
`--locked --all-targets --all-features` check 通过；修改路径主动 LSP 检查无诊断。
`cargo audit --deny yanked --deny unsound --deny unmaintained`（cargo-audit 0.22.2）扫描 535 个条目 / 1310 条公告，
退出 0、公告漏洞 0、警告 0；公告库提交 `6aed84668c23d7f535077a9fe918bf1fa0f6a5bc`，无新增忽略项。

本次同时记录 SDK 隐式 headers 环境覆盖的既有问题，见上述 owner 文档；此次升级没有修复该配置边界，
不能宣称 TOML-only 已通过运行验收。未检查进程实际环境或凭据，未新增或执行行为测试、真实 OTLP 导出、
scrape、故障/退出验证、数据库/外部业务操作或部署；未重建发布 payload。编译与公告扫描不证明生产验收。

### 2026-10-14 框架补丁与指标依赖精简

在 `3511a41` 的既有工作树改动上，定向升级 Tokio 1.51.4 → 1.51.6、直接 reqwest 0.13.4 → 0.13.5、
Clap 4.6.6 → 4.6.7；clap_builder/clap_derive 随之锁到 4.6.7。保留原 HTTP/TLS、运行时及 CLI feature，
未切换 Tokio 支持分支，也未升级 OTel 或引入新框架。Cargo 重选的无关 syn/Windows 依赖边恢复为既有锁定关系，
最终锁图通过 `cargo metadata --locked --all-features` 校验。此步骤仅改依赖清单、锁图和相关文档，保留此前源码改动。

observability 的 Prometheus 0.14.0 关闭默认 `protobuf`，保留 `process`，使用上游 plain_model。
源码核对仍使用原 TextEncoder、进程/池/tracing collector 与业务 recorder，保留 Prometheus 0.0.4 文本、
指标名、label、bucket 和媒体类型契约；不关闭独立的 OTLP/HTTP-proto 追踪编码。
`protobuf 3.7.2`、`protobuf-support 3.7.2` 移出活跃图及锁图；reqwest 新增间接 `base64 0.23.1`，
现有直接 `base64 0.22.1` 不变。全工作区/all-features/all-target-platforms 活跃图 487 → 486 个节点，
锁图 536 → 535 个条目（外部包 516 → 515），净减一个而非两个包；未测量制品体积或性能收益。

Rust `1.98.0` 下 observability 独立 `--locked --all-targets` check 与 Clippy（`-D warnings`）通过；
最终锁图下完整 `just verify` 通过，覆盖 49 份文档、格式/策略/布局/配置/i18n、工作区全部目标、
独立 PostgreSQL/SQLite/Redis、全部目标/特性 Clippy 和两端 WASM。
MSRV `1.94.1` 的工作区 `--locked --all-targets --all-features` check 通过。
`cargo audit --deny yanked --deny unsound --deny unmaintained`（cargo-audit 0.22.2）扫描 535 个条目 / 1310 条公告，
退出 0、漏洞 0、警告 0；公告库提交 `6aed84668c23d7f535077a9fe918bf1fa0f6a5bc`，未添加忽略项。

当前选择与后续评审边界见[技术栈](tech-stack.md#指标与追踪)。未新增或执行行为测试，未做实际 scrape/逐字节
输出比较，未访问业务数据库或真实外部服务、未重建发布 payload、未部署；静态结果不证明运行兼容或生产验收。

### 2026-10-14 低风险依赖精简

承接下节版本升级，在 `3511a41` 的既有工作树改动上实施五项低风险精简：scraper 仅留库解析及错误支持、
Axum 移除未用宏/Form/Query、observability 删除直接 serde、Admin 以标准库 poll_fn 取代直接 futures-util、
Gloo feature 下沉到 Public/Admin。各 owner 的当前边界见[技术栈](tech-stack.md#2026-10-14-精简审计候选)。
SQLite feature、HTTP/事件字段、配置、迁移、轮询唤醒与卸载逻辑均未改变。

受控刷新锁图未新增或升级包。全工作区/all-features/all-target-platforms 活跃图去重后 491 → 487 个节点，
退出 `getopts`、`unicode-width`、`axum-macros`、`serde_urlencoded`；前三者也移出锁图，
锁图总数 539 → 536（外部包 519 → 516）。`serde_urlencoded` 仍被未启用的 Dioxus fullstack 等路径锁定，
不能通过手删锁记录消除；serde 和 futures-util 也仍有其他实际调用者。不以节点数宣称制品体积或性能提升。

Rust `1.98.0` 下 observability/integrations/http 各自 `--locked --all-targets` check 通过；
Shared/Admin/Public 各自独立 `--locked --target wasm32-unknown-unknown` check 通过。
反向 feature 图确认 Shared 只有 Gloo HTTP、Admin 增 JSON、Public 增 JSON/WebSocket，没有依赖合并构建补项。
完整 `just verify` 通过（49 份文档、独立 PostgreSQL/SQLite/Redis、全特性 Clippy、两端 WASM）；
MSRV `1.94.1` 的工作区 `--locked --all-targets --all-features` check 通过。
`cargo audit --deny yanked --deny unsound --deny unmaintained`（cargo-audit 0.22.2）扫描 536 个锁图条目 / 1310 条公告，
退出 0、漏洞 0、警告 0；公告库提交仍为 `6aed84668c23d7f535077a9fe918bf1fa0f6a5bc`，未添加忽略项。

未新增或执行行为测试、未重建发布 payload、未访问业务数据库或真实外部服务、未部署。

### 2026-10-14 依赖升级与精简审计

基线 `3511a41`。scraper 0.24.0 → 0.27.0（selectors 0.38.0 改用 rustc-hash）、
gloo-net 0.6.0 → 0.7.0、gloo-timers 0.3.0 → 0.4.0。未改 HTTP/事件字段、配置、迁移或业务规则；
共享 GET 保留精确 URL、10 秒头/正文预算及取消守卫，只更新对应说明。
锁图从 543 个包降至 539 个（外部包 523 → 519），fxhash 已移除；非默认 Dioxus fullstack
路径仍锁定旧 Gloo 版本，但未进入当前两端 CSR 或全工作区活跃依赖树。

Rust `1.98.0` 下完整 `just verify` 通过（49 份文档、独立 PostgreSQL/SQLite/Redis、全特性 Clippy、
两端 WASM）；工作区全部目标/特性 check 与 MSRV `1.94.1` 同范围 check 通过，7 个改动路径 LSP 无诊断。
本轮额外执行 `cargo audit --deny yanked --deny unsound --deny unmaintained`（cargo-audit 0.22.2），
539 个锁图条目 / 1310 条公告，退出 0、漏洞 0、警告 0、无忽略列表；公告库提交
`6aed84668c23d7f535077a9fe918bf1fa0f6a5bc`。本地/CI 既有 dependency-audit 阻断策略未变。

依赖精简另完成两条并行只读审计及主代理后端补审，六项候选见[技术栈](tech-stack.md#2026-10-14-精简审计候选)，
未把审计建议直接当作删除授权，也未宣称已减少候选对应的包或体积。
未新增或执行行为测试、未调用真实业务服务/数据库、未重建发布 payload、未部署；
真实论坛/邮件/装备解析及浏览器兼容性仍无本轮运行证据，不能以静态通过替代验收。

### 2026-10-14 审计修复

基线为 `c9a2d1e` 工作树，按静态审计修复 A01–A18，并仅将 `quinn-proto` 从 0.11.17 更新至 0.11.18。
保留已有玩家指南与文档改动；未修改历史迁移、业务表结构、HTTP/事件字段或运行配置字段。

Rust `1.98.0` 下完整 `just verify` 通过：49 份文档、格式、仓库/布局/配置/i18n 门禁、工作区全部目标、
独立 PostgreSQL/SQLite/Redis、全特性 Clippy（`-D warnings`）和两端 WASM。另通过工作区全部目标/特性 check、
MSRV `1.94.1` 同范围 check 及 `git diff --check`。
`cargo audit --deny yanked --deny unsound`（cargo-audit 0.22.2）扫描 543 项依赖、1310 条公告，退出 0；
无漏洞/撤回/unsound 命中，保留 `fxhash` 停维警告，不隐藏维护债。

上述仅为源码与静态证据。未新增或运行行为测试，未访问业务数据库/真实外部服务，未运行迁移、恢复、启动或部署。
已有异常 Draft、旧配置、失败 recovery 和邮件事实不自动回填；上线前仍须经授权核对现场与回归边界。

### 文档整理记录（本轮修复前）

本次仅更新 48 份正式文档。使用 Rust `1.98.0` 执行 `just docs-check`（48 份文档）、
`just fmt repository-policy-check layout-check config-example-check i18n-check` 和 `git diff --check`，均通过。
另静态比对全部 27 组表格列宽与源码调用，比例一致且各组总和为 100。
未改实现、配置或迁移，未重编产品、运行行为测试或操作服务；没有重跑完整 `just verify`。
以下日期是仓库既有记录，保留原范围和结果，不表示本次重跑。

### 2026-10-13

| 既有工作 | 当时结果与限制 |
| --- | --- |
| 文档维护机制调整 | 移除独立笔记目录和模板，保留通用文档门禁。`just docs-check` 覆盖 48 份文档；格式、策略、布局、配置、i18n、差异检查通过；文档工具以 `rustc --edition=2024 -D warnings` 编译，30 个路径 LSP 无诊断。提交前完整 `just verify` 通过，仅编译既有测试目标 |
| 邮件装备取时修复 | application 全部目标 check/Clippy、Worker `postgres,cache-redis` check、Worker 全特性 Clippy、格式/差异/策略/布局/i18n 通过；改动源码和六份文档 LSP 无诊断。Worker 单 PostgreSQL Clippy 被未改动 `admin_mail/backfill.rs:37` 的 `infallible_destructuring_match` 阻塞；当时 docs-check 因历史链接失效失败 |
| 线上规则只读排查 | Public 规则 GET 的 `zh-CN/en` 均为 `503 / rules_unavailable`，所选及默认英文正文尚未发布；只读日志和 GET，未登录、发布、改配置或操作数据库 |

取时修复用每件装备读取后的注入时钟校验 `collected_at`，不放宽未来时间门禁，不改命令时间/摘要或旧邮件事实。
当时未重建部署、重启服务或重放邮件；规则正文仍需操作者确认后发布。该日期的文档/源码修改均未执行行为测试。

### 2026-10-12

以下源码静态记录使用 Rust/Cargo `1.98.0`。三轮的 docs-check 当时均因历史链接失效失败，未恢复用户删除。

| 既有工作 | 当时静态结果与限制 |
| --- | --- |
| Admin 拒收说明 | 五语言显示持久诊断与原始码；Admin 原生/WASM check 与 Clippy、格式/差异/i18n/布局/策略通过，4 个 Rust 文件 LSP 干净；CSS 保留 9 项既有 `!important` 警告。仅改善说明，取时缺陷到 10-13 才修复 |
| A44 Cookie 只读校验 | 工作区全部目标、Admin PostgreSQL + Web、独立两后端 storage、双 WASM、全特性 Clippy 和格式/差异/策略/布局/配置/i18n 通过。29 个 Rust 文件 LSP 未报诊断，其中 18 个确认干净、11 个未确认；未用真实 Cookie 调用接口 |
| 两份 AMD Cargo 候选 | CPU 白名单经 `rustc --print cfg` 核对，EPYC 不含 `rdseed`；两配置分别通过四程序的 `--locked --release` check，显式 PostgreSQL/Redis/Web。TOML LSP、格式/差异/策略/布局/配置/i18n 通过；这一阶段尚未链接服务程序 |

独立授权的后续构建、测量和部署证据：

| 范围 | 限定结论 | 证据入口 |
| --- | --- | --- |
| Intel KVM 领域微基准 | 6 组参数 × 10 类操作 × 9 轮；v3/Fat LTO/1 CGU 等权几何平均吞吐为基线 1.158 倍。AMD 微基准仅编译，不能外推目标收益 | [测量记录](../ops/build/README.md#已有测量和待补证据) |
| Ryzen 7950X 四程序与双 Web | Release 链接、CPU/LTO/CGU、内嵌隔离、ELF、源码/页面身份及压缩包 SHA-256 静态核对通过；是 dirty 工作树候选，非正式归档。该构建阶段未运行服务或部署，未生成 EPYC 服务制品 | [重编记录](../ops/build/README.md#2026-10-12-ryzen-7950x-重编记录) |
| Ryzen 目标空库初始化与启动 | 路径/权限修正，严格停机、快照校验与预检后在 PostgreSQL 18.6 应用 39 条迁移，最大版本 39、失败 0、64 张 public 表。三角色 ready、重启 0，最近 160 条本次日志无 WARN/ERROR，receipt 和程序哈希匹配 | [目标初始化](../ops/build/README.md#2026-10-12-ryzen-目标初始化与启动) |
| 后续 Cookie 候选替换 | 保存旧程序与已校验 custom-format 备份后替换；schema 保持 39，无迁移/恢复/配置修改。三角色就绪、哈希/回执和两端各 6 个静态资源匹配，重启 0，外部 readiness 200 | [候选更新](../ops/build/README.md#后续-cookie-校验候选更新) |

这些部署记录不证明网页设置、竞价/并发、外部论坛/邮件、完整浏览器或恢复演练已通过；
Cookie 校验接口仍未用真实凭据验证。PostgreSQL p99/吞吐、AMD 现场性能和代表性 PGO 尚无证据。

### 2026-10-11

两轮使用 Rust/Cargo `1.98.0`。格式/差异、策略/布局/配置/i18n、工作区全部目标、独立 PG/SQLite/Redis、
全特性 Clippy（`-D warnings`）及双 WASM 均通过；docs-check 当时失败，未恢复被删除文档或改门禁。

#### 复审修复

另通过工作区 `--all-targets --all-features` check，10 个 Rust 文件 LSP 无诊断；
三个 SQL 文件的 8 条查询/绑定链数量静态匹配，未执行 SQL。docs-check 首个错误为 `ci/README.md` 历史链接失效。
修改领域、应用和两后端存储，不新增依赖、锁图或迁移；旧错误时间和损坏账本不自动修复。

#### 首轮领域优化

六个 Rust 文件 LSP 无诊断；docs-check 因历史目录缺失及残留链接失败，完整 `just verify` 未通过。
仅修改领域及文档，不新增依赖、锁图或迁移。两轮均未运行行为测试、服务、数据库、部署或发布重编；
首轮亦未做性能测量，静态结果不证明历史兼容性。

### 2026-10-09

在 `4a04214` 基线修复 5 项问题。Rust/Cargo `1.98.0` 的格式/差异、策略/布局/配置/i18n、
工作区和独立 feature、Clippy、双 WASM、`--all-targets --all-features` check，以及 MSRV `1.94.1`
同范围 check 均通过；18 个 Rust 文件 LSP 无诊断。

完整 `just verify` 退出 1，停在 `ci/README.md` 历史链接失效。用户确认保留 7 份历史说明的删除，
其他门禁独立通过不能改写为完整 verify 通过。未执行依赖公告审计；锁图仅增加 HTTP 到既有
thiserror 2.0.20 的依赖边，未升级版本或改历史 SQL。未运行行为测试、产品、数据库、迁移或浏览器流程。

### 2026-10-03 历史记录

Rust/Cargo `1.98.0` 下完整 `just verify`（当时 52 份文档）、工作区全部目标/特性 check、MSRV
`1.94.1` 同范围 check 及差异检查通过。依赖审计 cargo-audit `0.22.2` 使用 `--deny yanked --deny unsound`，
检查 543 个条目，无漏洞/撤回/unsound 命中，保留 `fxhash` 停维提示；公告库提交
`f8dee89e1b2f2f1eaf548312df7655fe5202a302`。另发现的 chacha20 0.10.1 撤回版本升至 0.10.2。

SQL 静态扫描覆盖 39 份迁移、737 个 Rust SQL 候选；两份受限执行视图解析通过，686 条可识别直接绑定链
未见数量不匹配。历史 SQL 未改，保留字问题由兼容视图处理；8 份 PL/pgSQL 的解析器输出存在限制，
不能视为全部函数体/schema/事务验证。LSP 因工具链缺少 `rust-analyzer` 未完成，不以 Cargo 结果替代。
未执行行为测试、发布构建、浏览器、服务、数据库或迁移。

### 2026-09-22 历史记录

Rust/Cargo `1.98.0`、just `1.58.0` 下完整 `just verify` 与差异检查退出 0：格式、当时 51 份文档、
策略/布局/配置/i18n、工作区全部目标、独立 PG/SQLite/Redis、全特性 Clippy 及双 WASM 通过。
修复门禁发现的借用、错误映射和 feature 编译问题后重跑，未执行行为测试、发布、浏览器、服务、数据库或迁移。
29 项报告问题、论坛 DTO 附项及三份追加迁移属于该轮；历史流拍 Return、资产冲突、错误归档 Credits 和
旧排期来源仍须独立核对，见[邮件资产核对](operations/mail-diagnostics.md#历史资产处置核对)。

## 源码核对发现的实现缺口

2026-10-14 全仓审计的 18 项（11 项 P1、7 项 P2）已在源码范围修复；不将静态检查等同于行为验收：

| 审计项 | 修复 owner 与范围 |
| --- | --- |
| A01/A02 · 自动 Draft 与后继经济模板 | [生命周期与存储](../crates/auction-storage/README.md#持久调度与慢速代理)：两后端不提前写运行期结束时间，新后继冻结有效模板 |
| A03/A04 · 退款确认与 Recovery 人工重试 | [Recovery 契约](contracts/routes.md#recovery-与实时运维)：对齐 ready 义务，匹配的父子投影与 delivery 重试原子恢复 |
| A05/A09/A10 · runtime 与首次凭据 | [配置](../ops/config/README.md#runtime-共享与重启)：恢复错误阻断启动，复用会话校验，Unix runtime 校验有效 UID |
| A06/A07 · 论坛越序与邮件成功前缀 | [Worker 编排](../crates/auction-jobs/README.md#生命周期慢速代理与结算)：持久 pending 先挡后续楼层，失败后缀不丢完成前缀 |
| A08 · Lite 时间文本比较 | [存储](../crates/auction-storage/README.md)：领取/开拍使用时间值，兼容存量 RFC3339 与空格格式 |
| A11/A14 · 未知结果重试与时间精度 | [Admin 交互](../crates/auction-web-admin/DESIGN.md#拍卖录入与结算)：冻结完整请求，先恢复精度再预检 |
| A12/A13 · 录入世界与隐藏来源字段 | [Admin 命令](../crates/auction-web-admin/README.md#管理数据与命令)：world 隔离表单，JSON 来源不携带旧 URL |
| A15 · 灾备文件替换 | [灾备安全边界](../ops/deploy/RECOVERY.md#安全边界)：校验属主与完整受信目录链 |
| A16/A17/A18 · 空库与恢复后验 | [存储迁移](../crates/auction-storage/README.md#迁移)：共用完整对象盘点，subscription 限当前库，旧迁移也核对完整前缀与 checksum |

旧版本提前物化的 Draft、缺失的邮件完成证明、未闭合 recovery 和不一致 runtime 仍须独立确认；
本轮未清理、重放或改写这些历史现场。此前玩家提示与 systemd 停止预算缺口继续保留如下。

[中文玩家指南](player-guide.md)的源码核对还发现两项玩家提示偏差：
[Public 中文登录占位文案](../crates/auction-web-public/i18n/zh-CN.ftl)仍称“32 位 Key”，而
[邮件 Key 协议](../crates/auction-application/src/ports/mail/user/security.rs)实际签发和换绑使用 43 字符的 canonical Base64URL；
登录输入没有按 32 字符截断，指南要求完整复制回信 Key。
[论坛主题正文](../crates/auction-integrations/src/forum/render.rs)把 notify 描述为反超提醒，但
[当前通知 owner](../crates/auction-jobs/src/worker/forum/reply/notification.rs)仅入队单次普通 MoogleMail 通知，
[默认正文](../crates/auction-integrations/src/clients/mooglemail/transport/outcomes.rs)也不是出价结果或持续订阅；论坛 PM 仍未接入。
本次只在玩家指南说明并登记这些偏差，未修改译文、渲染或通知逻辑，未取得真实邮件或浏览器行为证据。

本次文档核对发现，[分体总控 unit](../ops/deploy/systemd/auction-stack.service.example)的
`TimeoutStopSec=120` 小于三角色依次各用满 45 秒的合计 135 秒。若通过该 unit 执行完整顺序停止，
外层可能先超时；部署前应按内层预算及控制命令开销重新核算，见[systemd 手册](../ops/deploy/systemd/README.md)。
本次只登记这一配置预算缺口，未修改模板或取得停止流程的运行证据。

2026-10-12 的装备拒收排查发现，邮件开始时固定的 `now` 早于读取后生成的 `collected_at`，
使正常快照被记为 `snapshot_invalid / collected_at_future`。2026-10-13 的
[拍卖入站修复](../crates/auction-application/src/services/mail/processing/processor/auction.rs)改为每件装备读取后
重新取应用时钟执行 `mail_validation_issue`，未来时间规则、命令原有业务时间与幂等字段不变。
当前仅完成源码与静态验证，尚未部署或取得实际接收证据；既有拒收关联的退回/通知任务须独立核对，
不得盲目重放，也不改写历史诊断。

2026-10-11 在领域源码收口代理变更报错后的半修改状态，以及竞价事实、代理转换链与最终
快照的内部不一致校验缺口。三条自动反超路径合并为同一计算，没有变更拍卖机制或持久化布局；
版本溢出、非法终止原因、历史局部重放和同价边界仍缺运行证据。同日复审的 4 项 P2 与 1 项 P3
已在源码范围修复：

| 问题 | 源码修复 | 待闭合边界 |
| --- | --- | --- |
| P2 · 合法慢速代理被账户投影判为损坏 | [账户能力](../crates/auction-storage/src/database/account/mapping/proxy.rs) 区分授权合法性与可降额下限，不足完整下一口返回 `can_lower=false` | 账户列表/详情与代理反超仍缺行为验证 |
| P2 · 代理历史与命令结果使用两套时间 | 两后端 `bidding/proxy.rs` 显式绑定同一锁后 `occurred_at`，覆盖 Public、slow due 与 PostgreSQL Final 计划 | 并发/精度需数据库验证；旧记录不回填、不重写 |
| P2 · 已截止候选占满慢速扫描批次 | [到期扫描](../crates/auction-storage/src/database/slow_proxy_due.rs) 在 LIMIT 前排除已截止 lot，执行端仍锁后复核 | 未验证 Final 失败及跨截止竞态；不保证任意坏数据下的公平性 |
| P2 · `accepted` 重放缺少请求绑定 | [Public 防腐层](../crates/auction-application/src/services/auction/competition/legacy_ledger.rs) 核对请求/首条事实；[慢速重放](../crates/auction-application/src/services/auction/competition/slow_replay.rs) 核对命令/lot/授权/generation 与结果形状 | 合法旧载荷及损坏载荷仍需运行证据，不重算历史金额 |
| P3 · 拍卖聚合溢出后部分修改 | [拍卖聚合](../crates/auction-domain/src/auction/mod.rs) 校验版本后才发布状态、时间和元信息 | 版本极值仍缺行为验证 |

2026-10-09 审计的 1 项 P1 与 4 项 P2 已在源码范围修复；不宣称全项目不存在其他缺陷：

| 问题 | 源码修复 | 待闭合边界 |
| --- | --- | --- |
| P1 · 普通实时连接无活跃上限 | [Hub 配额](../crates/auction-http/src/http/shared/realtime_hub/connections.rs) 每进程 1024、每用户 8，RAII 释放；发现流独立 128 槽保留 | 多实例不共享配额，容量和释放路径仍缺运行证据 |
| P2 · 既有 WebSocket 不消费会话撤销 | [只读检查](../crates/auction-http/src/http/public/realtime/session.rs) 首次推送前与周期内查两后端权威会话，不续期，失效/读取失败关闭 | 非同步踢线；注销、封禁、换钥、到期和依赖失败仍须运行验证 |
| P2 · 健康空闲连接误断线 | [浏览器订阅](../crates/auction-web-public/src/api/auction/realtime.rs) 只限握手 10 秒，移除业务帧空闲看门狗 | 仍需静默连接、网络黑洞和重连恢复证据 |
| P2 · 公开拍卖空页总数为 0 | [拍卖读取](../crates/auction-storage/src/database/auctions/) 同一读取快照独立统计总数，cursor 不统计 | 并发翻页和越界页未作行为验证 |
| P2 · Lite 开拍展示按时间文本比较 | 拍卖当前/实时预检和 lot 列表/详情四处 SQL 改用 `julianday` | 历史两种格式、偏移与边界时刻未作数据库验证 |

此前 2026-10-03 的 7 项问题保留以下历史源码修复记录；其中两项初始化缺口沿用更早登记，
不重算为首次发现：

| 问题 | 源码修复 | 待闭合边界 |
| --- | --- | --- |
| F1 · Admin 凭据轮换后旧会话仍有效 | [session 服务](../crates/auction-application/src/services/sessions/mod.rs) 异步启动先永久撤销旧会话，HMAC 绑定 actor/secret，读取复核 actor；两后端共用既有表 | 重启需重新登录；凭据切换、恢复旧配置、多实例维护仍缺运行证据 |
| F2 · 首次初始化绕过迁移兼容层 | [initialization](../crates/auction-storage/src/database/initialization/mod.rs) 在既有迁移锁内复用受限执行视图，原 SQL/checksum 不变 | 仍须独立验证空库迁移、失败现场与三角色启动 |
| F3/F6 · rustls/h2 漏洞公告 | [锁图](../Cargo.lock) 为 rustls 0.23.45、h2 0.4.19；rustls 下限同步，CI 新增依赖审计 | 版本范围修复不等于生产可达性或未知漏洞证明 |
| F4 · 设置慢请求与无界退出 | [access](../crates/auction-setup/src/server/access.rs) 限正文/异步处理 10 秒，令牌先于正文；[lifecycle](../crates/auction-setup/src/server/lifecycle.rs) 复用 5 秒请求头与 10 秒排空；终止不继续初始化 | 未执行慢请求、断连、信号或阻塞文件系统运行验证 |
| F5/F7 · 设置防嵌入及拒绝响应安全头 | 正式表面与设置复用 [headers](../crates/auction-http/src/http/security/headers.rs)，包括拒绝、框架错误及超时应用响应 | 未执行浏览器/代理验证 |

`fxhash 0.2.1` 的 RUSTSEC-2025-0057 停维维护债已通过本轮 scraper 升级移除；
历史命中不是已确认生产漏洞，CI 不隐藏公告，维护边界见[技术栈](tech-stack.md#版本与升级)。

初始化失败应保留配置和数据库现场，停止进程后按[合并部署维护](../apps/server/README.md#生命周期与维护)
显式处理，不通过删除配置、修改历史迁移或自动重试来绕过问题。

## 仍需运行证据的事项

- PostgreSQL 并发竞价、资格变化、序列化冲突、故障重启、损坏事实与恢复边界。
- SQLite 单主机串行写入、文件锁、维护窗口和备份恢复；不能替代 PostgreSQL 验证。
- 真实 MoogleMail/E-Hentai 的认证、未知写入结果、Sent/Balance 证明与人工恢复。
- Public/Admin 完整页面、五语言、认证组合、Admin 重启/凭据轮换及旧配置恢复、严格 decoder、实时帧和断线恢复。
- Public 连接总量/用户配额、槽位释放、会话只读复核与撤销延迟、静默连接；分页快照及 Lite 兼容时间边界。
- 非空生产数据、目标主机权限与服务编排、制品切换、备份恢复和回执保管。
- 首次设置的本机/HTTPS 代理门禁、未知保存结果与空库初始化；合并部署的 readiness、信号、父进程强杀、迁移失败和升级。

后续获得授权开展行为验证时，应登记制品/源码身份、后端与版本、脱敏环境、时间、输入、预期与实际结果、
证据保存位置和限制。敏感证据保存在受控目录，项目文档只记录安全引用和结论。
静态验证、页面样式完成、空表或路由存在均不足以关闭上述事项。

## 未登记为现有能力

Public 当前实时通道是 WebSocket 加 HTTP Catch-up；未注册 SSE endpoint。
外部 realtime 广播 sink、retention 数据删除/归档 mutation、自动 OpenAPI 生成也不属于当前交付能力。
要新增这些能力，需要独立契约、owner、授权边界及明确的设计与运维说明。
