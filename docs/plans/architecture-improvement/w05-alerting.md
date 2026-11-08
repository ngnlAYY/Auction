# W5 告警通道、来源、分级、处置手册与告警台

[docs/plans/architecture-improvement.md](../architecture-improvement.md) · [docs/plans/architecture-improvement.md#2-执行须知](../architecture-improvement.md#2-执行须知) · [docs/plans/architecture-improvement.md#5-每个工作项的交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [docs/plans/architecture-improvement.md#7-进度](../architecture-improvement.md#7-进度) · [docs/plans/architecture-improvement/baseline.md](baseline.md) · [docs/plans/architecture-improvement/decisions.md](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 告警通道入口、告警来源补齐、外部存活检查、仪表盘口径与告警分级、处置手册、告警台 |
| 优先级 | P1（W5.1、W5.6 暂缓，决策 50） |
| 里程碑 | W5.2–W5.5、W5.7、W5.8 随 M4；W5.1、W5.6 暂缓（决策 50） |
| 依赖 | W5.1、W5.5–W5.7 依赖 W0；W5.2–W5.4、W5.8 依赖 W2 |
| 迁移 | 是（W5.5 扩展告警闭集，W5.8 确认与静默） |
| 估算 | 8–11 天 |
| 关联缺陷 | D9（详见[docs/plans/architecture-improvement.md#已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | 3、16、38、50、58、66（详见[docs/plans/architecture-improvement/decisions.md](decisions.md)） |

| 项 | 内容 |
| --- | --- |
| W5.1（已提交，默认关闭） | 运维告警抽象出通道端口与配置入口，与 MoogleMail 通道并存、可单独启用；外部通道采用通用 Webhook（决策 16），请求体为可配置的 JSON 模板，具体服务与地址作为敏感配置由用户填写。投递账本独立于 `operations_alert_batches` 与 MoogleMail delivery：迁移 `000102` 新增 `operations_alert_webhook_deliveries` 与 `operations_alert_signals.next_webhook_at`；创建时冻结 JSON、唯一 `command_id`、lease/fencing，结果未知只转 `manual_reconcile`，可重试失败最多 5 次后进入 `failed/retry_exhausted`；Dashboard 已显示 Webhook 待重试/发送中/失败/人工核对计数；Admin 运营视图 `GET /api/v1/admin/alerts/webhooks` 已提交（`679e3cac`）。用户仍未启用外部告警（决策 50），待选定服务后启用并部署 |
| W5.2 | 仪表盘状态复用告警策略的阈值评估，消除“>0 即需处理”与告警阈值两套口径；区分正常等待与超时积压：付款期内等待买家付款的 COD、等待前置的卖方货款不计入积压，超过付款期限或预期时长才计入，并复核各默认阈值（现为数量 1）；告警分级（决策 66）：需要立即处理的信号保留 30 分钟冷却重复通知，付款逾期、待重列等非紧急信号改入每日摘要，不单独发恢复通知，已恢复项列入下一封摘要。代码已提交（`a9350b9d`）：Dashboard 与扫描器共用 `assess_delivery`/`assess_fulfillment`/`assess_recovery` 的同一阈值判定（交付/履约/恢复 OR 语义、等待前置需数量与年龄同时达标）；履约快照区分等待付款、等待前置与可推进积压；relist-only 也会使 Dashboard 降级 |
| W5.3（已提交，未部署） | 新增“外部账号”分区：忙碌与暂停状态及持续时间、请求门使用率、维护健康、论坛状态。Worker control 新增只读 `GET /v1/external-accounts`，由 `ExternalAccountTracker` 消费既有外部客户端事件与新增维护轮次事件，经 fanout 与 Prometheus 记录器并行；Admin 分区合并既有邮件轮询暂停与凭据可用性投影。只有整个快照不可用（Worker 停止、control 未启用或 Worker metrics 关闭）时分区为 `unknown`，单个子投影缺失时在 `pollers`/`credentials` 内标记 `unavailable`，不用空列表或零值冒充完成。`canceled` 维护轮次不算成功或失败；维护失败使分区降级。提交 `6f161bdc`、`69fd6654` |
| W5.4 | 告警正文附 W5.7 处置手册的锚点；W10、W11 落地时各自注册备份与对账告警来源。当前已把活动 signal 与已恢复摘要项的 `runbook_anchor()` 附到 MoogleMail 摘要；Admin 可点击链接已提交（`679e3cac`） |
| W5.5 | 补齐告警来源：收件轮询停滞、凭据失效、生命周期逾期（应进入 Final/结算而未进入）、论坛同步与私信失败、磁盘与备份目录容量、外部账号忙或熔断持续、非结算通知投递失败（含邮资不足）、Sent 与论坛主题远端 HTTP 分页页数达到上限的 80%（论坛按 `pagination` 的 `st` 偏移总页数，不能用本地 `auction_forum_catalog_pages` 的目录页数代替）、付款邮件未寄出但已逾期（D9）、换季前 isekai 仍有未完成义务、慢速代理到期后超过阈值仍未执行；告警种类、来源与范围是数据库 CHECK 闭集（迁移 005、020），扩展须新增迁移并按 W7.8a 登记为 `breaking`；Final 期间 Worker、Public 或数据库不可用只告警，不延长 lot 截止（决策 38）。备份、上传与 WAL 归档的失败经本项并入运维告警与仪表盘；线上未启用 MoogleMail 运维告警（决策 58），在它或外部通道（决策 50）启用前，这些来源只在仪表盘与 W5.8 告警台可见，没有主动通知 |
| W5.6（代码已实现，默认关闭） | 外部存活检查采用心跳打卡（决策 16）：Worker 在自身心跳新鲜、数据库就绪且常驻循环正常时，每 60 秒向配置的敏感 HTTPS 地址报到；超时未报到由外部服务告警，覆盖 Worker 崩溃循环与整机宕机。报到地址属于敏感配置，默认空即关闭；启用仍按决策 50 等待服务选定。Public 的 `/health/ready` 已由反向代理对外可达，可在同一外部服务配置 HTTP 检查 |
| W5.7 | 处置手册：在[ops/README.md](../../../ops/README.md)按告警来源组织处置步骤，至少覆盖账号忙、Cookie 失效、Worker 崩溃循环、数据库不可用、磁盘满、HV 页面结构变化、备份失败、邮资耗尽、Bot 账号被盗或被封 |
| W5.8 | 告警台：Admin 新增活动与历史告警列表（来源、范围、首次与最近时间、计数、处置手册锚点），支持确认与带到期的静默（只抑制重复通知，不改变信号与阈值评估），以及每日摘要（决策 66）；运维告警未启用期间（决策 58），告警台是唯一的告警入口；导航显示待办数（人工对账、待确认恢复、活动告警）；仪表盘自动轮询默认开启（30 秒）；同步[docs/contracts/routes.md](../../contracts/routes.md)与五语言译文 |

验收：在 W6.6 替身上模拟账号忙、Cookie 失效，并在本地停止 Worker，三种场景各在阈值时间内经至少一条不依赖该故障的路径产生告警（外部通道启用前以仪表盘为准，Worker 停止与账号忙期间可能没有主动通知）；每个告警来源都有对应的处置手册锚点；正常运营（没有超时积压）的一周内不产生积压类告警；告警台能列出全部活动信号，并完成一次确认与静默到期。回退：新告警来源与通道可分别经配置停用；分级与摘要可配置回逐条冷却通知；告警台为增量页面，确认与静默记录只追加。
