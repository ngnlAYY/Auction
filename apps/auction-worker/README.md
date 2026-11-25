# auction-worker

[docs/README.md](../../docs/README.md) · [crates/auction-jobs/README.md](../../crates/auction-jobs/README.md) · [docs/guide/settlement.md](../../docs/guide/settlement.md) · [docs/guide/mail-flows.md](../../docs/guide/mail-flows.md) · [docs/guide/forum.md](../../docs/guide/forum.md)

后台任务进程：拍卖生命周期、慢速代理、排期、结算与履约、邮件投递、付款恢复、MoogleMail 收件与维护、论坛同步与回帖、装备私信、运维告警与本机 Worker control。本 crate 只做装配（读取配置、连接数据库与外部服务、取得运行时所有权），把端口交给 [crates/auction-jobs/README.md](../../crates/auction-jobs/README.md) 的调度运行时。未配置数据库时以“骨架”运行（仅 control 与指标），不领取任务、不伪造就绪。

## 启动

`AUCTION_CONFIG_FILE=/etc/auction/worker.toml auction-worker`（本地开发 `cargo run --locked -p auction-worker`，优先读 `ops/config/worker.toml`；仓库模板在 [ops/config/examples/worker.toml](../../ops/config/examples/worker.toml)）。

加载角色配置后先检查一口价挂单的制品能力和所需的数据库配置。开启寄售且制品门放行时，Worker 与 Public、Admin、CLI 使用同一个校验器 `process_consignment_activation`：用 `auction-observability` 计算的本进程构建号与本程序摘要核对页面栈回执 `RECEIPT.v1`，缺 `mount_root`、缺回执或任一项不符即拒绝启动；整个进程只在这里判定一次，结果向下传给邮件轮询。关闭寄售时不读回执，`consignment-dev-preview` 的本地调试构建也不要求回执，规则见[页面静态挂载](../../ops/config/README.md#页面静态挂载)。通过后初始化日志、指标和信号，再连接数据库。启用了 MoogleMail、论坛、告警邮件或告警 Webhook 时也须配置数据库，否则按配置运行骨架。持久装配先同步一口价挂单的准入代次，再装配生命周期、慢速代理、论坛与邮件轮询。实际装载外部凭据时，以 PostgreSQL session advisory lock 取得该种凭据的唯一 owner（每秒检查，失效即排空并非零退出）。完成后才绑定已启用的 Worker control、发布进程心跳并启动持久任务。

MoogleMail 默认 `poll_isolation_enabled = true`：每个世界独立运行收件与维护，维护按自己的日程先 `sweep` 再按最小间隔观察完整 Read Mail；省略维护日程时跟随 Admin 接管后的收件日程，两世界用启动时冻结的相位平移。该 TOML 值是在线开关 `mooglemail_poll_isolation_enabled` 的基线（`auction-cli switch set` 切换，clear 回落）；切换在下一轮或下一次维护门复读后生效，不重写 `mail_poll_state.next_poll_at`。见[ops/config/README.md#mooglemail-收件与维护](../../ops/config/README.md#mooglemail-收件与维护)。

## 外部能力的启用条件

| 能力 | 配置 | 依赖 |
| --- | --- | --- |
| MoogleMail 收件、投递、履约 | `[worker.mooglemail] enabled = true`（默认关闭；默认仅 main，isekai 加入 `additional_worlds`） | 凭据、`equipment_data_key` 与 `user_login_data_key`；数据库凭据另需 `credential_data_key` |
| 论坛同步、回帖与装备 PM | `[worker.forum] enabled = true`（默认关闭） | `equipment_data_key` 与论坛凭据；装备 PM 不依赖 MoogleMail 开关 |
| 运维告警 | `[worker.operations_alert]`：`scan_enabled`（默认 true，只写告警台）、`notify_enabled`（默认 false，MoogleMail 告警邮件）、`webhook_enabled`（默认 false，独立 Webhook）；见[ops/config/README.md#worker](../../ops/config/README.md#worker) | 扫描只需数据库；邮件通知需收件人，告警世界必须在已启用的 MoogleMail 世界内；Webhook 需 `operations_alert_webhook_url` |
| 邮件诊断归档 | `[worker.mooglemail.diagnostics]`（默认启用） | `credential_data_key`，否则用 `equipment_data_key` 作加密父密钥 |
| 一口价挂单解析 | 共用 `[consignment] enabled = true`（默认关闭） | PostgreSQL、装备域当前数据密钥及按需保留的旧钥、固定装备来源。本阶段仅带 `consignment-dev-preview` 的调试制品可开启 |

诊断归档目录默认在 Worker 配置文件旁的 `mail-diagnostics/`，按保留天数（默认 7）与总字节上限（默认 256 MiB）清理；用 `auction-cli mail export-evidence` 离线导出。周排期与早鸟期分别由 `[auction.schedule] enabled`、`[auction.schedule.early_bird] enabled` 控制，默认关闭。关闭邮件或论坛不会停用生命周期、慢速代理与结算；外部履约缺证据时保持等待，不能当作已完成。凭据不是逐项回退：配置完整的 `credential_data_key` 时只读数据库 active 凭据，未配置时邮件用 `keyenv_file` 或敏感配置 `mooglemail_cookie`、论坛用 `ehentai_cookie`。需要联合账号身份证据的流程还依赖论坛目标资料读取端口（仅论坛启用时装配）。

## Worker control 与指标

`[worker.control]` 默认关闭；启用后 Worker 在 Unix socket（默认 `/run/hv-auction/worker/control.sock`，组 `auction-worker-control`）上提供 HTTP/JSON 控制面，连接按 `SO_PEERCRED` 检查 `allowed_peer_uids`/`allowed_peer_gids`。路径、方法与就绪判定见[docs/contracts/routes.md#worker-controlunix-socket](../../docs/contracts/routes.md#worker-controlunix-socket)，socket 与授权实现见[crates/auction-jobs/README.md#worker-control](../../crates/auction-jobs/README.md#worker-control)。Admin、`auction-cli` 与 `auction-server` 都经该 socket 判断 Worker 状态；Admin 与 CLI 需要的外部只读读取（含 Admin 邮件 Cookie 在线检查）也经它交给 Worker 执行，不在本地直连；邮件凭据检查需要 Worker 启用 MoogleMail 并配置 `credential_data_key`，否则该路由报告能力不可用。

附加指标出口由 `metrics.worker_loopback_listen_address` 选择（示例 `127.0.0.1:19102`），独立于 control socket 上的 `/metrics`；行为见[docs/contracts/routes.md#三角色附加指标出口loopback-tcp](../../docs/contracts/routes.md#三角色附加指标出口loopback-tcp)，字段校验见[ops/config/README.md#commontoml](../../ops/config/README.md#commontoml)。

## 一口价挂单

启用时，解析循环按数据库全局许可领单，空闲时至多等 5 秒，复用 Worker 的外部访问暂停与共享排空。固定来源读取的字节上限和本次领取的硬截止由 `[consignment]` 提供。维护循环每 30 秒补扫活动挂单，逐单核实碰撞、资格、停用或到期事实，并在开启时回收过期的解析租约。准入同步失败即启动失败。

第 4 期在准入同步后读取 `has_consignment_work`：单一只读快照检查非终态挂单、active 预约、未终态处置投递、queued/leased 重检、无证明的起写、非终态 position 与未完成的季通知。寄售关闭但仍有存量时，处置维护（Take/Return）、隔离重检与季任务三条循环照常启动；有存量却缺少 MoogleMail 读取器/传输或当前 Credential 数据密钥时启动失败，不能静默跳过。装配使用与 Public/Admin 相同的 `DepositSettings` 构造，并从 Credential 域密钥环构造预约码加解密。

登记准入心跳每 10 秒重新核对数据库及 schema，并依赖已装配的开封器、装备密钥环与来源端口；核对通过后续期至数据库时刻后 30 秒。收件就绪心跳另按 10 秒节拍核对数据库与 schema，读取器签收详情能力时为每个已配置世界续期 `mail_evidence_sessions` 与 `consignment_mail_readiness`（当前有效期 120 秒）。第 4 期生产就绪策略把 `take_ready` 与 `return_ready` 固定为 false：需要取得或退回的整封在首决时分别记 `take_unproven` / `return_unproven` 并隔离，零资产外写，由运营处理；只有维护者提供两世界多件寄件详情、Take 前后与系统 Return 回执的真实脱敏样本并签收后才会改变，业务判定见[一口价挂单](../../docs/business-rules.md#一口价挂单)。指标固定循环名为 `consignment-resolve`、`consignment-maintenance`、`consignment-readiness`，处置、重检与季任务计入 `consignment-maintenance`。各循环共用持久任务的监督与停止预算。

**打开 `take_ready` 的前置清单**（第 5 期结束时的状态，生产仍为 false）：v2 本人与 Admin 读模型、解除止售页与 Public 撤回入口、position 季末终态、库存对账认识代管件、v1 兼容过滤均已交付；仍缺三项：（1）维护者提供两世界寄件列表与详情、Take 前后、系统 Return 回执的脱敏真实样本；（2）验证器按样本改写并签收（`CONSIGNMENT_DISPOSITION_VALIDATOR_VERSION` 去 `-unsigned`）；（3）以单独一个提交把 `readiness.rs` 的写死 false 改为按签收清单读取。运营前置：allowlist 含系统发件人 `MoogleMail` 与实际 Bot、就绪心跳 120 秒续期、Admin 隔离页与解除止售页可用。见[架构](../../docs/architecture.md#一口价挂单)与[一口价挂单的停用与恢复](../../ops/README.md#一口价挂单的停用与恢复)。

**季任务循环。** 仅在寄售装配完成（启用，或关闭但有存量）后运行；它与处置维护、隔离重检并排，空闲 60 秒一轮，失败按 1–30 秒退避，内存定时器只负责唤醒。每轮按顺序：① 寄售启用且日历可用时，用与换季提醒同源的 `schedule_calendar().early_bird()` 经 `ConsignmentCalendar::from_early_bird_config` 加 `isekai_preclose_days` 计算当前季窗并首次冻结，已有 `(world, season_id)` 行原样保留；日历未启用或构造失败只记结构化警告，不冻结也不退出。② 对预停已到、季末未到的冻结季窗，列出该季窗内仍非终态且尚无 `preclose_withdrawal` 通知的 `(position, seller)`，逐条排队通知；预停后新取得的代管件在下一轮补上。③ 对已有通知但无工作的行，在同一事务写入通用 Send 投递与 `mail_disposition_work(action='send')`，投递、工作与 `external_mail_id`/`origin_id` 都由通知 ID 派生，重复调用幂等。④ 季末已到且仍有登记早于季末的非终态旧季挂单时调用 `close_season`；冻结界不因迟跑延长。所有到期判定读数据库墙钟，季窗、通知、工作与投递均以数据库为唯一事实来源。

**处罚执行与权利通知循环。** 处罚执行只依赖数据库，在任何邮件依赖检查之前接入；寄售关闭时，存量探针在上述存量之外另查待执行的处罚账号工作，只剩这类工作也会启动寄售任务收口。循环空闲 60 秒一轮，失败按 1–30 秒退避，每轮至多 64 项，同一处罚至多一次账号效果。处罚的持久事实自相矛盾（如工作已完成却没有效果）时失败关闭：不补写、不重试执行，记错误日志“寄售处罚账号工作的持久事实自相矛盾，已失败关闭，需人工对账”并按失败退避；只要存在这类事实，即使寄售已关闭，Worker 也会一直常驻退避，直到人工对账处理。权利通知循环只在 MoogleMail 读取器/传输与凭据域密钥环都已装配时接入：外部访问暂停期间不领取新工作，领取提交后才解封挑战码并经纯文本发信入口发出；结果未知与租约过期后重领的工作记 `unknown` 转人工，不重发；空闲 30 秒一轮，租约 300 秒。两条循环的指标计入 `consignment-maintenance`。上线后对存量合同补刷一次权利状态，见[ops/README.md#升级](../../ops/README.md#升级)。

**恢复、派发、补寄、下游首轮、退件 Take 与订单复核循环。** 恢复循环（退回处置、自动重列与付款提醒）只写数据库，提醒经通用投递正规入队、外发不在本循环；它与处罚执行一样在任何邮件依赖检查之前接入，自动重列开关与 `[consignment]` 段摘要只在装配根读取一次、重启生效，决定时冻结落库。派发、补寄、退件 Take 三个 executor、下游首轮消费端与订单复核消费端在 MoogleMail 读取器/传输与凭据域密钥环都已装配后接入，与通用投递共用同一个发信传输。下游首轮消费端为已放行、尚未建轮的 isekai 原件交割、卖款与退款义务备好首轮投递并授权首轮（只写数据库，起写与 POST 仍只经派发 executor）。寄售关闭时这六条循环都照存量收口运行：已成交订单的首次派发、补寄与交割、卖款、退款首轮是续办旧权利，新购买已在购买入口截止；存量探针另查恢复循环的两类待办（未处置的退件消费、到期未提醒的已售合同），只剩这类工作也会启动寄售任务。外部访问暂停期间派发、补寄、退件 Take 与订单复核不领取新工作。派发与退件 Take 空闲 5 秒一轮，补寄、下游首轮、恢复与订单复核空闲 30 秒一轮，失败按 1–30 秒退避；六条循环的指标都计入 `consignment-maintenance`。派发结果未知只记未知、留在“已起写、无终局证据”，不重发。派发不起写的两种情形（会话没有持久凭据、冻结身份带 Credits）与订单复核的结果口径见[一口价挂单的停用与恢复](../../ops/README.md#一口价挂单的停用与恢复)。

**收款与确领推进循环。** 与派发 executor 同条件接入（MoogleMail 读取器/传输与凭据域密钥环都已装配），寄售关闭但仍有存量时照常运行。它对已成功派发、已绑定 Sent 的寄售轮只读 Sent 详情（不 POST、不 Take），读到已领取时经既有写口记录证据并推进：main 原件 COD 先 `certify_cod` 再 `prove_handoff`，isekai 载体 COD 只做付款认证，isekai 原件交割只确领，卖款与退款记现金确领（同事务 `claimed`）。验证器未签收时零读取、零推进（签收清单目前为空，生产上不推进）；外部访问暂停时不读远端；严格现金模式还没有只读现金观察入口，收款认证只等待。每次扫描按页（32）走完全部待办，空闲 300 秒一轮，失败按 1–60 秒退避；指标计入 `consignment-maintenance`。

## 退出

第一次终止信号：关闭入口，立即取消未登记交接的工作且不释放 claim；已登记交接或关键段共享一次默认 60 秒截止，之后普通收口默认 10 秒，PM 与主 runner 一起等待。超时或第二次信号强制非零退出，跳过可选 tracing 刷新。独立邮箱维护任务同样受监督：停止后不开始新 `sweep` 或 Read Mail 观察。运行时故障（control 停止、PM 循环停止、持久化循环退出、凭据所有权失效）同样触发排空并**总是非零退出**，由 systemd（`Restart=on-failure`）或 `auction-server` 重启；已领取任务靠持久租约恢复，发送中的外部写入只读对账，不因退出重复发送。

delivery/PM/discard 在持久围栏调用前登记交接，围栏开始即受保护；登记不授权外写，只有围栏确认提交才执行外部操作。论坛在持久 claim 下的第一个实际 POST 前登记。未收口 drop 只减少活动计数并记录未知边界，不补写持久状态、不授权重发。配置、unit 与完整回退见[ops/config/README.md#排空与停止预算](../../ops/config/README.md#排空与停止预算)。

## 数据库与 SLI

启动装配注入 TOML 就绪/SLI/邮件隔离/指标转发拒绝基线并启动开关刷新任务，Worker 另负责为已到期的覆盖追加 expire 事实；语义见[ops/config/README.md#数据库运行时开关](../../ops/config/README.md#数据库运行时开关)，操作见[ops/README.md#运行时开关](../../ops/README.md#运行时开关)。profiled 模式使用 `auction-worker`，`[database_pool]` 默认物理上限 16、长期预留最坏 3（邮件与论坛 Cookie 所有权、投递 LISTEN）；服务超时见[ops/config/README.md#数据库连接分档](../../ops/config/README.md#数据库连接分档)。

有数据库且 `metrics.enabled = true` 时建立独立 SLI 任务：`metrics.sli_snapshot_enabled` 默认 false，经 Worker 专用开关 `metrics_sli_snapshot_enabled` 在线覆盖；每 60 秒至多借一个空闲连接、两秒预算只读轮次，失败只降级指标、保留旧快照。见[ops/README.md#sli-基线与容量观察](../../ops/README.md#sli-基线与容量观察)。

## 部署

unit 示例见[ops/deploy/systemd/auction-worker.service.example](../../ops/deploy/systemd/auction-worker.service.example)（`RuntimeDirectory=hv-auction/worker` 管理 control socket 目录）。每个部署只运行一个 Worker 实例。
