# 实时事件与客户端恢复契约

[文档导航](../README.md) · [事件 DTO](../../crates/auction-contracts/src/contracts) · [HTTP 与 WebSocket](../../crates/auction-http/src/http/public/realtime) · [浏览器严格解码](../../crates/auction-web-public/src/api/decoder/realtime.rs)

区分数据库事件信封、Public v1 wire 和客户端快照恢复，任何已发布字段变更均须评审。

本文档定义 Auction Rust 的 Public 实时事件、回放接口和客户端恢复规则。事件注册表与负载校验由
[应用 realtime 端口](../../crates/auction-application/src/ports/realtime/public.rs)持有，JSON DTO 由
`auction-contracts` 持有，HTTP/WebSocket 映射由 `auction-http` 持有；任何不兼容变更都必须新增
schema version，不能依赖仓库外的协议说明。

## 本页导航

- [事件信封](#事件信封)
- [已冻结的事件注册表](#已冻结的事件注册表)
- [流和游标](#流和游标)
- [Catch-up HTTP](#catch-up-http)
- [WebSocket](#websocket)
- [兼容和实现边界](#兼容和实现边界)

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

`stream_key` 是持久化和进程内 Hub 的路由字段，不属于 Public v1 事件 JSON。对外的 HTTP
`events[]` 与 WebSocket 单事件帧只包含 `event_id`、`stream_sequence`、`kind`、
`schema_version`、`payload`、`created_at`；目标流由查询参数和回放响应顶层字段表达。
`created_at` 以及注册 payload 内的时间字符串按 Public RFC3339Nano 规则输出：UTC 使用 `Z`，
小数秒裁掉尾随零，整秒不追加固定六位小数。存储层不得为 PostgreSQL 已丢失的亚微秒位伪造精度。
服务端判断零时间时使用 `0001-01-01T00:00:00Z`；Unix epoch 是合法时间，
不能把两者混为一谈。

事件先以 Draft 形式产生，再由数据库分配流序号。业务状态版本和流序号是两个不同
概念：状态版本用于业务幂等和快照一致性，流序号用于回放和客户端游标。客户端不得
用任意一个字段替代另一个字段。

事件负载不得直接暴露数据库行或内部凭据。每个 kind/version 都必须有显式允许字段
和必填字段；公开投影按精确字段集合、JSON 类型、范围、规范 UUID、world、时间、状态及枚举校验，
未知字段默认拒绝，以便在 HTTP、WebSocket 或 relay 前尽早发现契约漂移。

## 已冻结的事件注册表

当前生产注册表包含十一种事件 kind、十二个 schema。公开端必须保留 kind 字符串和已发布版本；
不兼容修改必须新增 schema_version，不能原地改变已有版本的字段含义。

| kind | 版本 | 必填负载字段 | 目标流或用途 |
| --- | --- | --- | --- |
| auction.created | 1 | auction_id, world_id, sequence, state_version | 公开拍卖发现流和拍卖流 |
| auction.preview_updated | 1 | auction_id, preview_revision, item_count, manual_open, state_version | 拍卖流 |
| preview_catalog_changed | 1 | world, auction_id, preview_revision | 拍卖预览目录变化 |
| auction.opened | 1 | auction_id, world_id, state_version, preview_revision | 拍卖流 |
| auction.schedule_updated | 1 | auction_id, world_id, scheduled_end_at, state_version | 拍卖流；正式开放前任一计划时间变更的重读唤醒，预览/开拍时间从权威快照取得 |
| auction.final_phase_entered | 1 | auction_id, state_version | 拍卖流 |
| auction.early_settlement_requested | 1 | auction_id, state_version | 拍卖流；Admin 已立即关闭竞价并交给结算 Worker |
| auction.cancelled | 1 | auction_id, state_version | 拍卖流 |
| auction.settling | 1 | auction_id, state_version | 拍卖流 |
| auction.settled | 1 | auction_id, sold_count, unsold_count, state_version | 拍卖流；另有版本 2 |
| auction.settled | 2 | auction_id, state_version, policy_version, sold_count, unsold_count, obligation_count | 拍卖流 |
| lot.bid_accepted | 2 | auction_id, lot_id, bid_id, amount, effective_end, state_version, fact_index, mechanism | 拍卖流 |

auction.settled 的版本 1 和版本 2 同时属于历史兼容面；新发布者应根据消费方
能力选择版本，不得把版本 2 的字段偷偷塞入版本 1。lot.bid_accepted 当前冻结
为版本 2，竞价来源、事实索引和有效结束时间都是客户端恢复正确状态所需的数据；
`mechanism` 必须是字符串闭集 `ordinary` 或 `proxy`，服务端公开投影和浏览器都必须拒绝
其他类型或枚举值。

preview_catalog_changed 的负载字段属于已发布 v1 契约，其中 world 是外部契约
字段，不应在 Rust 内部未经映射扩散为数据库列名。其状态版本与预览修订号具有
明确关联，发布前必须通过应用层校验。

## 流和游标

拍卖专属流使用 auction:<auction_id> 形式的 stream_key；发现流使用稳定名称
auction:discovery。拍卖在 draft 创建时只向专属流写入 `auction.created`；当同一拍卖进入公开
preview 后，生命周期事务向发现流追加同 kind 的持久化派生别名。该别名有自己的流序号和
事件 ID，并复用 preview 后的状态版本与来源命令，不构成第二套拍卖事实。这样尚不可公开读取的
draft 不会提前触发发现，而 draft→preview 必定唤醒当前拍卖的权威快照读取。发现流丢失或损坏时
仍可从拍卖专属事件和拍卖状态重建。
外部广播 sink 若消费拍卖事件，必须以拍卖专属流为来源，不应同时消费并再次转发
`auction:discovery` 派生流，否则同一个 `auction.created` 会被重复广播。

同一流内的 stream_sequence 必须严格递增且不可复用。事件表和消费游标是追加式
持久化对象；重放、转发失败和消费恢复不得编辑历史事件或重排已有序号。

## Catch-up HTTP

Public 提供 GET /api/v1/events 作为权威事件回放入口。冻结的查询参数为：

| 参数 | 约束 |
| --- | --- |
| auction_id | v1 单拍卖读取必填；必须是小写规范 UUID，且不能与 `scope` 同时出现 |
| scope | 发现流扩展；只接受显式 `auctions`，且不能与 `auction_id` 同时出现 |
| after | 可选；规范非负十进制，默认 0，从该序号之后读取 |
| page_size | 可选；规范正十进制，默认 100，上限 200 |

响应必须能表达：

| 字段 | 含义 |
| --- | --- |
| auction_id / scope | 单拍卖响应返回规范 `auction_id`；发现扩展响应返回 `scope=auctions` |
| events | 按 stream_sequence 递增的 v1 公开事件数组，不含内部 `stream_key` |
| next_after | 下一次请求使用的游标 |
| has_more | 是否仍有可读取事件 |
| refetch_required | 事件保留窗口不足或无法安全续读时要求读取权威快照 |

当前 Rust 适配器已经把 P03 接到 PostgreSQL 完整档和 SQLite Lite 的
`realtime_events` 持久化读取；按拍卖读取时先校验启用 world 下的拍卖边界。省略
`auction_id` 不再隐式改变 v1 语义；需要发现回放的 Rust 客户端使用显式 `scope=auctions`。
存储适配器不会把未知 kind/schema、非法流名或负载字段不符合注册表的历史行直接返回。
它会置 `refetch_required=true`，但仍扫描本次有界原始页：每个原始序号——包括无法公开投影的行——
都推进恢复高水位，后续可校验事件仍可出现在响应中，`next_after` 最终指向本次扫描到的原始高水位。
流名错误或原始序号不严格递增仍是存储错误，不伪装成普通 refetch。

按拍卖流回放时，发现专用的 auction.created 不重复注入拍卖事件序列。HTTP 外层共享 envelope
要求 `code`、`message`、`data` 的合法类型并允许非破坏性扩展；其 `data`、事件页、事件和每种 payload
都按精确字段闭集解码。客户端遇到序号跳号、游标失效、schema 不可解码或
`refetch_required` 时，必须重新读取权威快照，不能靠本地猜测补齐竞价、资格或结算状态。
带 `refetch_required` 的页不合并其中事件：客户端清空本地事实/版本缓存，保留
`after=max(previous_after,next_after)` 作为恢复游标，并保持已经建立的 WebSocket。

Public 浏览器可将通过上述校验的 `lot.bid_accepted` 合并为竞价资源的重读通知，避免重取
无关的拍卖元数据和内容页面；该优化不改变游标推进、事件防重或命令确认语义。进行中的权威读取
不能被连续竞价反复取消，尚未消费的通知在读取完成后补读；生命周期变化和恢复缺口仍触发全局刷新。

## WebSocket

Public 提供 `/api/v1/ws`：HTTP/1.1 通过 GET Upgrade 返回 101，HTTP/2 通过扩展 CONNECT 返回 200，
其他方法返回 405；两种握手共用会话、Origin、限流与范围门禁。连接必须显式选择一种范围：

- auction_id=<uuid>：订阅单一拍卖流；
- scope=auctions：订阅公开拍卖发现范围。

查询必须且只能包含上述一个键；`after`、`page_size` 或同时出现两个目标都返回 400。每个文本帧
是一个 v1 公开事件对象，不使用 API 外壳，也不发送事件页；发现流只允许
`auction.created`。客户端不得把内部 `stream_key` 当作公开字段。
单拍卖 WebSocket 除核对事件所属 `auction_id` 外，还会把 `auction.created`、`auction.opened`、
`auction.schedule_updated` 和 `preview_catalog_changed` 中的 world 与当前页面 world 核对；HTTP catch-up 按 v1 只冻结
`auction_id` 的事件页目标语义。

连接使用已认证会话，服务端只推送事件，不把浏览器消息当作业务命令入口。启用 origin allowlist
时，握手必须且只能携带一个精确命中的 `Origin`；独立连接尝试限流未拒绝时，错误优先级保持
origin→query→session。连接尝试限流使用自己的固定窗口账本，数值沿用 `mutation_rate_limit`，但两者
不能互相消耗额度。升级成功后不发送 `hello`、`ack` 或其它应用握手帧。每条连接把 frame/message
读取上限都固定为 4 KiB；初始 Pong
期限为 60 秒，收到 Pong 后重新延长 60 秒，服务端每 50 秒发送一次 Ping。事件、Ping 和主动 Close
的每次写入都受 5 秒期限约束。

升级后首次推送前必须只读复核同一 Public 会话；之后每次成功复核按 30 秒与当前会话剩余有效期的
较小值安排下一次检查，单次读取最多 5 秒。检查覆盖持久化撤销、用户/凭据 active 状态、闲置和
绝对过期时间，不更新活动时间或有效期；Pong 不能让会话续命。到期时重新读取权威状态，真实 HTTP
活动已经延长会话则继续连接。注销、封禁、换钥撤销在下一次检查生效，不承诺与命令提交同步关闭；
检查前可能等待当前一次有界写入，关闭帧仍遵守 5 秒写入预算。

连接结束语义由 v1 固定：客户端发送 Text/Binary 时使用 1008/`实时连接只允许服务端推送`；
Hub 淘汰慢消费者时使用 1013/`连接消费速度过慢`；服务端发现无法安全公开的事件或 discovery
收到非 `auction.created` 时使用 1002/`实时事件合同错误`；进程 drain 时使用
1001/`服务正在关闭`。会话失效使用 1008/`会话已失效`；复核超时或依赖不可用使用
1013/`会话校验暂不可用`，失败关闭而不沿用旧认证继续推送。
Tungstenite `MessageTooLong` 读超限会尝试发送空 reason 的 1009 close；
Pong 超时、其他底层 framing/读错误以及事件或 Ping 写失败直接结束连接，
不得伪装成上述应用原因。慢客户端不能无限占用内存，也不能阻塞竞价事务或事件持久化。

所有范围共用每个 Public 进程 1024 条、每用户 8 条活跃连接配额，跨拍卖、会话和 discovery 合计；
额度不足在升级前返回 HTTP 429 `realtime_busy`。Hub 在分配订阅缓冲区前取得配额，失败升级、取消和
订阅结束自动归还，最后一条连接结束后删除用户计数；慢消费者仍持有订阅时不能提前释放配额。
`scope=auctions` 另受 discovery 专属 128 槽限制，耗尽返回 HTTP 429 `discovery_busy`；
普通 `auction_id` 连接不占 discovery 槽。这些是进程内资源保护，不是跨实例集群总配额，
也不替代连接尝试限流。新的单拍卖连接在预检时拒绝已经 `settled` 的拍卖并返回 404；
已建立连接仍可接收随后产生的 `auction.settled` 事件。

WebSocket 只负责低延迟通知，不替代 Catch-up。浏览器限制握手等待为 10 秒；建立后不以业务消息
空闲判断断线，因为浏览器 API 不暴露协议 Ping/Pong。无新竞价时保持连接，存活判定由服务端
Ping/Pong 拥有；关闭或读取失败才进入恢复。浏览器先完成 WebSocket 握手，再从已确认序号
调用 HTTP 回放；握手后到达的帧会在 catch-up 期间缓冲，从而关闭“先回放后订阅”的竞态窗口。
收到不连续序号时再次切换到回放/快照流程。无法保持长连接时使用已登记的 HTTP Catch-up 与权威快照。
SSE 是尚未注册 endpoint 的扩展方向；若后续实现，必须复用同一事件信封、序号和恢复语义。

## 兼容和实现边界

- auction-contracts 拥有 Rust 侧事件 DTO 和外部 JSON 映射。
- auction-storage 拥有事件事实、流序号、消费游标和重放查询。
- auction-application 决定哪些已提交的领域事实产生哪些事件。
- auction-http 只负责 HTTP/WS 输入输出映射和连接生命周期，不实现状态机。
- auction-jobs 负责可靠转发、失败记录和游标推进，外部失败不能撤销已提交的本地事实。
- 缓存和发现派生流都不是领域事实的权威来源；它们丢失后必须能够从拍卖专属事件恢复。

### Worker 消费游标与重放边界

Worker 使用 `(consumer_key, stream_key)` 读取 `realtime_consumer_offsets`。一次确认必须匹配
当前游标的下一个连续 `stream_sequence`，成功确认才推进游标；sink 已收到事件但游标提交
失败时允许按同一 `event_id` 重复投递，因此外部 sink 必须提供幂等键。

PostgreSQL consumer 启动时还会按命名空间化的 `consumer_key` 持有会话级 advisory owner lock；
同 key 的第二个实例必须启动失败，不能共享游标并竞态确认。SQLite Lite 文件库按实际 main
路径在数据库同目录取得同 key 的 0600 sidecar 锁，不能按 URL 或临时目录命名；内存库只在
同进程的同一有效内存身份内互斥。Lite 继续遵守每个角色一个实例和串行写入约束。
Public relay 的目录、消费快照、游标确认及失败记录全部使用该持锁物理连接；连接失效时
失败关闭并撤销 relay readiness，不从普通连接池自动重连继续确认。锁查询前即设置关闭时
丢弃连接，确保取消或退出不会把 session lock 遗留到池内；恢复消费需重新启动并取得 owner。

schema 不可识别、序号出现缺口或外部 sink 失败都会写入 `realtime_failures`，且不会推进游标。
失败记录未解决前，消费会停在该边界。只有在替代事件已经追加到同一 stream 后，运维流程才
能写入引用原事件的 `replay_event_id` 并越过原事件序号；原事件不被修改，替代事件仍按自己的
新序号进入后续消费。这保证重放不会静默重排历史事件。

Rust 已完成 PostgreSQL/SQLite 的游标、失败记录和重放边界适配器，并把可注入的消费 runner
接入现有 Worker 调度循环。Public 进程另有独立的持久化 relay：按 `auction:` stream 目录追赶
事件，把 `auction:discovery` 派生流也交给本地有界 Hub；Hub 队列满时淘汰慢连接，relay 只在
本地 sink 接收后推进对应游标。该链路是当前 Public 实例的 WebSocket 分发边界；若未来
增加外部广播 sink，它属于需要另行冻结协议的新能力。

Admin A21-A24 已接入同一持久化边界：lag、failure 列表/详情只返回安全投影，列表页码保持已发布的
`data.pagination` 信封；replay 的 v1 兼容请求只含 `schema_version` 和公开 payload，HTTP 防腐层按
failure 与修正负载稳定派生 `command_id`。当前客户端也可显式携带并在重试中复用
`command_id`。数据库事务会
锁定原 failure、追加带 `replay_of_event_id` 的新事件、解决 failure、追加 audit，并把结果
写入 `auction_commands`；同一命令重试返回同一事件投影，负载或 failure 不同则返回冲突。

当前已完成持久化 Catch-up、生产事件注册表和 `/api/v1/ws` 的只读服务端推送：连接使用
Public session 和有界并发，服务端从本地 Hub 逐帧发送 v1 单事件并定期 ping；客户端发送
文本/二进制消息会关闭连接，Hub 缓冲区满时对应慢连接会被关闭。
Public Rust 客户端完成握手后先从 HTTP 补齐，HTTP 页与 WebSocket 文本帧共用严格事件 decoder。
单拍卖 WebSocket 的目标 key 同时包含 auction ID 与 world，带 world 的文本帧必须匹配当前目标。
单拍卖缓存有界维护 cursor、seen event ID、bid fact identity、`state_version` 和 `fact_index`：完全相同
的重复事件/事实为 no-op，同一事实身份冲突、序号跳号或版本回退触发权威快照恢复。HTTP 快照恢复
后，第一个晚于恢复 cursor 的事件只为对应 lot 建立 state/fact 基线，随后继续执行严格连续性校验，
避免把合法的 `fact_index > 1` 首事件反复拒绝。带
`refetch_required` 的页按上述规则保留高水位和现有 socket；只有真实连接结束才按 3 秒间隔重连。
拍卖页面订阅单一 `auction:{auction_id}` 流，发现目标使用自己的 `auction:discovery` 游标和缓存，
不能互相污染。页面仍按固定周期读取 readiness、当前/共享拍卖、目录和活动 lot 详情作为权威恢复
兜底；这些读取不替代 WebSocket 的低延迟路径。Public 本地 relay/Hub 提供低延迟分发。未来若连接外部广播系统，必须使用
独立 relay/sink 协议，并避免把本地发现流误报为远端投递证明。
