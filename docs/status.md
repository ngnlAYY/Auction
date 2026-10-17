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
| HTTP 与页面 | Public 26、Admin 46 条 feature API；15/21 条浏览器入口 | 每条流程已经通过浏览器或部署验收 |
| PostgreSQL | 完整竞价、结算/恢复、论坛、邮件、凭据、运维 owner | 真实外部服务与生产数据正确性已获证明 |
| SQLite 历史资产 | 运行支持已移除，保留已发布迁移与已有测试 | 保留文件或锁图可选依赖意味着当前支持 SQLite |
| 迁移 | PostgreSQL 48 个追加迁移文件；SQLite 13 个历史文件保留 | 任一目标数据库已经运行到该版本 |
| 发布 | API-only/embedded 隔离、payload 清单、确定性归档门禁及默认分支/手动 Actions 制品任务 | 新工作流已经完成制品上传、部署或运行验收 |
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

### 2026-10-17 论坛无装备通知分类遗漏（本地修复与定向复审完成）

前轮定向复审中，funding 三项及 evidence 期限均为 `Merge verdict: OK`；execution 为 `BLOCK`：
真实 producer 的 `forum_auction_notice` 复用 mail_intake 与 `forum:…` origin，但分类器遗漏其 origin/模板，
使合法零资产通知仍阻断两世界内部履约。该历史阻断保留，本次不修改或重审已通过的资金/证据源码。

以冻结 tree `25cb78b517c79dd2a008cf525ec254d878708fbb` 为输入，仅修改 storage conflicts 分类器及直接 README/本节：
按同 command_id、同 world、command_kind=notify、零命令金额的既有论坛命令账本，重建 producer 完整
`forum:<auction_uuid>:<thread_id>:<post_id>:<line>:<lot_code>` origin；再核对 origin_id=external_mail_id、
精确模板/结果码、零 Credits/COD、空附件及无 key/正文。关联在原事务快照只读，不反向加论坛业务行锁。
只将已证明无装备的通知排除，不凭前缀或 succeeded 泛化放行；错误 owner/world/载荷和未知 schema 仍阻断，
真实资产、旧 unknown、精确 recovered_handoff 例外及原统一锁序保持。

本次定向 rustfmt/格式检查、`cargo check --locked -p auction-storage --all-targets --no-default-features --features postgres`
及一次完整 `just verify` 均通过，覆盖45份文档、格式/策略、工作区全部目标、PostgreSQL/Redis、全特性 Clippy `-D warnings` 与双端 WASM。
工具链 rustc/cargo 1.99.0，主线 target、离线依赖和本任务私有 TMPDIR/日志。
最终 execution 定向复审已在冻结 tree `f79459b1ffeacda6ca7c437edbbf65eb72b34790` 完成，结论为 `Merge verdict: OK`：
原论坛通知遗漏 P1 闭合，本次修复影响区未发现新增 P0/P1/P2。此前资金三项和证据期限的定向复审均已通过，相关源码未再改动；
至此五项初次复审发现全部闭合。父核验冻结源码、补丁、清单及实际静态日志一致；复审后只收尾本节，代码与迁移保持冻结，复核格式和文档门禁。
以下记录保留各阶段原有的待复审状态及阻断事实，不代表当前仍有这些代码复审阻断。
未新增/改写/运行行为测试，未运行 LSP、SQL、迁移、服务、真实配置/秘密读取、外呼、浏览器或发信，
未操作旧32条未知投递及草稿，未 commit/push。真实数据库/协议及大库扫描运行证据仍待另行授权。

### 2026-10-17 Bot 内部履约五项复审发现（本地修复，待定向复审）

初次三路独立复审已完成，父采纳资金三项、装备冲突一项及现金 fence 期限一项；本节不改写此前静态通过记录或初次阻断结论。
三组件以冻结输入 tree `19e03eb65eaf56d93327420134e059532cbcb0c5` 分别输出2/1/5文件增量，核验后由主线唯一 writer 一次整合：

- funding：preview/apply 共用 snapshot 阻断未闭合外发 COD opening（Main 装备及跨世界现金 carrier）；
  补偿/隔离新增需求明确零下界而保留既有占用；collected 在满足/发行/解依赖前检查同一现金，缺口确定提交冻结并延期。
  跨 opening 来源拒绝与来源唯一性保留。
- execution：按真实 origin/payload schema 排除已验证无装备通知，附件与 secret 引用按真实 EID/world 判断；
  未知/畸形载荷和真实同资产占用仍阻断，旧 unknown、精确 recovered_handoff 例外和统一锁序不变。
- evidence：prepared 原单调 deadline 经兼容窄端口 `start_sending_authorized_until` 传至现金 fence，
  Worker 收紧为原证据/有效租约/finalize 最小截止；同一 guard 覆盖重试，PostgreSQL 18 transaction_timeout 覆盖整事务与 COMMIT。
  截止后回执、错误/取消或提交未知撤销 owner，不 poll 发送；旧 Some 入口缺精确期限明确拒绝，None 保留受保护 legacy 门。

