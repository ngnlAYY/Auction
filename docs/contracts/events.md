# 实时事件与客户端恢复契约

[文档导航](docs/README.md) · [事件 DTO](crates/auction-contracts/src/contracts) · [HTTP 与 WebSocket](crates/auction-http/src/http/public/realtime) · [浏览器严格解码](crates/auction-web-public/src/api/decoder/realtime.rs)

[应用 realtime 端口](crates/auction-application/src/ports/realtime/public.rs) 拥有注册表和负载校验，`auction-contracts` 拥有 DTO，`auction-http` 拥有 HTTP/WebSocket 映射；不兼容变更必须新增 `schema_version`。运行证据见[验证状态](docs/status.md)。

## 本页导航

- [事件信封](#事件信封)
- [已冻结的事件注册表](#已冻结的事件注册表)
- [流和游标](#流和游标)
- [Catch-up HTTP](#catch-up-http)
- [WebSocket](#websocket)
- [兼容和实现边界](#兼容和实现边界)

## 事件信封

| 字段 | 含义 |
| --- | --- |
| event_id | 事件唯一标识 |
| stream_key | 事件所属流 |
| stream_sequence | 流内单调递增序号 |
| kind | 稳定事件名称 |
| schema_version | 该事件负载的契约版本 |
| payload | 经过注册表校验的对象负载 |
| created_at | 事件创建时间 |

`stream_key` 是持久化和进程内 Hub 的路由字段，不属于 Public v1 事件 JSON；对外 `events[]` 与 WebSocket 单事件帧只含 `event_id`、`stream_sequence`、`kind`、`schema_version`、`payload`、`created_at`，目标流由查询参数和回放响应顶层字段表达。`created_at` 及 payload 内时间按 Public RFC3339Nano 输出：UTC 用 `Z`，裁掉小数尾随零，整秒不补六位；存储层不得为 PostgreSQL 已丢失的亚微秒位伪造精度；零时间判断用 `0001-01-01T00:00:00Z`，Unix epoch 是合法时间，二者不可混同。

事件先生成 Draft，再由数据库分配流序号；`state_version` 用于业务幂等和快照一致性，`stream_sequence` 用于回放和客户端游标，二者不能互替。事件不暴露数据库行或凭据：每个 kind/version 的字段集合、类型、范围和枚举均为闭集，未知字段拒绝；公开 UUID 必须小写规范、非 nil 的 RFC4122 v1–v5；整数限于 JavaScript 安全整数范围，序号、版本及 ID 计数按注册表区分正数/非负数；world 最多 64 字节且仅小写字母、数字、`_`、`-`。校验在 HTTP、WebSocket 和 relay 公布事件前执行。

## 已冻结的事件注册表

当前生产注册表包含十一种事件 kind、十二个 schema；公开端必须保留 kind 字符串和已发布版本，不兼容修改必须新增 schema_version，不得原地改变已有版本字段含义。

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

`auction.settled` v1/v2 都属于兼容面，v2 字段不得塞入 v1；v2 要求成交与流拍总数大于零、`obligation_count` 为正数。`lot.bid_accepted` 固定为 v2，`mechanism` 只接受 `ordinary`/`proxy`。`preview_catalog_changed.world` 是外部字段，进入内部模型须映射，预览修订号须通过应用校验。

## 流和游标

拍卖专属流 stream_key 为 auction:<auction_id>，发现流使用稳定名称 auction:discovery。draft 创建时只向专属流写入 `auction.created`；进入公开 preview 后，生命周期事务向发现流追加同 kind 的持久化派生别名，该别名有自己的流序号和事件 ID，复用 preview 后的状态版本与来源命令，不构成第二套拍卖事实（draft 不提前触发发现，draft→preview 必定唤醒权威快照）。发现流丢失或损坏仍可从拍卖专属事件和拍卖状态重建。外部广播 sink 消费拍卖事件必须以专属流为来源，不得同时消费并转发 `auction:discovery`，否则同一 `auction.created` 会被重复广播。同一流内 `stream_sequence` 严格递增且不可复用；事件只追加，消费游标持久化推进，重放和恢复不得编辑历史事件或重排序号。

## Catch-up HTTP

Public 提供 GET /api/v1/events 作为权威事件回放入口，冻结查询参数：

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

P03 读取 PostgreSQL `realtime_events`，先校验拍卖公开边界；发现回放必须显式用 `scope=auctions`。未知 kind/schema 或非法 payload 行不公开，置 `refetch_required=true` 后继续扫描本次有界原始页；所有原始序号（含坏投影）都推进高水位，后续合法事件仍可返回，`next_after` 指向扫描终点；流名错误或原始序号不严格递增直接报存储错误，不伪装成 refetch。发现流的 `auction.created` 别名不重复注入拍卖流。外层 envelope 要求 `code/message/data` 类型合法并允许非破坏性扩展，`data`、事件和 payload 均按字段闭集解码；
跳号、游标失效、不可解码 schema 或 `refetch_required` 必须触发权威快照。refetch 页不合并事件：清空本地事实/版本缓存，保留 `after=max(previous_after,next_after)` 与现有 WebSocket，不猜测缺失业务状态。Public 浏览器可将通过校验的 `lot.bid_accepted` 合并为竞价资源重读通知，该优化不改变游标推进、事件防重或命令确认语义；进行中的权威读取不能被连续竞价反复取消，未消费通知在读取完成后补读，生命周期变化与恢复缺口仍触发全局刷新。

## WebSocket

Public `/api/v1/ws`：HTTP/1.1 GET Upgrade 返回 101，HTTP/2 扩展 CONNECT 返回 200，其他方法 405；两种握手共用会话、Origin、限流与范围门禁。连接必须显式选择一种范围：`auction_id=<uuid>` 订阅单一拍卖流，`scope=auctions` 订阅公开发现范围；查询必须且只能包含上述一个键，`after`、`page_size` 或同时出现两个目标返回 400。每个文本帧是一个 v1 公开事件对象，不使用 API 外壳，也不发送事件页；发现流只允许 `auction.created`，客户端不得把内部 `stream_key` 当公开字段。单拍卖连接除核对事件所属 `auction_id` 外，还核对 `auction.created`、`auction.opened`、`auction.schedule_updated`、`preview_catalog_changed` 的 world 与当前页面 world；HTTP catch-up 按 v1 只冻结 `auction_id` 的事件页目标语义。

连接使用已认证会话，服务端只推送事件，浏览器消息不得作为业务命令入口。启用 origin allowlist 时握手必须且只能携带一个精确命中的 `Origin`；连接尝试限流未拒绝时错误优先级为 origin→query→session，其固定窗口账本数值沿用 `mutation_rate_limit` 但与 mutation 额度互不消耗。升级成功后不发送 `hello`、`ack` 等应用握手帧。每条连接 frame/message 读取上限 4 KiB，初始 Pong 期限 60 秒、收到 Pong 后延长 60 秒、服务端每 50 秒 Ping，事件、Ping 和主动 Close 的每次写入受 5 秒期限约束。

升级后首次推送前必须只读复核同一 Public 会话；有限会话在 30 秒与当前剩余有效期的较小值后再次复核，永久会话固定每 30 秒复核，单次读取最多 5 秒。检查覆盖持久化撤销、用户/凭据 active 状态及有限截止，不更新活动时间或有效期，Pong 不能续命；到期时重读权威状态，明确保存登录时长已修改同一会话期限时允许继续连接。注销、封禁、换钥撤销在下一次检查生效，不承诺与命令提交同步关闭；检查前可能等待当前一次有界写入，关闭帧仍遵守 5 秒写入预算。Pong 超时、其他 framing/读错误、事件或 Ping 写失败直接结束，不得伪装成应用原因；慢客户端不得阻塞竞价事务或事件持久化。连接关闭使用以下冻结 code/reason：

| 原因 | Close code | reason |
| --- | --- | --- |
| 客户端 Text/Binary | 1008 | 实时连接只允许服务端推送 |
| Hub 淘汰慢消费者 | 1013 | 连接消费速度过慢 |
| 非法公开事件或 discovery 非 `auction.created` | 1002 | 实时事件合同错误 |
| 进程 drain | 1001 | 服务正在关闭 |
| 会话失效 | 1008 | 会话已失效 |
| 会话复核超时/依赖不可用 | 1013 | 会话校验暂不可用 |
| Tungstenite `MessageTooLong` | 尝试 1009 | 空 |

所有范围共用每 Public 进程 1024 条、每用户 8 条活跃连接配额，跨拍卖、会话和 discovery 合计；额度不足在升级前返回 HTTP 429 `realtime_busy`。Hub 在分配订阅缓冲区前取得配额，失败升级、取消和订阅结束自动归还，最后一条连接结束后删除用户计数，慢消费者仍持有订阅时不能提前释放配额。`scope=auctions` 另受 discovery 专属 128 槽限制，耗尽返回 HTTP 429 `discovery_busy`，普通 `auction_id` 连接不占 discovery 槽。这些是进程内资源保护，不是跨实例集群总配额，也不替代连接尝试限流。新的单拍卖连接在预检时拒绝已 `settled` 的拍卖并返回 404，已建立连接仍可接收随后产生的 `auction.settled`。

浏览器握手预算 10 秒，先订阅、再从已确认游标 HTTP 回放并缓冲期间到达的帧；存活由服务端 Ping/Pong 判定，业务空闲不算断线，只有真实连接结束才每隔 3 秒重连。跳号切换到回放/快照，无法保持长连接时同样使用这两条恢复路径；SSE 尚无登记 endpoint。单拍卖缓存有界维护 cursor、event ID、bid fact identity、`state_version` 和 `fact_index`：相同事件/事实重复为 no-op，身份冲突、跳号或版本回退触发权威快照；快照恢复后首个晚于恢复 cursor 的事件只为对应 lot 建立 state/fact 基线，再恢复严格连续性校验，允许合法首事件的 `fact_index > 1`。发现流使用独立游标和缓存；页面仍周期读取 readiness、当前/共享拍卖、目录与活动 lot 详情作为兜底。

## 内部投递唤醒提示

PostgreSQL `auction_delivery_ready` 由 storage/delivery_scheduler 与 `000059` 拥有；新 pending 超价 MM 入队时发送，
仅在业务事务提交后送达，载荷固定为空。jobs 经 `DeliveryWakeup` 订阅并合并即时扫描，不携带邮件/用户/凭据，不对外公布，
不新增 Public kind/schema 或替代 outbox、租约及命令账本。断线重订阅后主动扫描，丢失仍靠持久队列和普通轮询恢复；
已有在途发送不抢占，准备失败释放不再次发提示，drain 后停止订阅和新领取。

## 兼容和实现边界

- auction-contracts 拥有 Rust 侧事件 DTO 和外部 JSON 映射。
- auction-storage 拥有事件事实、流序号、消费游标和重放查询。
- auction-application 决定哪些已提交的领域事实产生哪些事件。
- auction-http 只负责 HTTP/WS 输入输出映射和连接生命周期，不实现状态机。
- auction-jobs 负责可靠转发、失败记录和游标推进，外部失败不能撤销已提交的本地事实。
- 缓存和发现派生流都不是领域事实的权威来源；丢失后必须能从拍卖专属事件恢复。

### Worker 消费游标与重放边界

Worker 使用 `(consumer_key, stream_key)` 读取 `realtime_consumer_offsets`；一次确认必须匹配当前游标的下一个连续 `stream_sequence`，成功确认才推进游标。sink 已收到事件但游标提交失败时允许按同一 `event_id` 重复投递，因此外部 sink 必须提供幂等键。

PostgreSQL consumer 启动时按命名空间化的 `consumer_key` 持有会话级 advisory owner lock，同 key 的第二个实例必须启动失败，不能共享游标并竞态确认。Public relay 的目录、消费快照、游标确认及失败记录全部使用该持锁物理连接；连接失效时失败关闭并撤销 relay readiness，不从普通连接池自动重连继续确认。锁查询前即设置关闭时丢弃连接，确保取消或退出不把 session lock 遗留到池内，恢复消费需重新启动并取得 owner。

schema 不可识别、序号缺口或外部 sink 失败都会写入 `realtime_failures` 且不推进游标，失败记录未解决前消费停在该边界。只有替代事件已追加到同一 stream 后，运维流程才能写入引用原事件的 `replay_event_id` 并越过原事件序号；原事件不被修改，替代事件按自己的新序号进入后续消费，保证重放不静默重排历史事件。
正常 acknowledge 与 Admin replay 统一按 failure → consumer offset 取得行锁，避免相反锁序形成等待环；
失败记录处理和游标确认仍在同一事务内，后续身份或连续性校验失败时整笔回滚。

PostgreSQL 提供游标、失败记录和 replay 适配器，Worker 可注入消费 runner。Public 的独立持久化 relay 追赶 `auction:` 目录（含 `auction:discovery`），交给本地有界 Hub 接收后才推进游标，队列满则淘汰慢连接；这只证明本实例已接收，外部广播 sink 仍需另行冻结协议。Admin A21–A24、A43 复用同一持久化边界，lag、failure 列表/详情与事件元数据只返回安全投影，failure 页保留 `data.pagination`。A24 v1 兼容请求仅含 `schema_version` 和公开 payload，HTTP 按 failure 与修正负载稳定派生 `command_id`，新客户端可显式携带并重用该 ID；事务锁定原 failure，追加带 `replay_of_event_id` 的事件、解决 failure、追加 audit 并写 `auction_commands`，同命令返回原事件投影，负载或 failure 不同则冲突。
解决 failure 时，同一事务还须按原 `consumer_key/stream_key` 锁验并推进原事件的连续消费边界；不能仅隐藏 failure、
也不能越过尚未处理的更早事件。替代事件仍按它的新序号独立消费，不伪造外部 sink 已接收。
