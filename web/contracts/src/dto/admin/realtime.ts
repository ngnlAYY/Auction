// Admin realtime DTO。

import * as v from "valibot";
import { int32, integer, optionalNullable, struct } from "../../wire";
import type { AdminPaginationResponse } from "./pagination";
import { AdminPaginationResponseSchema } from "./pagination";
import type { AdminJsonValue } from "./rules";

/** Admin realtime 事件的安全元数据投影；不包含 payload 与内部来源字段。 */
export const AdminRealtimeEventResponseSchema = struct({
  event_id: v.string(),
  stream_key: v.string(),
  stream_sequence: integer,
  kind: v.string(),
  schema_version: int32,
  created_at: v.string(),
});

export type AdminRealtimeEventResponse = v.InferOutput<typeof AdminRealtimeEventResponseSchema>;

export const AdminRealtimeEventListResponseSchema = struct({
  events: v.array(AdminRealtimeEventResponseSchema),
  pagination: AdminPaginationResponseSchema,
});

export type AdminRealtimeEventListResponse = v.InferOutput<
  typeof AdminRealtimeEventListResponseSchema
>;

/** Admin realtime lag 的安全投影。 */
export const AdminRealtimeLagResponseSchema = struct({
  stream_key: v.string(),
  consumer_key: v.string(),
  offset: integer,
  latest_sequence: integer,
  lag: integer,
});

export type AdminRealtimeLagResponse = v.InferOutput<typeof AdminRealtimeLagResponseSchema>;

export const AdminRealtimeLagListResponseSchema = struct({
  lags: v.array(AdminRealtimeLagResponseSchema),
  pagination: AdminPaginationResponseSchema,
});

export type AdminRealtimeLagListResponse = v.InferOutput<typeof AdminRealtimeLagListResponseSchema>;

/** Admin realtime lag 的旧版页码信封。 */
export type AdminRealtimePaginationResponse = AdminPaginationResponse;

/** Admin realtime failure 的安全投影；不包含原始驱动错误或未过滤 payload。 */
export const AdminRealtimeFailureResponseSchema = struct({
  id: v.string(),
  event_id: v.string(),
  stream_key: v.string(),
  consumer_key: v.string(),
  code: v.string(),
  safe_detail: v.string(),
  failed_at: v.string(),
  resolved_at: optionalNullable(v.string()),
  resolved: v.boolean(),
});

export type AdminRealtimeFailureResponse = v.InferOutput<typeof AdminRealtimeFailureResponseSchema>;

/** Admin realtime failure 的旧版页码信封。 */
export const AdminRealtimeFailurePaginationResponseSchema = struct({
  page: integer,
  page_size: integer,
  has_more: v.boolean(),
  total: integer,
});

export type AdminRealtimeFailurePaginationResponse = v.InferOutput<
  typeof AdminRealtimeFailurePaginationResponseSchema
>;

export const AdminRealtimeFailureListResponseSchema = struct({
  failures: v.array(AdminRealtimeFailureResponseSchema),
  pagination: AdminRealtimeFailurePaginationResponseSchema,
});

export type AdminRealtimeFailureListResponse = v.InferOutput<
  typeof AdminRealtimeFailureListResponseSchema
>;

/** 新客户端应在重试中复用 `command_id`；旧版请求没有该字段，HTTP 防腐层会按 failure 与
 * 修正负载稳定派生。payload 只允许应用事件注册表接受的公开对象。 */
export interface AdminRealtimeReplayRequest {
  /** 稳定命令 ID（UUID）；旧版请求省略该字段。 */
  command_id?: string;
  schema_version: number;
  /** 回传给服务端的自由 JSON 不得被静默改值：页面要么原样提交操作者的文本，要么在数值
   * 无法无损往返时拒绝输入。 */
  public_payload: AdminJsonValue;
}

/** Admin realtime replay 追加事件的安全投影。 */
export const AdminRealtimeReplayEventResponseSchema = struct({
  event_id: v.string(),
  replay_of_event_id: v.string(),
  stream_key: v.string(),
  stream_sequence: integer,
  kind: v.string(),
  schema_version: int32,
  created_at: v.string(),
});

export type AdminRealtimeReplayEventResponse = v.InferOutput<
  typeof AdminRealtimeReplayEventResponseSchema
>;

export const AdminRealtimeReplayResponseSchema = struct({
  replay: AdminRealtimeReplayEventResponseSchema,
});

export type AdminRealtimeReplayResponse = v.InferOutput<typeof AdminRealtimeReplayResponseSchema>;