本次显式 `cargo check --locked --workspace --all-targets --all-features` 与完整 `just verify` 均一次通过：
45份文档、格式、仓库/布局/配置/i18n、工作区全部目标、独立 PostgreSQL/Redis、全特性 Clippy `-D warnings` 与双端 WASM。
工具链 rustc/cargo 1.99.0，主线 target，离线依赖及任务私有 TMPDIR/日志；8个源码文件逐字节等于组件冻结输出，未需额外机械编译接缝。
正式 owner README、资金 CLI、不变量及升级说明同步。下一门禁为父核验冻结制品后，由原三位 reviewer 定向复审五项及影响区。
未新增/修改/运行行为测试，未运行 LSP、迁移、服务、资金 CLI、外呼或访问秘密，未 commit/push；
旧32条未知投递及草稿未动。PostgreSQL 时序/并发、真实 Main/Isekai 协议、运行对账和大库扫描仍待另行授权运行证据。

### 2026-10-17 Bot 内部履约与现金保护（静态整合，待独立复审）

在 `ad33eb7` 基线上一次整合证据/资金 foundation、内部履约及现金发送门三个冻结补丁，保留此前修复。
credentials/integrations 拥有 HV认证UID+论坛目标资料名、严格正向 Armory 库存与全局 mail generation；
funding 拥有真实对账/来源/用途/未知占用，fulfillment 拥有原义务内部证明和原子依赖推进，delivery 拥有全部现金发送门。
Worker 的内部履约和现金 transport 注入同一 gateway/client/凭据集合/owner，资料 reader 仅在 forum.enabled 下装配。
旧 None 预期名可联合核验，配置/keyenv 无持久内部身份；forum关闭或缺证据时明确延期。Admin Cookie check 仍只证明登录可用性。

