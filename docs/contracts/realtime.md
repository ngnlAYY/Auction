# 实时事件契约

[docs/README.md](../README.md) · [docs/contracts/routes.md](routes.md) · [docs/contracts/database.md](database.md)

公开站点的实时更新建立在**持久化事件流**之上：业务事务在提交状态变化的同时向 `realtime_events` 追加事件；Public 进程的 relay 轮询事件表并推送给本进程的 WebSocket 连接；浏览器断线后用 HTTP 回放补齐。事件只是“某处发生了变化”的通知，页面收到后重读权威接口，不根据事件负载推导竞价结果。

## 数据流

```text
业务事务（竞价 / 生命周期 / 结算 …）
   └─ 同一事务 INSERT realtime_events（stream_key, stream_sequence 连续递增）
         │
Public relay（每 poll_interval_millis，默认 250ms）
   ├─ 按 consumer 游标列出有新事件的 auction:* 流（分页，每页 stream_batch_size）
   ├─ 每流读取至多 event_batch_size 条 → 注册表校验 → 编码为 WebSocket 帧 → Hub 广播
   └─ 推进持久化游标；失败写入 failure 记录，不推进游标，下轮重试
         │
浏览器：WebSocket 接收单事件帧；GET /api/v1/events 回放补齐
```

- 每个 Public 实例用自己的 `realtime.consumer_key` 维护游标，并以 PostgreSQL session advisory lock 独占该 key；目录扫描与游标写入使用持锁连接，连接断开即失去所有权。
- relay 的投递是 at-least-once：游标提交失败时同一事件可能再次推送，客户端以 `event_id` 去重。
- Admin 的“实时事件”页可查看事件元数据、各 consumer 的滞后与失败记录，并对失败事件执行重放（追加一条新事件并解决原失败，带幂等命令）。

## 流

| stream_key | 内容 |
| --- | --- |
| `auction:<auction_id>` | 单场拍卖的全部公开事件 |

`stream_sequence` 在每个流内从 1 连续递增。

