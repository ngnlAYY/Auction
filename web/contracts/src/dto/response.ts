// 通用响应、健康探针、实时事件与错误 DTO 的线上 schema。

import * as v from "valibot";
import { int32, integer, jsonValue, optionalNullable, struct } from "../wire";

/** 已发布的统一响应外壳的成功形式；HTTP 状态与 `code` 的组合见路由契约。 */
export function apiResponseSchema<const TSchema extends v.GenericSchema>(dataSchema: TSchema) {
  return struct({
    code: v.string(),
    message: v.string(),
    data: dataSchema,
  });
}

export type ApiResponse<T> = {
  code: string;
  message: string;
  data: T;
};

/** `service` 与 `version` 由装配根传入，避免探针误用本契约 crate 的进程身份与版本。 */
export const HealthResponseSchema = struct({
  service: v.string(),
  status: v.string(),
  version: v.string(),
});

export type HealthResponse = v.InferOutput<typeof HealthResponseSchema>;

/** 与旧版 `/health/live`、`/health/ready` 保持一致的探针响应外壳；字段与成功外壳完全相同。 */
export type HealthProbeResponse<T> = ApiResponse<T>;

/** 进程存活探针数据。 */
export const HealthLiveDataSchema = struct({
  live: v.boolean(),
});

export type HealthLiveData = v.InferOutput<typeof HealthLiveDataSchema>;

/** 进程就绪探针数据。 */
export const HealthReadyDataSchema = struct({
  ready: v.boolean(),
});

export type HealthReadyData = v.InferOutput<typeof HealthReadyDataSchema>;

/** Public 持久化实时事件的安全信封；负载已由应用层事件注册表校验。
 * 不暴露数据库内部状态版本列或 origin 字段。 */
export const PublicRealtimeEventResponseSchema = struct({
  event_id: v.string(),
  stream_sequence: integer,
  kind: v.string(),
  schema_version: int32,
  payload: jsonValue,
  created_at: v.string(),
});

export type PublicRealtimeEventResponse = v.InferOutput<typeof PublicRealtimeEventResponseSchema>;

export const PublicRealtimeResponseSchema = struct({
  auction_id: v.string(),
  events: v.array(PublicRealtimeEventResponseSchema),
  next_after: integer,
  has_more: v.boolean(),
  refetch_required: v.boolean(),
});

export type PublicRealtimeResponse = v.InferOutput<typeof PublicRealtimeResponseSchema>;

/** 统一错误响应；可选数据缺省时仍固定输出 `data: null`。 */
export const ApiErrorResponseSchema = struct({
  code: v.string(),
  message: v.string(),
  data: optionalNullable(jsonValue),
  request_id: optionalNullable(v.string()),
});

export type ApiErrorResponse = v.InferOutput<typeof ApiErrorResponseSchema>;

/** 带稳定业务数据的错误响应，用于封禁原因等旧契约投影。 */
export function apiErrorResponseWithDataSchema<const TSchema extends v.GenericSchema>(
  dataSchema: TSchema,
) {
  return struct({
    code: v.string(),
    message: v.string(),
    data: dataSchema,
    request_id: optionalNullable(v.string()),
  });
}

export type ApiErrorResponseWithData<T> = {
  code: string;
  message: string;
  data: T;
  request_id: string | null;
};
