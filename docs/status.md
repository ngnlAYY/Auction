# 实现与验证状态

[文档导航](README.md) · [架构](architecture.md) · [默认验证范围](development.md#默认检查)

本页统一说明能力与证据的区别。2026-10-12 新增两份 [AMD KVM 定向编译候选](../ops/build/README.md)，
并按用户授权完成本机领域微基准初筛；同日按后续指令生成 Ryzen 7950X 四程序定向候选，
随后按独立运维授权在 Ryzen 目标完成空库迁移、三角色启动与就绪核对；仅为限定范围运行证据，
不构成 PostgreSQL 吞吐、完整业务或生产验收结论。
2026-10-11 在既有工作树上收紧竞价计划一致性与代理失败原子性，
并合并 Fast/Slow/Final 反超计算；同日复审的账户能力、代理时间、慢速扫描、重放绑定及拍卖聚合
失败原子性 5 项问题也已完成源码修复，见[领域与应用边界](architecture.md#命令事务与事实)。
这些变更不引入外部引擎依赖、不改变合法竞价规则；该轮未取得新的运行或性能证据。
2026-10-09 在 `4a04214` 基线上修复全仓静态审计的 5 项问题，
范围与取舍见[本页缺口登记](#源码核对发现的实现缺口)。
2026-10-03 的 7 项修复保留于[本页历史记录](#2026-10-03-历史记录)。
本轮不新增或改写迁移，不对应已发布制品，也不构成运行验收。此前 2026-09-22 的 29 项报告问题、
论坛 DTO 附项及三份追加迁移见[本页历史记录](#2026-09-22-历史记录)及[邮件资产核对](operations/mail-diagnostics.md#历史资产处置核对)。

核对范围覆盖根目录、docs、apps、crates、ops、migrations、ci 的正式文档。
本页保留各日期的原始验证结论，不因当前文档修订而把历史失败改写为当时通过，也不把历史声明当作当前运行证据。

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

### 2026-10-13

按用户要求移除独立笔记目录、强制写作模板、导航与专用格式校验，理由和限制直接归入正式文档；
同步协作规范及模块 README，Public/Admin 的设计文档继续分开维护。通用链接、锚点、导航覆盖与
Cargo 包 README 检查保留，本次 `just docs-check` 通过，覆盖 48 份正式文档。
`cargo fmt --all -- --check`、仓库策略、布局、配置示例、i18n 与 `git diff --check` 通过；
文档工具经 rustfmt 和 `rustc --edition=2024 -D warnings` 编译，30 个改动路径主动 LSP 检查无诊断。
本次仅调整文档与静态工具，未运行行为测试、构建部署制品、连接数据库或操作线上服务。
提交前另执行完整 `just verify`，退出码为 0，覆盖格式、48 份正式文档、仓库/布局/配置/i18n、
工作区全部目标、独立 PostgreSQL/SQLite/Redis feature、全特性 Clippy（`-D warnings`）及双 WASM 编译。
本次只编译既有测试目标，不执行行为测试。以下各阶段保留原有范围和执行结果；历史文档门禁失败不改写为当时通过。

按独立授权修复邮件装备读取后的校验取时：每件读取成功后用已注入的 `Clock::now` 校验来源时间，
不改来源时间、不放宽未来时间门禁，也不修改命令原有业务时间、摘要或旧收件/退回事实。
实现及限制见[源码缺口登记](#源码核对发现的实现缺口)。
`cargo check --locked -p auction-application --all-targets`、同范围 Clippy（`-D warnings`）、
`cargo check --locked -p auction-worker --all-targets --features postgres,cache-redis`、Worker 全特性 Clippy
（`--all-targets --all-features -- -D warnings`）、格式/差异及仓库策略/布局/i18n 检查通过。
Worker 单 PostgreSQL 特性的 Clippy 被未改动的 `auction-storage/src/database/admin_mail/backfill.rs:37`
既有 `infallible_destructuring_match` 阻塞；没有用关闭 lint 或修改无关回填代码规避。
当时 `just docs-check` 因 `ci/README.md` 的历史说明链接失效而未通过。主动 LSP 确认改动的 Rust 文件和六份文档无诊断。
未新增或运行行为测试，未重建部署制品、未推送重启服务或重放旧邮件。

同轮针对用户报告的线上规则加载失败，仅查看 Public/Admin 日志及对 Public 的规则接口执行只读 GET：
`lang=zh-CN` 与 `lang=en` 均返回 `503 / rules_unavailable`，对应请求语言与默认英文规则尚未发布；
不是凭据探测或业务冒烟。没有登录管理端、发布规则、修改配置或直接操作数据库，正式规则正文仍需操作者确认并发布。

### 2026-10-12

后续完善 Admin 邮件拒收说明：列表/详情保留原始码并显示五语言解释，装备拒收摘要优先使用已留存诊断，
区分时间/数据异常、规则拒收、定价问题、登录/网络故障和留证失败；不修改业务决定、通知模板或任务门禁。
说明与证据边界见[邮件诊断](operations/mail-diagnostics.md#装备读取失败诊断)。
Rust/Cargo `1.98.0` 下，Admin 原生全部目标与 WASM 的 `cargo check --locked`、两目标 Clippy
（`-D warnings`）、全仓格式/差异和 i18n/布局/仓库策略门禁通过。主动 LSP 确认 4 个改动 Rust 文件干净；
CSS 报告 9 项既有 `!important` 警告，本次仅增加拒收摘要的长代码换行规则，不改这些旧选择器。
当时 `just docs-check` 因 `ci/README.md` 的历史说明链接失效而未通过，未恢复用户删除。
本轮未新增或运行行为测试、浏览器流程、服务或数据库操作，未重建部署制品、未推送远端。
**该消息说明阶段没有修复装备采集/校验取时缺陷；后续 2026-10-13 源码修复单独登记，尚未部署或取得接收运行证据。**

新增 A44 与论坛/游戏 Cookie 管理页的手动只读校验，按凭据版本返回登录有效、失效或无法确认。
共享游戏凭据分别观测两个世界；不持久化检查结果、不改启停或 Worker 所有权，也不新增迁移。
认证证明与只读边界见[外部凭据契约](contracts/routes.md#外部凭据)。
功能实现阶段未使用真实 Cookie 联网验证、未运行行为测试、未重新发布或替换远端制品。
Cookie 校验轮次的工作区全部目标编译、Admin PostgreSQL + `web-embed`、独立 PostgreSQL/SQLite
storage 全部目标、Public/Admin WASM、全特性 Clippy（`-D warnings`）、格式/差异和仓库/布局/配置/i18n
门禁均通过。主动 LSP 检查 29 个 Rust 文件未报告诊断，18 个确认干净、11 个未确认；不以静默响应
代替 Cargo 结果。当时 `just docs-check` 因 `ci/README.md` 的历史说明链接失效而未通过，未恢复用户删除。

同日用户另行授权重编和部署，Ryzen 7950X 新四程序及双 Web 候选已替换指定目标服务。
上传和内嵌资源校验通过，严格停机后保存旧程序及已校验的 PostgreSQL custom-format 备份；
schema 版本保持 39，未执行迁移、恢复或修改配置。三角色就绪、运行制品哈希、启动回执及
双端各 6 个 HTTP 静态资源全部匹配，自动重启为 0，外部两端 readiness 为 200。
详见[候选更新记录](../ops/build/README.md#后续-cookie-校验候选更新)；仍未使用真实 Cookie
调用校验接口，未执行业务流程、浏览器测试或恢复演练，不据此宣称生产验收完成。

使用 Rust/Cargo `1.98.0`，新增两份显式 Cargo 配置，保持默认发布和 WASM 参数不变：

- 两个 CPU 特性白名单均经 `rustc --print cfg` 核对；EPYC 候选不包含 `rdseed`。
- 两份配置各自通过 Public/Admin/Worker/`auctionctl` 的 `cargo check --locked --release`，
  显式选择 PostgreSQL、Redis 和对应 Web feature；不是服务二进制的链接或运行验收。
- 主动 TOML LSP 检查无诊断；`cargo fmt --all -- --check`、仓库/布局/配置/i18n 门禁及
  `git diff --check` 退出码均为 0。
- 当时 `just docs-check` 因 `ci/README.md` 的历史说明链接失效而未通过；未恢复用户删除。

用户明确授权性能测量后，在当前 Intel KVM 上完成 6 组参数 × 10 类领域操作 × 9 轮交错采样。
这只说明本机纯领域负载中 v3/Fat LTO/1 CGU 的等权几何平均吞吐为默认基线的 1.158 倍；
完整定义、原始证据位置与限制见 [ops/build](../ops/build/README.md#已有测量和待补证据)。
两份 AMD 专用微基准程序已经编译但未在本机运行，不能将本机初筛结果外推为目标机器的收益。

初筛阶段没有链接专用服务发布制品；同日后续按用户指令完成 Ryzen 7950X 的四程序 Release
链接和 Public/Admin Web 重建，配置为 PostgreSQL + Redis + 指定 Web feature。实际 CPU/LTO/CGU
参数、页面内嵌隔离、ELF 依赖、构建前后源码与页面身份、压缩包内容及 SHA-256 均静态核对通过。
压缩包位于 `target/amd-artifacts/auction-ryzen-7950x-20261012T044747Z-dirty.tar.gz`，
仅为未提交工作树候选，不属于正式发布归档；原始日志和完整边界见
[重编记录](../ops/build/README.md#2026-10-12-ryzen-7950x-重编记录)。

后续重编未执行服务程序、行为测试、数据库、迁移或部署替换，也未生成 EPYC 服务制品。
PostgreSQL 并发/p99、AMD 现场性能和代表性 PGO 尚未执行；默认行为测试禁令不变。

同日用户另行授权远端排查与配置修复，再明确授权初始化和启动。限定范围结果：

- 修正旧部署路径与权限，严格配置预检通过；无凭据内容变更。
- 三角色严格停机后确认空库，保存普通 pg_dump 快照并校验，使用服务账号和实际 unit 名通过迁移门禁。
- PostgreSQL 18.6：39 条迁移成功、最大版本 39、失败数 0、public 表 64 张。
- Worker → Admin → Public 均 ready，自动重启为 0；本次 invocation 最近 160 条日志窗口无 WARN/ERROR。
- adopted 启动回执已发布，三个角色 receipt 匹配；磁盘四程序和三个运行中制品均匹配候选 SHA-256。

这不是网页首次设置、竞价/并发、外部邮件/论坛、恢复演练或性能验收。脱敏证据位置及完整限制见
[目标初始化与启动](../ops/build/README.md#2026-10-12-ryzen-目标初始化与启动)。

### 2026-10-11

#### 复审修复

使用 Rust/Cargo `1.98.0`，以下静态检查退出码均为 0：

- `cargo fmt --all -- --check`、`git diff --check`。
- `just repository-policy-check layout-check config-example-check i18n-check`。
- `just check feature-check lint wasm-check`：工作区全部目标、独立 PostgreSQL/SQLite storage 与
  Redis cache、全特性 Clippy（`-D warnings`）及 Public/Admin 双 WASM。
- `cargo check --locked --workspace --all-targets --all-features`。
- 修改/新增的 10 个 Rust 文件主动 LSP 检查，无诊断。
- 三个 SQL 修改文件中的 8 条查询/绑定链静态核对，参数数量匹配；未执行 SQL，不能替代 schema
  或事务语义的数据库验证。

当时 `just docs-check` 退出 1，首个失败为 `ci/README.md` 的历史说明链接失效；
没有恢复已删除文件或修改门禁来改变该轮结果。

本次修改领域、应用和两后端存储边界及文档，不新增依赖、修改锁图或迁移；未新增/运行行为测试，
未启动服务、连接数据库或部署，也未重编发布包。已有错误历史时间或损坏账本不会自动修复；
静态检查通过不表示运行验收完成。

#### 首轮领域优化

使用仓库 Rust/Cargo `1.98.0`，以下检查退出码均为 0：

- `cargo fmt --all -- --check`、`git diff --check`。
- `just repository-policy-check layout-check config-example-check i18n-check`。
- `cargo check --locked --workspace --all-targets`。
- 独立 PostgreSQL/SQLite storage 与 Redis cache 特性的 `--locked --all-targets` 编译。
- `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`。
- `just wasm-check`：Public/Admin 双 WASM 编译。
- 本轮六个修改的 Rust 文件经主动 LSP 检查，无诊断。

当时 `just docs-check` 因历史文档目录缺失及残留失效链接未通过。
保留工作树原有删除与无关改动，没有恢复已删除文件或修改门禁；该轮完整 `just verify` 未通过。

本轮仅修改领域实现与对应文档；未新增依赖、修改锁图或迁移，未新增/运行行为测试、性能基准、
服务、数据库或部署流程，未重编发布包。较强校验与静态编译不构成历史数据兼容性或生产验收的运行证明。

### 2026-10-09

使用仓库 Rust/Cargo `1.98.0`，以下代码静态检查退出码均为 0：

- `cargo fmt --all -- --check`、`git diff --check`。
- `just repository-policy-check layout-check config-example-check i18n-check`。
- `just check feature-check lint wasm-check`：工作区全部目标、独立 PG/SQLite/Redis 特性、
  all-features Clippy（`-D warnings`）与 Public/Admin 双 WASM 编译。
- `cargo check --locked --workspace --all-targets --all-features`。
- `cargo +1.94.1 check --locked --workspace --all-targets --all-features`。
- 修改/新增的 18 个 Rust 文件经主动 LSP 检查，无诊断；最终 WebSocket 调整后已重新检查。

当时 `just verify` 退出码为 1，停在 `docs-check`：`ci/README.md` 的历史说明链接失效。
工作期间原有 7 份已跟踪历史说明被外部删除，用户确认保留删除；未恢复或改写这些文件。
因此完整文档门禁未通过，其他静态门禁已独立执行通过，不能把本轮报告为完整 `just verify` 通过。

本轮未执行依赖公告审计；`Cargo.lock` 仅增加 auction-http 到既有 thiserror 2.0.20 的直接依赖边，
没有升级任何依赖版本，历史 SQL 无变更。未新增、改写或运行行为测试，
未启动产品、连接数据库、执行迁移或浏览器流程；代码静态结果不构成生产验收。

### 2026-10-03 历史记录

使用仓库 Rust/Cargo `1.98.0`，以下命令退出码均为 0：

- `just verify`：格式、52 份文档、仓库/布局/配置/i18n、工作区和独立 PG/SQLite/Redis feature、
  all-features Clippy（`-D warnings`）与双 WASM 编译。
- `cargo check --locked --workspace --all-targets --all-features`。
- `cargo +1.94.1 check --locked --workspace --all-targets --all-features`。
- `just dependency-audit`：cargo-audit `0.22.2` 使用 `--deny yanked --deny unsound`，检查 543 个
  crate 条目，未命中漏洞、撤回或 unsound，保留一项 `fxhash` 停维提示；公告库提交为
  `f8dee89e1b2f2f1eaf548312df7655fe5202a302`。额外发现的撤回版本 chacha20 0.10.1 已升至 0.10.2。
- `git diff --check`；历史 SQL 文件无修改。

辅助 SQL 静态扫描覆盖 39 份 PostgreSQL 迁移与 737 个 Rust SQL 候选；两份受限兼容执行视图
解析通过，686 条可识别直接绑定链未见参数数量不匹配。历史原文保留保留字错误，由兼容视图执行；
HTTP/Redis 文字和动态拼接片段的误报已人工区分。8 份 PL/pgSQL 的解析器 JSON 输出存在工具限制，
不据此声称全部函数体、schema、事务或运行行为已验证。
LSP 未完成：固定工具链缺少 `rust-analyzer`，返回
`Unknown binary 'rust-analyzer' in official toolchain '1.98.0-x86_64-unknown-linux-gnu'.`；
可安装对应组件或修正 `pi-lsp.json` 的 server command，本轮不改全局工具链。
编译及 Clippy 结果独立有效，不冒充 LSP 通过。

未新增或运行行为测试、发布构建、浏览器流程、产品服务、数据库或迁移；上述结果不构成生产验收。

### 2026-09-22 历史记录

2026-09-22 使用仓库 Rust/Cargo `1.98.0` 和 just `1.58.0` 执行完整 `just verify` 及
`git diff --check`，退出码均为 0。完整门禁包括：

- `just fmt`、`git diff --check`；
- `just docs-check`：覆盖 51 份文档，本地链接、锚点、导航、包 README 及当时的其他文档门禁通过；
- `just repository-policy-check`、`just layout-check`、`just config-example-check`、`just i18n-check`；
- 工作区 `--locked --all-targets` 编译、独立 PostgreSQL/SQLite storage 与 Redis cache 特性编译；
- 工作区 `--locked --all-targets --all-features` Clippy，`-D warnings`；
- Public/Admin 的 `wasm32-unknown-unknown` 编译。

本轮同时完成各修复范围的独立源码复核；完整门禁中发现的借用、错误映射及特性编译问题已修正后重跑。
未运行行为测试、发布构建、浏览器流程、服务、数据库或迁移。上述结果不能替代制品或生产验收。
历史流拍 Return、入站资产冲突、已错误归档的 Credits 与缺少排期来源的旧数据保留证据边界，
不会通过静默重写或自动重发宣称修复远端资产；见[邮件核对](operations/mail-diagnostics.md#历史资产处置核对)。

## 源码核对发现的实现缺口

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

`fxhash 0.2.1` 的 RUSTSEC-2025-0057 是停维提示而非已确认漏洞，暂保留解析依赖，
2026-11-03 前复查上游兼容迁移；CI 不隐藏该提示，维护边界见[技术栈](tech-stack.md#版本与升级)。

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
