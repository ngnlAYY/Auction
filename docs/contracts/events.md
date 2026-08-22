# 实时事件契约

本文档冻结 Auction-BF 基线中的 Public 实时事件、回放接口和客户端恢复规则，作为
Rust 迁移阶段的契约输入。基线提交为 f109146e246c6bbfefd7f4749ac2bba087bcfa0b，
核对日期为 2026-08-22。

## 事件信封

持久化事件使用以下字段：

| 字段 | 含义 |
| --- | --- |
| event_id | 事件唯一标识 |
| stream_key | 事件所属流 |
| stream_sequence | 流内单调递增序号 |
| kind | 稳定事件名称 |
| schema_version | 该事件负载的契约版本 |
| payload | 经过注册表校验的对象负载 |
| created_at | 事件创建时间 |

事件先以 Draft 形式产生，再由数据库分配流序号。业务状态版本和流序号是两个不同
概念：状态版本用于业务幂等和快照一致性，流序号用于回放和客户端游标。客户端不得
用任意一个字段替代另一个字段。

事件负载不得直接暴露数据库行或内部凭据。每个 kind/version 都必须有显式允许字段
和必填字段；未知字段是否允许由该版本注册表决定，默认拒绝，以便尽早发现契约漂移。

## 已冻结的事件注册表

生产基线当前注册十种事件 schema。公开端迁移必须保留 kind 字符串和已发布版本；
不兼容修改必须新增 schema_version，不能原地改变已有版本的字段含义。

| kind | 版本 | 必填负载字段 | 目标流或用途 |
| --- | --- | --- | --- |
| auction.created | 1 | auction_id, world_id, sequence, state_version | 公开拍卖发现流和拍卖流 |
| auction.preview_updated | 1 | auction_id, preview_revision, item_count, manual_open, state_version | 拍卖流 |
| preview_catalog_changed | 1 | world, auction_id, preview_revision | 拍卖预览目录变化 |
| auction.opened | 1 | auction_id, world_id, state_version, preview_revision | 拍卖流 |
| auction.final_phase_entered | 1 | auction_id, state_version | 拍卖流 |
| auction.cancelled | 1 | auction_id, state_version | 拍卖流 |
| auction.settling | 1 | auction_id, state_version | 拍卖流 |
| auction.settled | 1 | auction_id, sold_count, unsold_count, state_version | 拍卖流；另有版本 2 |
| auction.settled | 2 | auction_id, state_version, policy_version, sold_count, unsold_count, obligation_count | 拍卖流 |
| lot.bid_accepted | 2 | auction_id, lot_id, bid_id, amount, effective_end, state_version, fact_index, mechanism | 拍卖流 |

auction.settled 的版本 1 和版本 2 同时属于历史兼容面；新发布者应根据消费方
能力选择版本，不得把版本 2 的字段偷偷塞入版本 1。lot.bid_accepted 当前冻结
为版本 2，竞价来源、事实索引和有效结束时间都是客户端恢复正确状态所需的数据。

preview_catalog_changed 的负载字段沿用旧系统的命名，其中 world 是外部契约
字段，不应在 Rust 内部未经映射扩散为数据库列名。其状态版本与预览修订号具有
明确关联，发布前必须通过应用层校验。

## 流和游标

拍卖专属流使用 auction:<auction_id> 形式的 stream_key；发现流使用稳定名称
auction:discovery。发现流别名只属于进程内订阅组织，不是可持久化的数据库流名，
不能把它当作第二套事件事实。

同一流内的 stream_sequence 必须严格递增且不可复用。事件表和消费游标是追加式
持久化对象；重放、转发失败和消费恢复不得编辑历史事件或重排已有序号。

## Catch-up HTTP

Public 提供 GET /api/v1/events 作为权威事件回放入口。冻结的查询参数为：

| 参数 | 约束 |
| --- | --- |
| auction_id | 可选；按拍卖流读取时必须是 UUID |
| after | 可选；从该序号之后读取 |
| page_size | 可选；由服务端实施上限 |

响应必须能表达：

| 字段 | 含义 |
| --- | --- |
| events | 按 stream_sequence 递增的事件数组 |
| next_after | 下一次请求使用的游标 |
| has_more | 是否仍有可读取事件 |
| refetch_required | 事件保留窗口不足或无法安全续读时要求读取权威快照 |