内部结果不伪造 delivery/Sent，不改变 B+C=S+T 或原 payment→handoff→seller；Isekai 单独证据，退款仍需原恢复授权。
retained/税不自动再投资；有 allocation 的五种新 Admin delivery 动作拒绝，原成功命令先重放。
旧无 allocation 重试已按 recovery→所需业务锁→actions→delivery→fulfillment/reset 接入两阶段预锁。
新增迁移 `000046`—`000048`；[bot-funds](../apps/auctionctl/README.md#bot资金对账) preview 零写、不取 owner、不算可用额度，
apply 才获取真实证据并复核全责任/版本/摘要和精确确认。[手动升级](../ops/README.md#bot-内部履约与资金保护升级)要求可验证备份、
全 writer 停机和同版程序，禁止滚动混用旧 Worker；旧000039顺序升级至000048，普通启动不隐式迁移。

显式 `cargo check --locked --workspace --all-targets --all-features` 与完整 `just verify` 已通过，工具链 rustc/cargo 1.99.0，
主线 target、任务私有 TMPDIR/日志；覆盖45份文档、格式、仓库/布局/配置/i18n、全部目标、PostgreSQL/Redis、
全特性 Clippy `-D warnings` 和双端 WASM。初次检查发现 A 根22项导出缺失、C path模块路径及新结果大enum、
B SQL字面量helper生命周期等纯编译接缝；以真实导出、路径、Box结果、自动解引用等最小修正收口，未改旧测试或关闭lint。
原 delivery lease.rs、慢续租公平性与未知提交撤销owner保留；独立复审仍为下一门禁。

本轮未新增/修改/运行行为测试、LSP、真实配置秘密、资金CLI执行、迁移、服务、外呼、浏览器或发信，未commit/push。
旧32条未知投递及草稿保持原状。真实 Main/Isekai 协议、资金对账、PostgreSQL迁移/并发仍待授权运行证据；
按UUID锁全部deliveries的冲突扫描及ready义务预锁的规模/事务时限须独立审查。以下旧阶段结论按原时间保留。

### 2026-10-17 截止、发送与 Admin 修复阶段交付

三路独立复审及必要的定向复查均收口为 `Merge verdict: OK`，确认发现已修复；最终复查影响区未发现当前明确的 P0/P1/P2：

- integrations/jobs/storage 负责真实发送前检查、同凭据互斥、claimed/sending 续租与发送门禁；
  慢续租后的操作 poll 具有确定机会，仍受请求与租约双截止保护，不复活租约、不重寄未知发送。
- web-admin 负责分页路由级未决命令、凭据导入累计未知与原值/UUID保护，以及首次提交和重试共享的一次性 Key 门禁。
  Cookie/Key 不进入未决集合或持久缓存；正常的路由、认证退出边界不变。
- domain/config/application/storage/auctionctl 负责900/900秒新默认与合格未发布草稿的显式修正；
  现有显式60配置、进行中及已结算场次不会被自动更改。截止复审通过后未重做该范围。

既有审计修复、用户时间修复、弹窗/按钮及 Admin 分页均保留在最终工作树与完整 `just verify` 静态检查范围内。
完整检查包含45份文档、格式、仓库/布局/配置/i18n、全部目标、PostgreSQL/Redis、全特性Clippy和双端WASM，
使用既有 `--locked` 依赖入口；复审后仅追加本节，Rust、SQL、配置及迁移保持冻结，收尾复核格式和文档门禁。
以下记录保留中间阶段的发现及待复审状态，不代表当前仍有这些复审阻断。

**bot 自动免寄/内部结清仍未实现。** 缺可信自身名称、库存与必要资金证明；可选 `player_name` 只是预期值。
`000044`、`000045` 尚未执行，普通重启不自动迁移，后续遵循[手动升级说明](../ops/README.md#截止规则与邮件发送隔离升级)。
旧未知投递、远端草稿及已结算场次未修改；未提交推送、打包部署、启动服务或执行行为测试。阶段源码验收不等于生产验收。

### 2026-10-17 Delivery 慢续租饥饿修复（待第三轮定向复查）

原A确认 prepare 裸等租约问题已修复，但发现续租4秒、间隔3秒时 renew_at 已到期，biased 续租分支持续抢占
准备/发送 future，形成新增P2；Admin C定向复查为OK，截止B保持OK。本次仅改 jobs delivery 调度及所属说明：
准备首次续租独立确认，准备/发送循环均按请求超时 → 租约过期 → 双截止直接保护的操作poll → 到期续租排序。
成功续租后，只要仍在有效预算/单调截止内，操作先获一次poll机会；Pending才处理续租，不随机选择分支。
续租的开始时刻/TTL算法、fence前后确认、SQL有效期门禁及其他已通过范围均未改。

源码推演：lease10/renew3/finalize5、续租4秒时，t=4回执后截止为t=10，操作先poll再续租；
瞬时回执仍按原t+3续租；调度停顿到达请求或租约截止时直接检查阻止poll，即使定时器尚有粒度误差。
该推演未编写或运行测试。完整 `just verify` 使用主线target、offline及既有 `--locked` 依赖入口通过，
覆盖45份文档、格式、仓库/布局/配置/i18n、全部目标、PostgreSQL/Redis、全特性Clippy及双端WASM。
初次编译的Sleep借用冲突在同一白名单文件内通过冻结request_deadline修正后复跑通过；下一门禁仅为原A第三轮复查。
本轮未新增/修改/执行行为测试、LSP、外呼、服务、迁移、秘密读取或提交；bot内部履约仍待真实证据闭合。

### 2026-10-17 独立复审三项发现修复（第二轮静态记录）

三项独立复审已完成：截止规则/草稿修正结论为 `Merge verdict: OK`，本次不改该范围。
父采纳发送 prepare 租约 P2、Admin 导入 unknown P1 与一次性 Key 覆盖 P2，由两个隔离组件修复后整合：

- delivery 在有界 prepare 中为 claimed 续租，fence 前后确认续租并在首次外部 poll 前直接检查单调截止；
  storage 先锁行再校验实时租约、token/fencing，不复活过期 claim。原配置组合、请求预算及同 Cookie guard 保留。
  尚未 poll 的失败丢弃句柄并尽力释放；sending/fence 提交未知不退回 pending，不改变历史未知投递。
- 凭据导入累计 unknown，原表单字段/事件处理器与 UUID 同时锁定；后续401/429/前置业务拒绝或普通409不解除，
  仅明确成功复位。Cookie 只留原表单内存和请求，不进入 pending/持久化。
- Users 首次提交及原命令重试共用同步页级 gate，spawn 前占用；Key 先交页面并保持待确认，关闭展示后才释放。
  其他页面默认并发策略、原行 try_write、页 Scope 与认证/路由退出边界不变。

整合后完整 `just verify` 通过，使用 rustc/cargo 1.99.0、主线 target、离线模式及 `--locked` 依赖检查，
覆盖45份文档、格式、仓库/布局/配置/i18n、全部目标、PostgreSQL/Redis、全特性 Clippy `-D warnings` 和双端WASM。
源码未需额外编译接缝修正，仅同步全局 owner/契约；下一门禁为原A/C审查者对三个发现与修复影响区的定向复查。
未新增/改写/执行行为测试、LSP、迁移、服务、外呼、秘密读取或提交。可信身份/库存及 bot 自动内部履约仍未完成。
以下保留初次整合与静态结果，初次待复审状态不替代本节最新进度。

### 2026-10-17 截止规则与邮件发送隔离整合（初次静态记录）

在既有97文件 WIP 基线之上整合三项增量，保留审计修复、`000044`、弹窗/按钮、Admin分页与 pending owner：

- credentials 贯通 nullable 预期完整玩家名称、版本与历史命令/DTO兼容，新增 `000045`；旧凭据不回填，未提供补绑定入口。
- delivery 的真实只读 compose preflight 位于 sending fence 前，prepared future 连续持有同一 Cookie guard；
  不安全时释放有效 claim、撤销本次领取计数并追加审计，仅本轮暂排对应 world 的 Send；无永久历史错误 SQL gate。
- 后续新场次默认软截止900/900秒，未改变接受时刻加延长/不回退算法；显式单目标草稿修正 CLI 默认只读，
  execute 具备版本、确认、Serializable、原账本重放与追加审计门禁。已有显式60及旧草稿仍需[用户手动升级](../ops/README.md#截止规则与邮件发送隔离升级)。

**自动 bot 内部免寄/付款未完成。** 当前未找到可编码的可信自身名称来源，装备详情也不是当前库存归属证明。
Gateway 无绑定返回 None，有绑定返回明确 `identity_unproven`，不生成 Verified proof；无内部 satisfied/资金写路径或 `000046`。
未接线的私有身份锁已移除，未来同事务 current credential/world/version 与 fencing 门禁保留为正式设计。
旧32条未知投递与草稿资产保持人工核对，不自动清理、重寄或结清。

整合静态执行完整 `just verify` 通过：45份文档、格式、仓库/布局/配置/i18n、工作区全部目标、PostgreSQL/Redis、
全部目标/特性 Clippy（`-D warnings`）及双端WASM；依赖命令使用 `--locked`，主线 target，离线模式。
首次编译发现 application 根导出缺失，补齐后复跑通过。独立安全、截止修正和Admin分页复审仍为下一门禁。
本轮未新增/改写/执行行为测试、LSP、迁移、服务、外呼、秘密读取、提交或部署；SQL并发、真实协议及浏览器运行证据待另行授权。

### 2026-10-17 用户列表时间异常与界面修复（进行中）

经授权以现有 Admin 浏览器会话只读请求用户列表，确认 HTTP `503 temporarily_unavailable`，不是前端 JSON 解码错误。
云端只读事务确认迁移停留 `000039`、43 个用户中 2 个存在 `updated_at < created_at`，其创建时间均匹配 MM `registered` 账本，
旧更新时间均匹配论坛身份同步历史；其余已检查的标识、状态、版本、文本和时间范围未发现异常。未读取 Key、Cookie 或密码。

源码由 application 分离 Users 本地处理时钟与装备入站既有时间门禁；storage 的用户/身份/Key 信封快照更新时间不倒退。
追加 `000044` 仅更正有上述双重证据的倒退行并推进版本、追加审计；其他异常失败关闭，不放宽 Admin 投影或重写创建时间。
Shared Dialog 点击背景保持打开，正常关闭/取消/Esc 和 busy 保护保留；Public 目录按钮样式缩到直属操作按钮，避免覆盖出价/代理弹窗的悬浮与焦点文字色。
上述用户时间与弹窗/样式修复已通过完整 `just verify`（含 PostgreSQL/Redis、全目标 Clippy 与两端 WASM）；
主动 LSP 探测的 3 个路径未能确认，不能用空诊断作为通过证据。Admin 分页仍在隔离实现，按用户确认保留各接口上限，未包含在本次主线静态结果中。
后续浏览器单行抽查因现有管理会话失效返回 `401 unauthenticated`，未取得修复后页面证据；不读取凭据或代替用户登录。
本轮未执行迁移、部署、重启、发信或真实竞价；源码改动不代表线上已恢复，UI 尚无本轮运行验收。

### 2026-10-17 最近五次提交审计修复

只读审计 `367e342` 至 `2842c19` 后确认两项 P2，均由 `450cd52` 引入，分别属于 web-public 与 storage。
装备标记现累计保存原命令未知状态，重试遇 429 不降级；重试复用完整原负载，忙碌/未知时禁止迟到刷新清除命令，
只在成功或明确 409 后解除。MM 内部入队保留 SQLx 错误，由竞价边界识别 `40001`/`40P01`，恢复既有有界整事务重试；
独立邮件入口仍脱敏，不改原子性、锁序、命令身份、发送资格或未知发送不重寄的规则。
无新增路由、DTO、配置、数据库迁移或依赖；只修复审计发现，历史结论保留。
修复后使用 rustc/cargo 1.99.0 执行完整 `just verify` 通过，覆盖 45 份文档、仓库/布局/配置/i18n、
工作区全部目标、PostgreSQL/Redis feature、全部目标/特性 Clippy（`-D warnings`）及两端 WASM；依赖检查使用 `--locked`。
原两位审计者已完成定向只读复查，两项问题均闭合，修复影响区未发现新增 P0/P1/P2，两份结论均为 `Merge verdict: OK`。
复查后仅更新本节记录，源码保持冻结；本轮未新增或执行行为测试、LSP、迁移、服务、浏览器或外部操作。

### 2026-10-17 PM 与论坛结拍复审修复

初次独立复审发现并阻断三项问题：终态遗留 pending 且关闭正文校验时反复 POST 未变化固定帖；
账号偏好首次结果未知后，重试遇 429 会丢失未知状态；论坛帮助仍只称 MM 反超通知。
现已区分部分镜像更新与最终全页确认，pending 不清除且未知写入门禁不放宽；偏好累计保存未知状态，
重试、刷新及提示使用同一状态，完整原命令保留到成功或明确 409；论坛帮助明确账号 MM/PM 选择、MM 默认、
PM 仅反超以及 `notify mm` 单次通知、`notify pm` 不可用。owner 分别为 jobs、web-public 与 integrations，
无额外路由、DTO、配置、迁移或依赖变化。修复后使用 rustc/cargo 1.99.0 执行完整 `just verify` 通过，
覆盖 45 份文档及既有格式、策略、编译、Clippy 与两端 WASM 门禁。原两位复审者已完成独立定向只读复审：
三项问题均闭合，修复影响区未发现新增的具体 P0/P1/P2，两份结论均为 `Merge verdict: OK`。
收口只补记本节结论，Rust、SQL 与配置保持复审时版本；以下保留初次整合记录，不代表仍有未完成复审。

以下两项初次整合记录中的“未运行行为测试”“未使用扩展”仅描述 writer 的显式操作，不能概括父会话。
父会话自动运行器日志另外记录三项目标被调度：一项结果标记 stale、两项因预算延期，无法由这些记录确定实际执行范围。
这些越界自动检查不属于授权验证，不算测试通过或业务验收证据。源码补丁核对未变，日志保留，不改写历史结果。
发现后停止新写入；现经用户确认用任务专用禁用配置重启父进程，旧进程已退出、全局偏好未改，才恢复本次修复。
本次续接只做显式静态检查，不新增/修改/运行行为测试，不调用 LSP、浏览器或外部服务，
不执行迁移、启动部署、发帖发信或提交推送。数据库并发、真实论坛协议和浏览器交互仍待另行授权验证。

### 论坛结拍奖惩展示（本轮静态整合，独立复审待完成）

forum owner 新增 `000043`，历史 settled 明确不纳入，完成版本不伪造；升级时未结算及新场次按原冻结分页刷新。
同一 Repeatable Read 快照核对持久结算、落锤价、赢家、真实来源与连续领先起点；小时整数一位小数，调整精确 PPM 格式、不重算经济规则或补偿。
终态停止新命令导入；live 扫描期间结算也走最终全固定页证明，锁内复核 settled 版本及 content lease/fencing/revision 后才能停止轮询。
保留 pending 诊断和未知写入栅栏；缺事实、正文超限、部分成功均不伪报完成。目录 seller/start/current 与用户竞价语法保持兼容。
本轮未新增/修改/运行行为测试，未迁移、启动服务、读密钥或发帖/发信；数据库并发、升级与真实多页论坛协议未运行验证。
不修改已整合 PM 功能、依赖/锁图及旧迁移。使用 rustc/cargo 1.99.0，显式 `cargo fmt --all` 与完整 `just verify` 通过：
45 份文档、仓库/布局/配置/i18n、工作区 all-targets、独立 PostgreSQL/Redis feature、全目标/特性 Clippy（`-D warnings`）及两端 WASM。
依赖图检查均 `--locked`，all-targets 仅编译既有测试；未使用扩展、LSP 或自动诊断。独立复审仍待完成，不以静态通过代替生产验收。

### 装备账号 MM/PM 竞价警告（本轮静态整合，独立复审待完成）

本轮追加 `000042`，账号渠道 GET/PUT 独立正 i64 CAS 与命令账本，旧 watch.channel 保持 mm 兼容占位。
逐条接受竞价快照冻结渠道，统一来源跨渠道按领先周期唯一；PM 缺论坛 ID 明确 suppressed，不回退 MM。
PM 独立持久队列先 lot 后任务锁进入 sending，原 sender 不可换，任何未知/过期发送只读核对、不重寄。
Worker 独立 PM runtime 共享论坛网关及凭据 owner，不依赖 MM enabled；CLI list/reconcile 只读投影/申请核对。

三个组件 lane 仅执行定向 rustfmt 与差异检查；现已在基线 `450cd52` 主线整合，不修改测试、依赖或锁图。
整合修复 PM 编辑器字体/字号默认控件兼容、请求表单编码、SQLx 静态 SQL 与 CLI 标识转换，偏好用例按布局门禁归入独立目录。
准备失败的任务按持久领取时间及稳定 UUID 轮转，避免坏收件人饿死后续任务；sending 后不重寄的不变量未变。
主线使用 rustc/cargo 1.99.0，`cargo fmt --all` 与完整 `just verify` 已通过：45 份文档、仓库/布局/配置/i18n、
`cargo check --locked --workspace --all-targets`、独立 PostgreSQL/Redis feature、全目标/特性 Clippy（`-D warnings`）及两端 WASM。
编译已有测试目标不等于执行测试；独立复审尚待父级后续安排，不以静态通过声称业务或生产验收。
未新增/修改/运行行为测试，未迁移、启动、发信、浏览器验证或访问真实凭据。历史浏览器单封样本不是后台 Rust Worker 验收。
并发取消/切换/竞价、崩溃/超时/租约、同 sender Sent 核验、论坛限流/拒收/满箱与 UID→姓名投递期间改名风险均未运行验证。
Sent 排序视图可能消耗分页预算、详情标题节点可能带前缀；真实 HTML 兼容仍缺运行证据，超预算或不精确匹配均失败关闭。
受控部署与原 sender 运维限制见[升级门禁](../ops/README.md#装备标记与超价提醒升级)。以下记录保留原历史结论。

### 2026-10-17 装备标记与超价提醒（静态整合）

本轮按登录账号保存装备标记，服务器权威、本地隔离缓存；新建代理默认标记，MM 只按真实领先→被超越周期去重，
取消或重新领先抑制未开始发送的旧提醒，PM 仅预留。涉及追加 `000041`、Public P23/P24 及既有 delivery 发送链，
契约见[HTTP](contracts/routes.md#p23p24-装备标记与超价提醒)和[不变量](contracts/invariants.md#i-09g-装备标记与超价提醒按领先周期去重)。
已整合主线：domain/application/contracts/storage/http/integrations/Public 与装配根贯通，新增三个 watch owner 表，
逐条竞价而非最终快照决定领先周期；同值新命令推进版本，原命令重放不恢复取消。
独立冻结来源 proof 同时保护领取和 sending；开始发送先锁 lot 再锁 delivery，失效未发送任务显式收口，不覆盖未知发送证据。
Public 未知 PUT 只在同账号/场次宿主内保留原负载；主线整合修复 Dioxus 文案 ID move，以及新 watch decoder 的正 i64 契约边界。
按额外明确授权补齐既有 Admin delivery 闭集兼容：lot_watch 标签、严格失效 preflight 形状、禁止旧周期人工重新排队；
没有新增 Admin 页面/列/渠道/动作，也未放宽普通失败、Sent 或未知发送规则。

显式 `cargo fmt --all -- --check` 与完整 `just verify` 已通过：45 份文档、仓库/布局/配置/i18n、
`cargo check --locked --workspace --all-targets`、独立 PostgreSQL/Redis feature、全部目标/特性 Clippy（`-D warnings`）及两端 WASM。
编译已有 test targets 不等于执行测试；本安全 writer 未运行测试、LSP/自动诊断、迁移、服务、浏览器或外部邮件。
初次独立复审发现 P2：共享 lot 锁提前抹去 SQLSTATE，使 Admin 既有 Serializable 三次重试不能识别
`40001`/`40P01`。已将共享锁错误保留为 SQLx，由 Admin 原事务边界分类；Worker 入口仍映射原有稳定错误。
锁序、来源 proof、发送状态与重试上限不变；修复后完整静态门禁再次通过，独立定向复审确认 P2 已闭合，无剩余阻断项。
并发数据库、升级回填、真实 MM 与 UI 行为仍无本轮授权运行证据，不登记为生产或业务验收通过。

本轮只授权实现与静态验证。早期子代理的自动诊断扩展越界启动 Cargo 测试，保留日志中存在测试通过记录；
这不属于获准验证，也不能作为本功能的测试或业务验收证据。自动运行还耗尽临时盘并造成个别文件截断；
旧实例已停止，源码保留后由不加载自动诊断扩展的新原生子代理修复、续接，不改全局工具偏好。
后续仅显式格式、编译和静态分析；本轮未获迁移、部署、浏览器流程或真实发信授权。

### 2026-10-16 论坛首帖计时说明精简

按用户要求，`auction-integrations/forum/render/countdown.rs` 不再在双图片下重复附加整段计时说明。
图片内 UTC 日期、首帖开拍/截止列表、规则及页尾的官网权威提示保留；图片时间来源、P22 HTTP、分页和写入门禁不变。
完整 `just verify`、该 Rust 文件主动 LSP 与差异检查通过。本地独立 18092 预览及 BBCode 下载同步移除该段，
只读 HTTP 核对确认两图和 UTC 时间仍在；记录保存在示例目录的 `removed-timer-note-checks.json`。
另用独立临时 profile 的 headless Chromium 渲染新截图 `forum-preview-without-timer-note.png`，不使用用户浏览器状态。
本次未替换三角色 release 制品，未发帖、运行迁移或部署云端。

### 2026-10-16 论坛自托管动态 SVG

首帖计时器不再依赖第三方域名。application 新增公开时间窄投影，storage 单条 SQL 读取公开边界、实际开拍/计划截止及数据库时钟，
HTTP P22 生成本站 SVG/CSS 数字转盘，integrations 只生成本站链接与 UTC 文字；无新配置、依赖、数据库对象或迁移。
仅展示计划时间，不改变竞价或 soft-close。缺 origin 降级文字，缺截止显示未排期；浏览器/CDN 禁止缓存，图片专用 CSP 禁止脚本和外部资源。

完整 `just verify`、变更 Rust/CSS 主动 LSP、差异检查及原生 release 构建通过。
独立复审发现 HEAD 被旧严格方法 builder 覆盖、亚毫秒被截断两项问题；已新增仅供图片的 GET/HEAD builder、正差向上取毫秒，
复跑静态门禁并通过定向复审。其他 JSON 方法闭集、论坛逐字证明与未知写入门禁未放宽。

用户另行批准本地行为核对：临时进程注入明确标注的内存时间样例，使用真实 Public router，不访问业务库或外部服务。
15 项 HTTP 核对覆盖 SVG 类型、32 KiB 上界、CSP/CORP、双 CDN/浏览器 no-store、HEAD、405/Allow、无效参数、缺场次、
未排期、过期及亚毫秒/最大日期跨度。Chrome 已确认 `<img>` 加载和两次截图图像数据变化，
并用动画时间轴核对分/时/日进位、归零隐藏/终态和 reduced-motion 的绝对日期降级。
跨源图片首次被样例父页自身 CSP 阻止；仅为临时样例 HTML 允许两个回环 origin 后验证成功，生产页面 CSP 未放宽。
临时进程已停止；源码样例、JSON 与截图保留在隔离环境 `state/countdown-preview/`，不是仓库测试或权威拍卖数据。

新 `auction-server`/`auctionctl` 已替换现有隔离实例，复用未改动的两端 Web payload，仍监听 `0.0.0.0:18090/18091`，
PostgreSQL 保持回环。两端经回环及本机局域网地址就绪均为 200；真实空库新图片 GET/HEAD 返回预期 404 且 no-store，
记录在 `state/readiness-self-hosted-countdown.json`。未插入业务样例、运行迁移、发帖、寄信或部署云端。
真实非空库时间投影、论坛图片过滤/代理、远端正文回读、Cloudflare 控制面及其他浏览器仍待单独验收。

### 2026-10-16 Public 自定义数量解除上限

按用户要求取消指定 Public 列表的 200 条数量上限：共享输入、Public URL/偏好、HTTP、应用请求及结果页校验贯通正 `i64`；
Admin、账户与实时事件上限不变，offset、超时及 cursor 互斥继续生效。四处存储游标只对额外探测行使用饱和加法，
请求数量不截断；共享页数计算改用无加法溢出的上取整。无写入规则、数据库迁移或 DTO 字段变化。
五语言错误提示移除最大值文案，并在自定义控件下方显式左对齐。

独立审查发现旧响应解码仍限制 JS 安全整数；已仅为 `page_size` 放宽到完整正 `i64`，其余金额、版本等校验不变。
原 JSON 整数不经过浮点转换，数量标签使用字符串插值以免 Fluent 舍入；定向复审无剩余问题。
完整 `just verify`、变更 Rust 主动 LSP 和差异检查通过；两端 Web 与原生程序已重编，Public 解码修复后再次重建并替换隔离实例。
两端仍监听 `0.0.0.0:18090/18091`，数据库保持回环；Public/Admin 经回环与本机局域网地址就绪检查均为 200。

授权的本地只读核对覆盖空列表：四个无需拍卖 ID 的列表接受 201、1000、超 JS 安全整数及 `i64::MAX`，
三条 cursor 路径接受最大值；无效数量返回 400，共 24 次 API 检查。浏览器确认 1000 可提交保存，
最大值通过 URL 加载及跨列表偏好恢复且不丢精度，0/整数溢出不提交；错误位于表单下方并左对齐。
证据在隔离环境 `state/http-unbounded-page-size.json`、`browser-unbounded-page-size.json`、`readiness-unbounded-page-size.json` 与构建日志。
未造业务数据、出价、发帖或寄信；无真实大数据容量验证，较大数量可能增加响应内存及渲染成本。云端未更新。

### 2026-10-16 Public 数量偏好、论坛计时器与本地隔离启动

Public 指定列表使用统一数字分页，保留 10/20/50/100 预设并支持 1–200 正整数；`surface/pagination/`
拥有 URL 优先级和本地偏好，共享 `components/pager/size.rs` 只拥有输入、校验与可访问交互，Admin 分页不变。
批次和流水偏好独立；非法输入不请求、不保存，浏览器历史明确保留默认数量，首页入口在导航时读取最新偏好。
独立静态审查发现并修复出价筛选失焦与末页超出 API offset 上限；定向复审无剩余问题。
论坛 `render/countdown.rs` 仅为首帖添加 List/Auction Timer，读取本场时间并保留 UTC 文字兜底；无后端请求或网站首页计时器。
数量、偏好与浏览器状态契约见 [Public 设计](../crates/auction-web-public/DESIGN.md#5-表格与信息密度)。

完整 `just verify`（45 份文档）、本次修改 Rust 文件主动 LSP、差异检查、两端 `just web-build` 通过。
`auction-server` 与 `auctionctl` 以 `--locked --release --features postgres,web-embed` 编译；
首页链接修复后重建 Public payload，并清理对应装配根的 release 缓存后重编内嵌 server，未复用旧页面制品。

经用户明确要求，在仓库外 `Auction-Test/local-test/` 建立 PostgreSQL 18.6 专用新库与三角色合并服务：
仅监听回环 55432/18090/18091，缓存、论坛、邮件扫描/发送、告警、自动排期与 OTLP 关闭，装备远程读取失败关闭。
未读取、复用或修改上级云端敏感配置，不连接原有数据库或云端服务，不安装系统服务或 Caddy。
角色配置及严格部署预检通过；真实 server 在空库 schema 门禁处退出并建立 owner 锁，确认无角色进程、
零用户关系及零其他客户端后显式执行 `migrate-managed`。本地迁移账本为 40 项成功、最新版本 40，
随后 Public/Admin readiness 均为 200；这不代表云端已迁移，旧库兼容迁移仍未获本次运行证据。

按用户允许的浏览器调试范围，仅核对隔离空列表：0、负数、小数、科学计数法、加号、空白、空值、字母及 201 均禁用提交，
1/200 可提交；37 保存后重新进入及往期拍卖恢复，明确 URL=10 优先且不覆盖偏好，返回旧条目恢复 10。
首页旧链接冻结数量的问题在本次浏览器核对中发现并修复，重编后链接只含世界参数；中文正整数提示正常。
Admin 登录页可渲染；未登录、出价、造装备数据、真实发帖或寄信，未新增或运行自动行为测试套件。
同次空页快照仍出现首屏“加载较慢”提示，虽页面已渲染且服务就绪，其提示生命周期不在本任务修复范围内。
原始证据保留于本地环境的 `logs/` 与 `state/`，包括构建/预检/迁移日志、制品 SHA-256、readiness 和浏览器检查记录。
论坛第三方图片的真实展示及 BBCode 回读、带业务数据的全列表翻页与外部流程仍未验证。

随后按用户要求，将本地 Public/Admin 监听改为 `0.0.0.0:18090/18091`，精确 Origin 增加本机局域网地址，
数据库仍仅回环、Worker control 仍为私有 Unix socket；未开放任意 Origin、修改防火墙或启用外部集成。
仅改仓库外部署配置并重启该隔离实例，无 Rust/HTTP DTO/数据库迁移变化。角色检查及严格部署预检通过，
本机经回环与局域网地址请求两端 readiness 和局域网页面均为 200；未从其他设备验证路由/防火墙可达性。
仍为无 TLS 的 HTTP 测试环境，只能在受信网络访问，不应映射公网；证据为本地 `state/readiness-all-interfaces.json`。

### 2026-10-16 Cloudflare 与 Caddy Unix 上游

`auction-http` 为正式 Public/Admin 响应默认发送两种 CDN 禁止缓存头，仅静态 handler 的私有标记可放行；
API 浏览器缓存契约保持，HTML/入口禁止缓存，稳定 favicon 每次再验证。Unix listener 不再伪造 TCP peer，
Unix 信任开关不放宽真实 TCP 连接。无新增依赖、配置字段或 HTTP/实时 DTO，TLS/地址清洗由 Caddy 拥有，
精确 Origin、Secure Cookie 与 CDN 响应策略仍归 HTTP；完整部署约束见[运维手册](../ops/README.md#cloudflare-整站代理)。

集成后完整 `just verify`（45 份文档）通过，4 个 HTTP Rust 文件主动 LSP 无诊断。
仓库 Caddy 示例与本地部署配置副本均通过 `caddy adapt`，仅解析配置，不启动或 provision 服务。
部署副本只修改 Public/Admin listener/security、Caddy 和两个 unit，另增 socket 目录定义、Caddy 组权限及说明；
已核对非目标 TOML 区块未变，原件备份目录 0700、文件 0600，不读改敏感配置。

后续按用户要求移除仓库模板及本地 Caddy 的 Cloudflare 来源 IP 拦截；CIDR 只用于代理地址头信任，直连使用真实连接 IP。
该配置调整通过两份 `caddy adapt`、JSON 静态核对、文档与差异检查；未改 Rust 或 CDN 缓存范围，未重新运行整套编译门禁。

后续按用户要求将部署副本的应用/HTTP socket 统一为 `ngnl:ngnl`，移除额外 socket 组，Caddy 保持原用户并附加 `ngnl` 组。
五份非敏感 TOML、三份 unit、Caddy drop-in 与 tmpfiles 定义完成静态核对，Caddy 适配和文档检查通过；未改业务策略。
副本权限审计发现三个敏感 TOML 为 0644、部署 state 目录为 0755：前者不满足严格部署预检，后者若用作总控状态目录也会被拒绝。
既有权限只报告、未自动修改；本轮新建部署模板已设为 0644。未读取敏感正文，数据库目标/数据密钥一致性未验证。
Worker control 的 UID 1000 只与本机账号核对，仍须验证云端 UID；Redis 制品 feature、目录安装和 Cloudflare 控制面也未获运行证据。

本节不代表已安装或上线：未创建用户组、安装 unit/目录定义、重启服务、签发证书、修改 Cloudflare 控制面或运行行为测试。
云端权限、证书续期、真实客户端 IP、缓存命中/排除、WAF、登录及 WebSocket 恢复仍缺运行证据；代码不能抵御错误 CDN 规则强制缓存。

### 2026-10-16 论坛目录分页与更新楼诊断

新主题目录每帖最多 150 件，从 #1 连续分页；系统更新楼位于最后一页之后，用户出价再从下一楼开始。
application 定义冻结布局与续页端口，integrations 负责分页/完整目录解析及限长日志，jobs 负责逐帖对账和命令导入，
storage 的 `forum/catalog.rs` 与追加迁移 `000040` 持久化续页身份/正文，全部证明与主记录同事务、同一 revision CAS 收口。
旧已尝试、已发布及未知写入主题保持单页并继续处理，不挪动已有出价；仅未尝试且无远端身份的队列项及新主题启用分页。
更新楼错误提示改为安全白名单和操作建议，区分待重试、警告、忽略，最多展示 100 条且受字节预算限制；省略不代表解决。
HTML 目录合并保留数字作者门禁；纯文本预览单独处理非权威作者标签。HTTP DTO 不增加字段，正文投影仍为首帖。

论坛变更已通过完整 `just verify`（45 份文档）、29 个 Rust 文件主动 LSP 及差异检查。
独立静态审查发现旧帖兼容和纯文本预览问题，修复后定向复审无剩余问题。
未新增/扩展或执行行为测试，未执行迁移、发帖、启动服务或部署；尚无真实论坛多楼发布/抢楼/未知写入及 PostgreSQL 运行证据。

### 2026-10-15 Actions 门禁修复与下载制品

只读核对首次 [Actions 运行](https://github.com/ngnlAYY/Auction/actions/runs/38757211415)：
`fc28700` 的 docs、security-audit、msrv 通过，verify 因 runner 缺少 `rg` 在布局检查阶段退出 127，尚未进入 Rust 编译。
工作流现显式准备门禁工具，先跑廉价检查，再并行编译、审计和 MSRV；同提交 PR/push 去重及旧任务取消不降低门禁。
增加隔离编译缓存、审计工具复用和固定哈希的官方 dx 工具缓存；默认分支推送或手动运行通过全部检查后，
调用原发布脚本构建并上传通用四程序包。Owner 为 `.github/workflows/ci.yml` 与 `ci/`，
未修改业务 HTTP/事件、运行配置、数据库或发布包内容白名单；调度和下载契约见[CI 说明](../ci/README.md)。

本地完整 `just verify`（45 份文档）、actionlint、ShellCheck、Bash 语法和差异检查通过，变更文件主动 LSP 无诊断。
已静态核对官方 dx 下载归档的 SHA-256、单文件布局和 ELF 依赖；未运行产品、行为测试或部署。
本节登记时尚无新版 Actions 全流程、缓存命中、完整 Release 重编及下载证据，不能将配置完成表述为制品已可下载。

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
- Bot 联合身份/正向库存（含 Isekai）、真实资金对账、generation换主/提交未知、全现金保护与大库冲突锁成本。
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