寄售市场的状态变化写入 `consignment_events` 供内部记录。该事件没有公开订阅或 HTTP 回放，也没有新增 WebSocket 或 SSE 入口。页面重读权威状态所用的接口见[寄售市场](routes.md#寄售市场)。

### 寄售订单阶段事件

寄售订单（成交与撤回合同）的阶段变化写入 `realtime_events`：流 `consignment_order:<contract_id>`，kind `consignment.order_stage_changed`，版本 1，序号在业务事务内立即取号。事件与它所描述的事实同事务提交、同事务回滚：购买事务（成交、义务图、首轮、挂单转 `sale_pending` 与事件）与 COD 实收认证（`certify_cod`，付款满足、资金分账、funded 投影与事件）各追加一条；其余资金与履约写口接入时调用同一个追加入口，不另造负载。

- 负载恰好六个字段：`contract_id`、`order_state`、`delivery_stage`、`payment_stage`、`payout_stage`、`refund_stage`，取值是[订单阶段闭集](routes.md#寄售市场)的线协议码；不含金额、预约码、装备 key、对方账号与原信正文。
- 订单态与阶段由领域 `project_order_stages` 在写事务内读取最新事实投影，与 `GET /api/v2/me/consignment/orders` 读到的同源；矛盾或未知的事实一律投影为 `reconcile`。
- 订单版本 `version` 取该流的头部序号（至少为 1）；事件的 `state_version` 只作排序提示。
- 这个流没有公开订阅、回放或推送：Public relay 只扫描 `auction:` 前缀的流，注册表（`PublicRealtimeEvent::validate`）也不接受它；它是内部持久通知，页面仍按上文“事件只是变化通知”的约定重读权威接口。

迁移 `000011` 建立的 `auction:discovery` 历史发现流只保留、不再写入、没有读者：relay 目录不列出它，公开回放与 WebSocket 不接受它，事件注册表把它当作非法流名拒绝；Admin 仪表盘的全量积压与“实时事件”页的滞后列表（计数与明细）也不计入它的游标，升级前旧版本留下的落后游标不会让实时分区永久降级。该流上的未解决失败无法经 Admin 重放收口（注册表拒收），升级前应确认为 0。其历史事件随下文的 30 天保留期清理，流行与各 consumer 游标按保留规则永久保留。

竞价事件（`lot.bid_accepted`）与出价事实同事务写入，但序号在提交后分配。出价事务因此
不再更新共享的 `realtime_streams` 计数行，同一场次不同 lot 的重叠出价不会再因这次读写反依赖
而序列化中止。分配器按 `(created_at, state_version, origin_fact_index, event_id)` 顺序补号
（同一命令的多条竞价事实共享 `created_at`，必须按 `state_version`/`origin_fact_index` 保留接受
顺序，不能由随机 `event_id` 决定），序号仍然从 1 连续、没有空号；
尚未分配序号的事件对 WebSocket 推送与 HTTP 回放都不可见（`stream_sequence > after` 对 NULL
不成立），补齐只推迟推送，不改变任何命令结果。Public relay 每轮先补齐待定序号再扫描目录，
覆盖“提交后进程崩溃”等窗口；未分配积压超过 5 分钟由一致性巡检报
`realtime_unsequenced_backlog`。

## 事件信封

WebSocket 每帧是一个 JSON 对象；HTTP 回放的 `events` 数组元素形状相同：

```json
{
  "event_id": "1f0c…",
  "stream_sequence": 42,
  "kind": "lot.bid_accepted",
  "schema_version": 2,
  "payload": { … },
  "created_at": "2026-10-30T12:00:00Z"
}
```

## 事件注册表

负载必须**恰好**包含下表字段（多一个、少一个或为 null 都视为不合法），并通过类型、范围与闭集校验。整数都在 JavaScript 安全整数范围内；UUID 必须是规范小写、非 nil 的 RFC 4122 v1–v5；状态版本、期号与出价等正数字段不能为 0。权威注册表及取值约束见 [crates/auction-application/src/ports/realtime/public.rs](../../crates/auction-application/src/ports/realtime/public.rs)。

| kind | 版本 | 负载字段 |
| --- | --- | --- |
| `auction.created` | 1 | `auction_id`、`world_id`、`sequence`、`state_version` |
| `auction.preview_updated` | 1 | `auction_id`、`preview_revision`、`item_count`、`manual_open`、`state_version` |
| `preview_catalog_changed` | 1 | `world`、`auction_id`、`preview_revision` |
| `auction.opened` | 1 | `auction_id`、`world_id`、`state_version`、`preview_revision` |
| `auction.schedule_updated` | 1 | `auction_id`、`world_id`、`scheduled_end_at`、`state_version` |
| `auction.final_phase_entered` | 1 | `auction_id`、`state_version` |
| `auction.early_settlement_requested` | 1 | `auction_id`、`state_version` |
| `auction.cancelled` | 1 | `auction_id`、`state_version` |
| `auction.settling` | 1 | `auction_id`、`state_version` |
| `auction.settled` | 1 | `auction_id`、`sold_count`、`unsold_count`、`state_version` |
| `auction.settled` | 2 | `auction_id`、`state_version`、`policy_version`、`sold_count`、`unsold_count`、`obligation_count` |
| `lot.bid_accepted` | 2 | `auction_id`、`lot_id`、`bid_id`、`amount`、`effective_end`、`state_version`、`fact_index`、`mechanism`（`ordinary`/`proxy`） |

每条公开事件都属于单拍卖流，负载的 `auction_id` 必须与流名一致。新增事件种类或负载变化必须新增注册表条目（或提升 `schema_version`），并同步更新本页与浏览器解码器；不符合注册表的历史事件不会被推送。

## HTTP 回放：`GET /api/v1/events`

| 参数 | 说明 |
| --- | --- |
| `auction_id` | 单场流，必填 |
| `after` | 游标（上次的 `next_after`），规范非负十进制，默认 0 |
| `page_size` | 1–200，默认 100 |

其他查询参数（包括 `scope`）一律返回 400 `invalid_query`。不需要登录。响应 `data`：

```json
{"auction_id": "…", "events": [ … ], "next_after": 57, "has_more": false, "refetch_required": false}
```

- `next_after` 是下次请求应使用的游标，即使本页有事件被跳过也会推进。
- `refetch_required=false` 时，服务端已在投影前校验本页的原始序号连续。HTTP 回放隐藏 `auction.created` 并保留游标，因此返回的事件序号可以跳号。客户端从请求的 `after` 起检查事件序号严格递增且不超过 `next_after`，合并后将游标推进到 `next_after`。
- `has_more` 为 true 时继续翻页。
- `refetch_required` 为 true 表示序列出现缺口或遇到不符合注册表的事件——客户端必须丢弃本地推导，重新读取权威快照。
- 拍卖不存在时返回 404。

## 回放保留与过期游标

- 事件保留 30 天：`realtime_events` 中早于 30 天的事件是清理候选，删除须先经只读演练与用户批准，不在请求路径内发生。`realtime_streams` 的序号与 `realtime_consumer_offsets` 永久保留，不随事件删除重排。
- 一致性巡检（`auction-cli db inspect-consistency`）的 `realtime_sequence_gap_or_tail_mismatch` 因此只要求保留窗口内的后缀连续：窗口取保留期减 1 天余量（29 天，创建晚于“巡检时刻 − 29 天”的事件不可能已被清理），从窗口内最小序号到最大序号之间不得缺号（边界附近序号更大的旧事件一并计入），窗口内有事件时 `next_sequence` 必须等于最大序号 + 1；任何时候 `next_sequence` 都不得小于等于已有最大序号。窗口外的缺号、事件全部清理后的空流以及仅剩旧事件时的尾部超前都是合法清理结果，不报告；未分配序号积压与消费游标超前照常检查。
- 浏览器回补窗口是 24 小时；断线超过该窗口、或请求游标早于仍保留的最早事件时，服务端没有可补事件。若游标与当前页之间出现序号缺口，响应保持 `refetch_required=true`（同一判定也覆盖损坏投影），客户端必须丢弃本地推导并重新读取权威接口。
- 游标早于全部保留事件且不存在缺口时（清理后完全没有剩余事件），`events` 为空、`has_more=false`、`next_after` 保持请求游标。浏览器不得把“无事件”当作增量完整：本地状态早于 24 小时回补窗口或跨过页面重载时，一律先读权威快照。
- 被 `realtime_failures` 或重放关系引用的候选事件受 `ON DELETE RESTRICT` 约束，在失败记录处置或重放引用清理前不会删除；`db retention-dry-run` 将其计入 `blocked_rows`，不视为可删。经批准的 `db retention-purge` 先清理已解决的失败、再清理事件，因此同轮解除引用的事件也可在批准上限内删除；未解决失败或重放引用仍然阻断。

## WebSocket：`/api/v1/ws`

- 查询参数只能是 `auction_id=<UUID>`；其他参数（包括 `scope`）返回 400 `invalid_query`。
- **必须已登录**（会话 Cookie），并通过 origin allowlist 与连接尝试限流；匿名连接返回 401。已结算或不存在的拍卖返回 404。
- 容量：每进程最多 1024 条连接、每用户 8 条；超限返回 429（`realtime_busy`）。
- HTTP/1.1 使用 `GET` Upgrade，HTTP/2 使用扩展 `CONNECT`。
- 连接是**只读**的：客户端发送任何文本或二进制消息都会以 1008 关闭；消息与帧上限 4 KiB，超限以 1009 关闭。
- 服务端每 50 秒发送 Ping，60 秒内未收到 Pong 即断开；单次写入超时 5 秒。
- 会话复核：连接期间至少每 30 秒（或在会话到期时）重新核对会话；会话失效以 1008 关闭，会话服务不可用以 1013 关闭。
- 建立连接并通过会话复核后，服务端的第一帧是构建号控制帧 `{"type":"hello","build":"<构建号>"}`；它不是事件，`stream_sequence` 与事件去重不受影响。浏览器据此在部署后提示刷新，不自动重载页面。旧版浏览器会把该帧按未知事件计数一次并继续接收后续事件，不会因此断开（除非已经连续失败）。构建号取值与 `x-auction-build` 响应头相同。只有连接 URL 带能力声明 `independent_build=1`（Solid 页面带，旧壳不带；其他取值以 400 拒绝）、配置了静态挂载且入口指针 build 与独立入口 build 不同（回切期）时，帧才另带可选字段 `independent_build`，取值与 `x-auction-build-independent` 响应头相同，即独立入口（新版 Solid 页面）的构建号；其余情况不带该字段，帧字节与此前一致。Solid 页面优先用 `independent_build` 比对，缺省回退 `build`。
- 进程关闭时以 1001 关闭；发送队列满（慢消费者）以 1013 关闭，客户端应重连并用 HTTP 回放补齐。

### 客户端建议流程

1. 建立 WebSocket，缓冲到达的帧。
2. 用最后确认的游标调用 `GET /api/v1/events` 补齐（`has_more` 时翻页），再按 `stream_sequence` 合并缓冲帧，以 `event_id` 去重。
3. WebSocket 单事件帧跳过已确认游标的下一序号时，先用 HTTP 回放补齐。收到 `refetch_required=true` 时重新读取权威快照。
4. 断线后指数退避重连（Public 前端：基数 1 秒起步、翻倍至 30 秒，实际等待取基数的 1/2 至 1 倍）；连接存活 30 秒后重置失败计数，连续 8 次快速失败转为手动刷新。页面隐藏时暂停重连退避与后台轮询，已建立的 WebSocket 不因隐藏而主动关闭。


## 推送观测边界

推送观测只在进程内帧中附带事件 created_at，
送给浏览器的原 JSON 编码字节、信封、schema_version 与 HTTP 补读不变。
Hub 缓冲、淘汰、连接配额、游标及 at-least-once 重投规则保持。

`auction_realtime_relay_duration_seconds{scope}` 从持久事件创建时间
到本地 sink 交给 Hub；无订阅者也可成功，不证明客户端收到。
`auction_realtime_push_duration_seconds{scope}` 从同一起点
到一个连接的 WS send 成功完成，包含轮询、排队和写出等待。
只观察事件文本帧，不把 Ping、失败写出或 HTTP 补读混入；
重投或不同连接各自形成成功写出样本。

scope 固定为 auction（公开实时只有单拍卖流），不导出事件或 stream 标识。
这两个终点均为服务端代理，不证明浏览器收到或处理完成；
created_at 也不是精确 COMMIT 时间。负时差跳过并计闭集 skipped 指标。
详细口径和局限见[crates/auction-observability/README.md#sli-与容量基线](../../crates/auction-observability/README.md#sli-与容量基线)。