当前 Rust 适配器已经把 P03 接到 PostgreSQL 完整档和 SQLite Lite 的
`realtime_events` 持久化读取；按拍卖读取时先校验启用 world 下的拍卖边界，页面大小上限为
200。存储适配器不会把未知 kind/schema、非法流名或负载字段不符合注册表的历史行直接返回，
而是停止本页续读并置 `refetch_required=true`。因此 `next_after` 只推进到最后一个已校验
事件，客户端看到该标记时必须读取权威快照。

按拍卖流回放时，发现专用的 auction.created 不重复注入拍卖事件序列。客户端
遇到序号跳号、游标失效、schema 不可解码或 refetch_required 时，必须重新读取
权威快照，不能靠本地猜测补齐竞价、资格或结算状态。

## WebSocket

Public 提供 GET /api/v1/ws 升级为 WebSocket。连接必须显式选择一种范围：

- auction_id=<uuid>：订阅单一拍卖流；
- scope=auctions：订阅公开拍卖发现范围。

连接使用已认证会话，服务端只推送事件，不把浏览器消息当作业务命令入口。服务端
必须实施有界缓冲、连接关闭和 ping/pong 规则；慢客户端不能无限占用内存，也不能
阻塞竞价事务或事件持久化。

WebSocket 只负责低延迟通知，不替代 Catch-up。重连后客户端必须带已确认的序号
通过回放接口补齐；收到不连续序号时立即切换到回放/快照流程。无法保持长连接的
客户端可以使用 SSE，但必须复用同一事件信封、序号和恢复语义。

## 兼容和实现边界

- auction-contracts 拥有 Rust 侧事件 DTO 和外部 JSON 映射。
- auction-storage 拥有事件事实、流序号、消费游标和重放查询。
- auction-application 决定哪些已提交的领域事实产生哪些事件。
- auction-http 只负责 HTTP/WS 输入输出映射和连接生命周期，不实现状态机。
- auction-jobs 负责可靠转发、失败记录和游标推进，外部失败不能撤销已提交的本地事实。
- 缓存和进程内发现别名都不是事件权威来源；它们丢失后必须能够从数据库事实恢复。

### Worker 消费游标与重放边界

Worker 使用 `(consumer_key, stream_key)` 读取 `realtime_consumer_offsets`。一次确认必须匹配
当前游标的下一个连续 `stream_sequence`，成功确认才推进游标；sink 已收到事件但游标提交
失败时允许按同一 `event_id` 重复投递，因此外部 sink 必须提供幂等键。

schema 不可识别、序号出现缺口或外部 sink 失败都会写入 `realtime_failures`，且不会推进游标。
失败记录未解决前，消费会停在该边界。只有在替代事件已经追加到同一 stream 后，运维流程才
能写入引用原事件的 `replay_event_id` 并越过原事件序号；原事件不被修改，替代事件仍按自己的
新序号进入后续消费。这保证重放不会静默重排历史事件。

Rust 已完成 PostgreSQL/SQLite 的游标、失败记录和重放边界适配器，并把可注入的消费 runner
接入现有 Worker 调度循环。由于旧系统的外部广播协议尚未冻结，当前装配根不注入具体 sink，
所以不会把本地读取伪装成远程转发成功。

Admin A21-A24 已接入同一持久化边界：lag、failure 列表/详情只返回安全投影；replay 请求
必须携带稳定 `command_id`、注册表允许的 `schema_version` 和公开 payload。数据库事务会
锁定原 failure、追加带 `replay_of_event_id` 的新事件、解决 failure、追加 audit，并把结果
写入 `auction_commands`；同一命令重试返回同一事件投影，负载或 failure 不同则返回冲突。

迁移阶段先冻结上述外部形状，再实现生产注册表和适配器。当前已完成持久化 Catch-up
读取边界和 `/api/v1/ws` 的只读服务端推送首片：连接使用 Public session、最多 128 个并发
连接，每 500ms 复用回放端口读取新事件并发送 ping；客户端发送文本/二进制消息会关闭连接。
它仍不是独立的内存广播事实源，重连和跳号依旧必须走 Catch-up/快照流程。
