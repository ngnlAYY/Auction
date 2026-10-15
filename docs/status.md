# 实现与验证状态

[文档导航](./README.md) · [架构](architecture.md) · [默认验证范围](development.md#默认检查)

本页登记源码能力、历史验证与待补证据。现有隔离验证声明、定向构建及限定部署不足以证明完整业务或生产验收；
文档修订不改变历史结果。

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
| HTTP 与页面 | Public 21、Admin 46 条 feature API；15/21 条浏览器入口 | 每条流程已经通过浏览器或部署验收 |
| PostgreSQL | 完整竞价、结算/恢复、论坛、邮件、凭据、运维 owner | 真实外部服务与生产数据正确性已获证明 |
| SQLite 历史资产 | 运行支持已移除，保留已发布迁移与已有测试 | 保留文件或锁图可选依赖意味着当前支持 SQLite |
| 迁移 | PostgreSQL 39 个追加迁移文件；SQLite 13 个历史文件保留 | 任一目标数据库已经运行到该版本 |
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

HTTP、竞价、结算、浏览器及 canonical PostgreSQL v28 接管/灾备曾登记“局部 R1”。
原始证据索引不足，保留为历史隔离验证声明，不能替代具体执行记录或视为当前运行确认。

## 本轮静态检查

各记录仅覆盖注明的工作树与验证范围，不代表运行验收。
SQLite 退役前的完整 `just verify` 包括格式、文档（份数逐节注明）、策略/布局/配置/i18n、工作区全部目标、
独立 PostgreSQL/SQLite/Redis、全部目标/特性 Clippy（`-D warnings`）和两端 WASM；SQLite 仅属当时范围。
当前检查入口见[开发指南](development.md#默认检查)。

2026-10-14 四轮依赖调整的公告审计共用 cargo-audit 0.22.2、
`cargo audit --deny yanked --deny unsound --deny unmaintained`、1310 条公告及公告库提交
`6aed84668c23d7f535077a9fe918bf1fa0f6a5bc`；各节保留锁图条目数与结果。“审计修复”的不同策略单列。

### 2026-10-15 论坛拒绝原因安全展示

新增 jobs `worker/forum/reply/rejection.rs`，将不可重试应用错误分类为具体拒绝码，并把固定 #2 的拒绝提示映射为安全英文。
覆盖已领先、最低出价、金额、截止、资格、状态及既有代理/通知结果；未知原因保留通用提示，不原样公开本地诊断。
application 的 `ForumCommandOutcome` 增加既有错误码投影，storage 只读取已有列，无迁移、HTTP/配置或竞价规则变化。
旧 `command_rejected`/空码仅通过完整白名单诊断格式映射，包含 Hea04 已领先拒绝；不改写历史账本、不重新执行出价，
pending 顺序、水位和未知写入门禁保持不变。详情更新仍依赖新版 Worker 正常领取活动主题并成功刷新固定帖。

已通过完整 `just verify`、6 个 Rust 文件主动 LSP 与差异检查；模块 README、架构、schema、不变量及玩家说明同步。
未新增/运行行为测试，未重编 Release 包、部署或重启云端；此前论坛界面候选不含本次拒绝说明改动，尚无真实论坛更新验证。

### 2026-10-15 论坛摘要与正文展示分离

Public `/forum` 保留同步摘要、真实处理楼层、更新时间和简短延迟提示，主题链接位于已同步提示行右侧。
正文、源/内容版本、同步/扫描及错误详情移到 Admin `/auctions`；正文默认折叠，按转义 BBCode 文本显示本地目标。
Owner 仅为 Public/Admin Web 展示、样式及五语言；沿用 P15/A46 DTO，不变更 HTTP 字段、权限、数据库或 Worker 同步逻辑。

已通过 `just verify`、双端 WASM 编译与完整 Ryzen 候选构建；两处 Rust 组件主动 LSP 无诊断，CSS 的既有提示未作无关修改。
CPU/feature、资源身份和隔离、ELF/glibc、Worker 依赖、页面样式归属、34 项文件哈希及归档一致性静态核验通过。
包及校验和见[构建记录](../ops/build/README.md#2026-10-15-论坛摘要与正文展示分离候选)。
未运行行为测试或浏览器，未部署/重启云端；前一项真实论坛恢复不构成本轮界面验收。完成记录在封存后追加。

### 2026-10-15 用户授权后的云端论坛恢复

用户手动更新后，运行中的 Worker SHA-256 为
`98135fdcc182a67b4853c4b8bf40c20f06a0f322b88157cdf77efaf93a63f30c`，与最新 Ryzen 候选一致；
三角色在 11:06 UTC 启动。Worker 配置省略 `verify_post_body`，按新版默认关闭，无需补字段。
原失败记录仍是 09:35 UTC 的 `write_authorization_required`、attempt_count=5，更新不会自动解除该门禁。

经用户再次明确授权，先只读核对板块精确标题唯一命中主题 `302009`、两种拍卖标记及固定帖身份，
再于 11:12:54 UTC 对拍卖 `6673453c-99c2-4b2f-834b-65d1c0daea1e` 提交一次官方恢复：
command_id=`81db12a1-70a8-45f3-ae71-de6dcdd5bd95`，actor=`operator-assisted-recovery`。
命令账本及 `forum.authorize_write_retry` 成功审计各一条；未重复提交授权。

11:16:44 UTC 发布状态转为 `published`、attempt_count=6，持久化 thread=`302009`、#1=`6984876`、#2=`6984877`。
11:16:47 UTC 内容版本为 2、真实处理楼层为 8，主题及内容错误码均空；第 3–8 楼共 11 条竞价账本均为 `applied`，
包括第 3 楼 `Hea03 100k`。远端只读 GET 确认原 #2 显示同步到 #8；Public 论坛投影 GET 返回 200、`published`、楼层 8。

本次仅覆盖用户授权的单场真实论坛恢复与只读结果核对，不是完整生产或浏览器验收；未新增/运行自动化行为测试。
未修改配置、部署、重启或直接修改数据库状态。Owner 为 Worker 论坛编排、存储授权/账本和集成回执；无新增外部契约或迁移。

### 2026-10-15 论坛校验开关与管理端状态刷新

`worker.forum.verify_post_body` 默认关闭，主题同步和回帖刷新统一配置；正常 POST 仍检查精确回执与固定帖身份。
创建端口明确区分新建和查到旧帖；关闭校验时，未知主题发布等待人工授权，回帖内容未知保留门禁，不把新快照误记为已发送。
现有 CLI 授权仅适用于主题发布失败。固定 #2 的待处理/拒绝摘要改为英文，本地账本详情保留。
公开页移除四处状态重试按钮；Admin `/auctions` 承接只读刷新，增加 A45/A46 认证 GET，复用安全 DTO，不新增恢复写入口或数据库迁移。
新增源码由 HTTP 状态适配、Admin API 消费和拍卖状态组件各自拥有，Public/Admin 五语言及契约同步。

已通过完整 `just verify`（格式、文档/策略/布局/配置/i18n、工作区全部目标、feature、Clippy `-D warnings`、双 WASM）及差异检查。
关键 Worker/HTTP/Admin 新增源码的主动 LSP 复核无诊断；批量检查仍保留 Admin CSS 九项既有 `!important` 提示，部分推送式检查无法确认干净。
已完成独立双端 wasm-release 与 Ryzen 四程序 Release 构建；CPU/feature、资源隔离、ELF/glibc、Worker 依赖、33 项文件哈希及归档一致性核验通过。
完整候选及 SHA-256 见[构建记录](../ops/build/README.md#2026-10-15-论坛开关与管理端刷新候选)；文档完成记录在封存后追加，不属于构建前源码指纹。
未新增/执行行为测试，未部署、重启云端或再次提交论坛恢复授权；线上恢复尚无本轮证据。

### 2026-10-15 论坛正文回读与限定恢复

应用端口统一只忽略末尾 CR/LF；集成层按真实编辑表单的列表边界换行生成 BBCode，保留其他正文逐字校验。
固定 #1 增加配置 origin 下的当期 `Auction address` 和论坛 `About system` 链接，领先来源使用英文 `(proxy)`/`(#N)`，
解析器兼容旧中文/全角标记。Worker 对带 `reconcile_` 的未知写入使用只读短退避，存储首轮水位可重新领取历史未知结果；
人工授权门禁不自动解除，未知结果路径不得补楼或升级旧布局。无新增迁移、HTTP/JSON/事件或运行配置字段。

Rust 1.99.0 下 `just verify` 通过（45 份文档），六个本轮改动源码路径主动 LSP 无诊断；
`auction-worker` 的 `--locked --release --no-default-features --features postgres,cache-redis` 编译通过。
未新增或执行自动化行为测试。后续 AMD 候选构建不由这项通用 Worker 编译结果代替。

经用户单独授权，对主题 `302009` 执行一次官方恢复授权，命令 ID 为 `212c2d0b-f614-4a97-82a6-ec2e6682233c`。
数据库由旧 `write_authorization_required` 推进后，在 2026-10-15T08:31:21Z 再次记录 `reconcile_write_unknown`、attempt_count=2；
只读 GET/浏览器确认原 #1 已出现可点击的当期拍卖链接，#2 仍停在楼层 2。编辑表单明确显示论坛删除了列表边界及末尾换行，
旧严格比较会失败；POST 回执未留原始证据，不将页面更新当作本地发布成功。未直接改表、重发未知 POST、创建新主题或运行新候选。
用户最终选择手动更新；本轮未替换云端程序、未重启三角色，发布确认、回帖导入及新模板的真实回读仍待更新后验证。

#### 后续长网址回读差异

用户手动更新后，只读哈希确认三角色为上述 Ryzen 候选；2026-10-15T09:20:24Z 首轮只读恢复转为
`write_authorization_required`、attempt_count=3，原帖仍是旧标签和中文代理标记，不能把新增模板直接当成已发布。
随后用户再次单独授权，执行命令 `b7f1cf64-0a49-428b-a125-48912f31f796`；2026-10-15T09:30:35Z
记录 `reconcile_write_unknown`、attempt_count=4。只读编辑表单确认新标签、说明链接及英文代理已写入既有 #1，
但论坛把纯网址链接的显示文字截成带 `...` 的字符串，完整目标 URL 未变。此差异足以阻止逐字正文证明；
不能据此断言该次 POST 回执解析已经通过。#2 仍停在楼层 2，不宣称发布或回帖导入成功。

集成层恢复链接文字内的 `[b]` 包装：同一原帖此前的带格式完整网址已通过只读回读确认不会被截短。
不放宽正文比较、未知写入或人工授权门禁，不新增迁移/HTTP/事件/配置字段。没有再次提交授权、直接改表或重启服务；
本次新模板的部署后发布确认仍待补证据。

### 2026-10-15 PostgreSQL 收口与项目精简

storage 删除 SQLite 驱动、事务/查询适配、文件锁及不可达分支；config 仅接受 PostgreSQL 连接，
apps/CLI/Worker 按单后端装配。既有 PostgreSQL 事务、锁、幂等、审计与 outbox 规则保留，HTTP/JSON/事件契约不变。
启用 tracing 时拒绝 SDK 的两个环境导出头（含空值、非 Unicode 值），不修改环境或回显值。
CI 静态入口合并为 `ci/check.sh`，敏感模板统一为 `ops/config/examples/secrets.toml`；Justfile 运行/迁移配方显式选择 postgres。
精简重复文档、注释和默认值说明；删除 48 个失效/重复文件，未删改已发布 SQL 或删除已有测试。

Rust 1.99.0 的 `just verify` 通过：48 份文档、格式/策略/布局/配置/i18n、工作区全部目标、
独立 PostgreSQL/Redis、全部目标/特性 Clippy（`-D warnings`）及两端 WASM。MSRV 1.94.1 的
`--locked --workspace --all-targets --all-features` check、CI 脚本 Bash 语法和 ShellCheck 均通过。
`cargo audit --deny warnings` 扫描 536 条锁记录/1329 条公告，无漏洞或警告；cargo-machete 无未用直接依赖。
全工作区/全 feature/全 target 活动图无 `sqlx-sqlite`、`libsqlite3-sys`；锁图可选项不等于活动驱动。
`just web-build` 完成 Public/Admin 的 wasm-release payload；五程序以 `--locked --release -j 1`
及 `postgres,cache-redis` 完成 API-only 构建，readelf 确认为 x86-64 ELF64/GNU glibc；Fat LTO/单 CGU/unwind 生效。
构建后完整 `just verify` 再次通过；日志位于 `target/dependency-review-20261015/cleanup-*.log`。
未新增或运行行为/性能测试、服务、迁移或部署，未执行正式 `release-check`，不宣称最优性能或生产验收。

### 2026-10-15 依赖审计与编译优化

覆盖全部 20 个工作区包、50 项直接依赖及完整锁图，决策和公告链接见[技术栈](tech-stack.md#2026-10-15-全依赖审计)。
提高 19 项直接版本声明、更新 83 个锁定版本记录；全 feature/全 target 活跃节点 486 → 485，
锁图 535 → 536（外部包 515 → 516），不把依赖条目变化等同于体积或性能收益。
工作区统一版本/profile/CI，摘要 API、严格 cron、CSR 及当时 SQLite 编译面的调整见上述决策表。
五个 apps 增加 Linux x86_64 编译期守卫，正式发布 GNU/glibc，WASM 保留；未批量删除条件平台代码或锁记录。
后端 release 与当时两份 AMD 配置统一采用[生产 profile](tech-stack.md#编译优化边界)，WASM 不混入后端 CPU 参数。
HTTP/JSON/事件、迁移、运行 TOML、会话算法、摘要字节及事务/幂等不变量不变。

Rust 1.99.0 工作区 `--locked --all-targets --all-features` check 和 MSRV 1.94.1 同范围 check 通过。
平台守卫落地后完整 `just verify`（49 份文档）再次通过；MSRV 同范围重查通过。
首次 Clippy 命中的两处原子 API 弃用仅作最小 MSRV 兼容例外，新名在 1.94.1 的静态编译明确未稳定。
SQLite check 输出定位两个实际 C archive，以 nm/strings 核对 FTS 符号及编译项、扩展加载符号均不存在，
OMIT_LOAD_EXTENSION 存在；没有运行 SQLite 查询或不可信输入。Web 发布构建与 release 链接证据待收口后登记。
cargo-machete 0.9.2 未报告未用直接依赖。RustSec 基线命中 rustls 四条公告，升级至 0.23.46 后为零漏洞、零警告，
无忽略项；`just dependency-audit dependency-unused` 通过，门禁为全部警告失败。cargo-audit 0.22.2
扫描 536 条锁记录 / 1329 条公告，公告库提交 `edb58f5974b83eb8302332821cb6d389777aa351`。
原始锁图/metadata/活跃树、公告 JSON 与日志保留于忽略的
`target/dependency-review-20261015/`，不提交机器日志或临时工具。

当时 SQLite 仍捆绑 3.51.3，SQLx 的 sys 版本上限阻止直接升至 FTS5 修复引擎；禁编 FTS/原生扩展仅是风险缓解，
不能据此称引擎已修复或无未知漏洞。该结论属于移除 SQLite 运行支持前的构建记录。
没有新增或执行行为测试、性能测试、服务启动、迁移、数据库/外部服务操作或部署；不宣称最优性能或生产验收。

### 2026-10-14 Tokio LTS 与 OTel 配套升级

基于 `3511a41` 既有工作树，Tokio 1.51.6 → 1.53.3，声明 `~1.53.3`，仅接收 LTS 分支补丁，
上游支持期至 2027 年 9 月；OpenTelemetry API/SDK/HTTP/OTLP/proto 锁到 0.33.1，tracing-opentelemetry 到 0.34.0。
只升级七个包，其余同版本包依赖边不变；全工作区/all-features/all-target-platforms 活跃节点仍为 486，
锁图仍为 535（外部包 515），未证明体积或性能收益。

核对 changelog、exporter/processor trait、HTTP builder、retry policy 与装配后，observability 唯一适配为
`RetryPolicy::disabled()`：拒绝新增的默认三次重试，保留每批一次导出、失败计数与原预算。
有界队列、flush/shutdown、远端 context、安全错误分类及 Prometheus 路径不变；未新增 TOML/HTTP/事件字段、
迁移或环境覆盖入口，未换框架。保留 64 MiB 请求体保护，超限记导出失败；详见
[兼容边界](../crates/auction-observability/README.md#实现布局与-tracing)。

Rust `1.98.0` 下 observability 独立 `--locked --all-targets` check 与 Clippy（`-D warnings`）通过；
完整 `just verify`（49 份文档）与 MSRV `1.94.1` 工作区 `--locked --all-targets --all-features` check 通过；
修改路径主动 LSP 检查无诊断。按本节开头的 10-14 审计口径扫描 535 条锁记录，退出 0、漏洞 0、警告 0，无新增忽略项。

此次升级未修复 SDK 隐式 headers 环境覆盖，不能据此确认 TOML-only 运行边界。
未检查实际环境/凭据，未新增或执行行为测试、真实 OTLP 导出、scrape、故障/退出验证、数据库/外部业务操作、
部署或 payload 重建；编译和公告扫描不证明生产验收。

### 2026-10-14 框架补丁与指标依赖精简

在 `3511a41` 的既有工作树改动上，定向升级 Tokio 1.51.4 → 1.51.6、直接 reqwest 0.13.4 → 0.13.5、
Clap 4.6.6 → 4.6.7；clap_builder/clap_derive 随之锁到 4.6.7。保留原 HTTP/TLS、运行时及 CLI feature，
未切换 Tokio 支持分支，也未升级 OTel 或引入新框架。Cargo 重选的无关 syn/Windows 依赖边恢复为既有锁定关系，
最终锁图通过 `cargo metadata --locked --all-features` 校验。此步骤仅改依赖清单、锁图和相关文档，保留此前源码改动。

Prometheus 0.14.0 关闭默认 `protobuf`、保留 `process`，改用 plain_model；源码核对确认
[指标与追踪边界](tech-stack.md#指标与追踪)不变，包括 TextEncoder、collector/recorder、0.0.4 文本及独立 OTLP 编码。
`protobuf 3.7.2`、`protobuf-support 3.7.2` 退出活跃图和锁图；reqwest 新增间接 `base64 0.23.1`，直接 0.22.1 不变。
全工作区/all-features/all-target-platforms 活跃节点 487 → 486，锁图 536 → 535（外部包 516 → 515），
净减一个包，未测量体积或性能收益。

Rust `1.98.0` 下 observability 独立 `--locked --all-targets` check 与 Clippy（`-D warnings`）通过；
最终锁图下完整 `just verify`（49 份文档）与 MSRV `1.94.1` 工作区 `--locked --all-targets --all-features` check 通过。
按本节开头的 10-14 审计口径扫描 535 条锁记录，退出 0、漏洞 0、警告 0，未添加忽略项。

当前选择与后续评审边界见[技术栈](tech-stack.md#指标与追踪)。未新增或执行行为测试，未做实际 scrape/逐字节
输出比较，未访问业务数据库或真实外部服务、未重建发布 payload、未部署；静态结果不证明运行兼容或生产验收。

### 2026-10-14 低风险依赖精简

承接下节版本升级，在 `3511a41` 既有工作树实施[五项精简](tech-stack.md#2026-10-14-精简审计候选)：
scraper、Axum、observability 直接 serde、Admin futures-util 与 Gloo feature。
SQLite feature、HTTP/事件字段、配置、迁移、轮询唤醒及卸载逻辑不变。

受控刷新锁图未新增或升级包。全工作区/all-features/all-target-platforms 活跃图去重后 491 → 487 个节点，
退出 `getopts`、`unicode-width`、`axum-macros`、`serde_urlencoded`；前三者也移出锁图，
锁图总数 539 → 536（外部包 519 → 516）。`serde_urlencoded` 仍被未启用的 Dioxus fullstack 等路径锁定，
不能通过手删锁记录消除；serde 和 futures-util 也仍有其他实际调用者。不以节点数宣称制品体积或性能提升。

Rust `1.98.0` 下 observability/integrations/http 各自 `--locked --all-targets` check 通过；
Shared/Admin/Public 各自独立 `--locked --target wasm32-unknown-unknown` check 通过。
反向 feature 图确认 Shared 只有 Gloo HTTP、Admin 增 JSON、Public 增 JSON/WebSocket，没有依赖合并构建补项。
完整 `just verify`（49 份文档）与 MSRV `1.94.1` 工作区 `--locked --all-targets --all-features` check 通过。
按本节开头的 10-14 审计口径扫描 536 条锁记录，退出 0、漏洞 0、警告 0，未添加忽略项。

未新增或执行行为测试、未重建发布 payload、未访问业务数据库或真实外部服务、未部署。

### 2026-10-14 依赖升级与精简审计

基线 `3511a41`。scraper 0.24.0 → 0.27.0（selectors 0.38.0 改用 rustc-hash）、
gloo-net 0.6.0 → 0.7.0、gloo-timers 0.3.0 → 0.4.0。未改 HTTP/事件字段、配置、迁移或业务规则；
共享 GET 保留精确 URL、10 秒头/正文预算及取消守卫，只更新对应说明。
锁图从 543 个包降至 539 个（外部包 523 → 519），fxhash 已移除；非默认 Dioxus fullstack
路径仍锁定旧 Gloo 版本，但未进入当前两端 CSR 或全工作区活跃依赖树。

Rust `1.98.0` 下完整 `just verify`（49 份文档）、工作区全部目标/特性 check 与 MSRV `1.94.1` 同范围 check 通过；
7 个改动路径 LSP 无诊断。额外按本节开头的 10-14 审计口径扫描 539 条锁记录，退出 0、漏洞 0、警告 0、无忽略列表；
本地/CI 既有 dependency-audit 阻断策略未变。

另完成两项只读审计与后端补审，形成[六项候选](tech-stack.md#2026-10-14-精简审计候选)；
当时仅记录候选，未将建议当作删除授权，也未计入包数或体积收益。
未新增或执行行为测试、未调用真实业务服务/数据库、未重建发布 payload、未部署；
真实论坛/邮件/装备解析及浏览器兼容性仍无本轮运行证据，不能以静态通过替代验收。

### 2026-10-14 审计修复

基线为 `c9a2d1e` 工作树，按静态审计修复 A01–A18，并仅将 `quinn-proto` 从 0.11.17 更新至 0.11.18。
保留已有玩家指南与文档改动；未修改历史迁移、业务表结构、HTTP/事件字段或运行配置字段。

Rust `1.98.0` 下完整 `just verify`（49 份文档）、工作区全部目标/特性 check、
MSRV `1.94.1` 同范围 check 及 `git diff --check` 通过。
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

Rust/Cargo `1.98.0`、just `1.58.0` 下完整 `just verify`（当时 51 份文档）与差异检查退出 0。
修复门禁发现的借用、错误映射和 feature 编译问题后重跑，未执行行为测试、发布、浏览器、服务、数据库或迁移。
29 项报告问题、论坛 DTO 附项及三份追加迁移属于该轮；历史流拍 Return、资产冲突、错误归档 Credits 和
旧排期来源仍须独立核对，见[邮件资产核对](../ops/mail-diagnostics.md#历史资产处置核对)。

## 源码核对发现的实现缺口

以下为历史源码修复及未验收边界，SQLite 仅指当时实现。2026-10-14 全仓审计的 18 项
（11 项 P1、7 项 P2）已在源码范围修复，不等于行为验收：

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
外层可能先超时；部署前应按内层预算及控制命令开销重新核算，见[systemd 手册](../ops/README.md#systemd-服务与顺序控制)。
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
- 真实 MoogleMail/E-Hentai 的认证、未知写入结果、Sent/Balance 证明与人工恢复。
- Public/Admin 完整页面、五语言、认证组合、Admin 重启/凭据轮换及旧配置恢复、严格 decoder、实时帧和断线恢复。
- Public 连接总量/用户配额、槽位释放、会话只读复核与撤销延迟、静默连接及分页快照。
- 非空生产数据、目标主机权限与服务编排、制品切换、备份恢复和回执保管。
- 首次设置的本机/HTTPS 代理门禁、未知保存结果与空库初始化；合并部署的 readiness、信号、父进程强杀、迁移失败和升级。

后续获得授权开展行为验证时，应登记制品/源码身份、后端与版本、脱敏环境、时间、输入、预期与实际结果、
证据保存位置和限制。敏感证据保存在受控目录，项目文档只记录安全引用和结论。
静态验证、页面样式完成、空表或路由存在均不足以关闭上述事项。

## 未登记为现有能力

Public 当前实时通道是 WebSocket 加 HTTP Catch-up；未注册 SSE endpoint。
外部 realtime 广播 sink、retention 数据删除/归档 mutation、自动 OpenAPI 生成也不属于当前交付能力。
要新增这些能力，需要独立契约、owner、授权边界及明确的设计与运维说明。
