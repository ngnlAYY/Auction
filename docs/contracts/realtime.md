# 实时事件契约

[文档导航](../README.md) · [HTTP 路由](routes.md) · [数据库](database.md)

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
| `auction:discovery` | 跨世界的“新场次创建”派生流，只含 `auction.created` |

`stream_sequence` 在每个流内从 1 连续递增。

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

负载必须**恰好**包含下表字段（多一个、少一个或为 null 都视为不合法），并通过类型、范围与闭集校验。整数都在 JavaScript 安全整数范围内；UUID 必须是规范小写、非 nil 的 RFC 4122 v1–v5；状态版本、期号与出价等正数字段不能为 0。权威注册表及取值约束见 [`crates/auction-application/src/ports/realtime/public.rs`](../../crates/auction-application/src/ports/realtime/public.rs)。

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

拍卖流中事件的 `auction_id` 必须与流名一致。新增事件种类或负载变化必须新增注册表条目（或提升 `schema_version`），并同步更新本页与浏览器解码器；不符合注册表的历史事件不会被推送。

## HTTP 回放：`GET /api/v1/events`

| 参数 | 说明 |
| --- | --- |
| `auction_id` | 单场流；与 `scope` 二选一 |
| `scope=auctions` | 发现流 |
| `after` | 游标（上次的 `next_after`），规范非负十进制，默认 0 |
| `page_size` | 1–200，默认 100 |

不需要登录。响应 `data`：

```json
{"auction_id": "…", "events": [ … ], "next_after": 57, "has_more": false, "refetch_required": false}
```

发现流以 `"scope": "auctions"` 代替 `auction_id`。

- `next_after` 是下次请求应使用的游标，即使本页有事件被跳过也会推进。
- `has_more` 为 true 时继续翻页。
- `refetch_required` 为 true 表示序列出现缺口、遇到不符合注册表的事件或已关闭世界中的不可见事件——客户端必须丢弃本地推导，重新读取权威快照。
- 拍卖不存在时返回 404。

## WebSocket：`/api/v1/ws`

- 查询参数只能是 `auction_id=<UUID>` 或 `scope=auctions` 之一。
- **必须已登录**（会话 Cookie），并通过 origin allowlist 与连接尝试限流；匿名连接返回 401。已结算或不存在的拍卖返回 404。
- 容量：每进程最多 1024 条连接、每用户 8 条，发现流另限 128 条；超限返回 429（`realtime_busy` / `discovery_busy`）。
- HTTP/1.1 使用 `GET` Upgrade，HTTP/2 使用扩展 `CONNECT`。
- 连接是**只读**的：客户端发送任何文本或二进制消息都会以 1008 关闭；消息与帧上限 4 KiB，超限以 1009 关闭。
- 服务端每 50 秒发送 Ping，60 秒内未收到 Pong 即断开；单次写入超时 5 秒。
- 会话复核：连接期间至少每 30 秒（或在会话到期时）重新核对会话；会话失效以 1008 关闭，会话服务不可用以 1013 关闭。
- 进程关闭时以 1001 关闭；发送队列满（慢消费者）以 1013 关闭，客户端应重连并用 HTTP 回放补齐。

### 客户端建议流程

1. 建立 WebSocket，缓冲到达的帧。
2. 用最后确认的游标调用 `GET /api/v1/events` 补齐（`has_more` 时翻页），再按 `stream_sequence` 合并缓冲帧，以 `event_id` 去重。
3. 发现序列跳号或 `refetch_required` 时重新读取权威快照并重新补齐。
4. 断线后指数退避重连（Public 前端：基数 1 秒起步、翻倍至 30 秒，实际等待取基数的 1/2 至 1 倍）；连接存活 30 秒后重置失败计数，连续 8 次快速失败转为手动刷新。页面隐藏时暂停重连退避与后台轮询，已建立的 WebSocket 不因隐藏而主动关闭。
