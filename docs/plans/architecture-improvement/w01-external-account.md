# W1 外部账号调度

[docs/plans/architecture-improvement.md](../architecture-improvement.md) · [docs/plans/architecture-improvement.md#2-执行须知](../architecture-improvement.md#2-执行须知) · [docs/plans/architecture-improvement.md#5-每个工作项的交付规范](../architecture-improvement.md#5-每个工作项的交付规范) · [docs/plans/architecture-improvement.md#7-进度](../architecture-improvement.md#7-进度) · [docs/plans/architecture-improvement/baseline.md](baseline.md) · [docs/plans/architecture-improvement/decisions.md](decisions.md)

| 项 | 内容 |
| --- | --- |
| 范围 | 外部账号调度：快速限界、熔断、三层争用统一调度、访问收口、主动暂停、增量扫描、证据退避、论坛对齐与增量导入 |
| 优先级 | P0 |
| 里程碑 | W1.0 为 M1 首项（D2）；W1.1–W1.3、W1.6、W1.7、W1.9–W1.11 随 M2b；W1.4、W1.5、W1.8 随 M3 |
| 依赖 | W0；W1.11 依赖 W0.5；W1.1、W1.4、W1.11 先有 W6.3；模拟类验收依赖 W6.6；W1.10 依赖 W1.3 |
| 迁移 | 是（暂停开关、扫描水位、论坛游标） |
| 估算 | 15–19 天 |
| 关联缺陷 | D2、D3、D4、D6（详见[docs/plans/architecture-improvement.md#已知缺陷](../architecture-improvement.md#已知缺陷)） |
| 关联决策 | 1、14、15、40、46、55、56、61（详见[docs/plans/architecture-improvement/decisions.md](decisions.md)） |

| 项 | 内容 | 关键约束 |
| --- | --- | --- |
| W1.0 快速限界 | Read Mail 观察改为有界页数或降频；维护从每轮 `tokio::join!` 中拆出为独立循环与独立节奏，不再拖慢下一轮收件；main 与 isekai 错峰触发；先确认共享 reader 对完整已读列表的依赖 | 只减少读取；不改变任何证明或业务结果 |
| W1.1 账号忙熔断 | 识别战斗页/冻结后按账号记录 `busy_until`，退避 1→2→5→10 分钟，期间只放行一次只读探测，其余请求不发网络直接返回 `AccountBusy`；熔断状态留在 Worker 进程内，Admin 与 CLI 的远端读取经 W1.10 走同一熔断 | 熔断只限流，不决定业务状态（不变量 5）；写入类在 sending 围栏前被拒 |
| W1.2 诊断去重 | 同一忙碌期只保存首份战斗页；诊断淘汰按类别优先保留写入未知证据 | 不降低 write_unknown 证据保全 |
| W1.3 统一账号调度 | 把三层串行点作为一个调度问题设计：出队优先级（与已有 `send_intervals` 七类节奏合并为一套；运维告警固定最高，其次是账号类回信（登录 key、注册与换绑结果，决策 61），出价被超提醒按 lot 有效截止先后排在其他类别之前，决策 14）、独占操作锁（长扫描不得在持锁期间进行，或按优先级排队）、`incoming` 锁（长列表扫描分段释放）、请求门（分级令牌桶替换 FIFO `IntervalGate`，桶容量为 1 不允许突发，低优先级保底份额） | 总速率不超过配置间隔；同账号远端写入串行；优先级只影响先后，不影响结果 |
| W1.4 增量扫描 | Sent 与 Read Mail 持久化“已完整扫描到的最高 MID + 时间”水位，常规轮按 MID 前缀扫描新增部分而不是按页号；发送前基线同样改为“发送前最高 MID + 前缀扫描”，消除 100 页悬崖（D3）；Read Mail 水位以 W0.12 确认的排序键为前提，目标已知时找齐即停；每日一次完整校验扫描，超过上限时只告警不阻断发送 | 水位不作为发送或收取证明；缺失或不连续回退完整扫描；影子运行期间新旧结果并行比对 |
| W1.5 证据退避 | 履约证据按年龄指数退避（每分钟逐步放宽到 10 分钟，决策 55），付款截止前后提高频率；晚付款的确认与卖方货款因此最多延后 10 分钟 | 不改变付款期限与恢复策略 |
| W1.6 论坛对齐 | 论坛客户端接入同一调度抽象：优先级、写入未知、熔断与指标 | 论坛写入仍按既有未知结果流程，不自动重发 |
| W1.7 通知优先级 | 出价被超提醒保持不合并、不限额（决策 1）；出队位置按决策 14 在 W1.3 实现；lot 有效截止后尚未发送的提醒在 sending 围栏前退役，不再发送，MoogleMail 投递与论坛 PM 队列同样适用（决策 14） | 截止后不再收到提醒是玩家可见变化，同步 Admin 发布的五语言规则文档与[docs/guide/player-guide.md](../../guide/player-guide.md)；退役沿用预检退役语义，不改变其他邮件 |
| W1.8 配置收尾 | 各项的预算、退避、暂停与通知参数随所在工作项同一提交进入 `worker.toml`（带中文“可选/必填”注释并同步[ops/config/README.md](../../../ops/config/README.md)），其中发送节奏与预算类参数纳入 W7.10 的热读取；本项只做汇总核对，并清理无实际效果的配置（如 `[worker.delivery] concurrency` 对 MoogleMail 的有效上限为 1） | 默认值不超过当前上限；删除字段按配置兼容规则处理 |
| W1.9 主动暂停 | 运维可经 Admin 或 CLI 设置带到期时间的“暂停外部访问”标记（复用 W7.7 的持久化开关机制），Worker、Admin 装备读取与 CLI 远端读取都遵守；用于人工使用账号、事故处置与恢复后的安全启动（[ops/deploy/RECOVERY.md#6-外部历史核对与启动](../../../ops/deploy/RECOVERY.md#6-外部历史核对与启动)）。与已有的按世界暂停收件并存：后者只停业务收件，本项停全部外部访问，二者同时生效时以更严格者为准，并在运维手册写明 | 暂停只阻止新请求，不中断已进入 sending 围栏的写入；到期自动恢复并留审计 |
| W1.10 访问收口 | Admin 手工录入的装备读取与 CLI `mail inspect-sent` 等远端读取改为经 Worker control 代为执行（决策 40），与 Worker 自身请求共用 W1.3 的统一调度与 W1.1 的熔断；Worker 自身的装备来源读取并入 W1.3 的请求门。`auction-cli funds` 依设计须先停止 Worker 并持有凭据所有权，不在此列 | 不在 Admin/CLI 进程持有第二个限速器；Worker 不可用时明确返回能力不可用 |
| W1.11 论坛增量导入 | 按主题持久化“已处理到的最大帖子 ID 及其所在页”游标，每轮只读游标所在页之后的分页与固定楼层所在首页，消除 `maximum_topic_pages` 整场失败的悬崖（D6）；每轮核对游标所在页仍包含该帖子 ID，删帖导致楼层前移或帖子缺失时回退完整扫描；未被引用的 `auction_forum_reply_cursors` 由本项决定改造复用或交 W8.2 清理，并同步[docs/contracts/database.md](../../contracts/database.md)；场次进入 Final 后按最早 lot 有效截止加快该主题的导入节奏；按决策 15 维持导入时刻判定并写明语义，同步 Admin 发布的五语言规则文档、[docs/guide/forum.md](../../guide/forum.md)与[docs/guide/player-guide.md](../../guide/player-guide.md) | 游标只缩小扫描范围，不作为出价证明；帖子 ID 不按楼层偏移推算；缺失或不连续回退完整扫描；编辑过的帖子仍不产生新命令；加速受论坛请求门约束 |

验收（以 W0.10 采集的指标度量）：稳态外部请求不超过请求门容量（配置间隔对应的每秒请求数）的 30%；维护 p95 低于 60 秒且无预算耗尽；收件轮询间隔不受维护耗时影响；在 W6.6 替身上模拟战斗期，每个退避间隔不超过 1 次探测、1 份诊断；Sent 页数超过 `maximum_pages` 时发送仍可完成；论坛主题超过 `maximum_topic_pages` 时导入仍可完成，Final 阶段回帖导入延迟达到 W0.13 的目标；投递积压时运维告警仍先于其他类别发出。回退：各项经 W7.7 运行时开关回到旧路径；水位与游标表只追加，停用后不影响旧路径。
