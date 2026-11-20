// 运行时开关 DTO；持久记录在 HTTP 边界显式映射。Admin 只能写外部访问暂停。

import * as v from "valibot";
import { float, integer, jsonValue, optionalNullable, strictStruct } from "../../wire";
import { timestamp } from "../../wire/timestamp";

/** 数据库最新意图，clear 与 expire 也保留版本及操作者。 */
export const RuntimeSwitchIntentDtoSchema = strictStruct({
  /** 闭集开关名。 */
  name: v.string(),
  /** 最新数据库版本；未设置为零。 */
  revision: integer,
  /** 最新动作；未设置为空。 */
  operation: optionalNullable(v.string()),
  /** 最新 set 的值。 */
  value: optionalNullable(jsonValue),
  expires_at: optionalNullable(timestamp),
  /** 操作者声明。 */
  actor_id: optionalNullable(v.string()),
  /** 实际入口。 */
  entrypoint: optionalNullable(v.string()),
  reason: optionalNullable(v.string()),
});

export type RuntimeSwitchIntentDto = v.InferOutput<typeof RuntimeSwitchIntentDtoSchema>;

/** Admin 当前实际使用的值，与数据库意图分开显示。 */
export const RuntimeSwitchAppliedDtoSchema = strictStruct({
  name: v.string(),
  /** 实际观察版本。 */
  revision: integer,
  /** TOML 启动基线。 */
  toml_baseline: jsonValue,
  effective_value: jsonValue,
  /** 是否使用数据库覆盖。 */
  overridden: v.boolean(),
});

export type RuntimeSwitchAppliedDto = v.InferOutput<typeof RuntimeSwitchAppliedDtoSchema>;

/** 本接口只证明 Admin 自身应用情况，不冒充其他角色确认。 */
export const RuntimeSwitchListResponseSchema = strictStruct({
  /** 本次意图查询的数据库时间。 */
  database_time: timestamp,
  /** 数据库最新事实投影。 */
  database_intent: v.array(RuntimeSwitchIntentDtoSchema),
  /** Admin 最后成功快照年龄；空值为从未读取成功。 */
  admin_last_success_age_seconds: optionalNullable(float),
  /** Admin 实际值。 */
  admin_applied: v.array(RuntimeSwitchAppliedDtoSchema),
});

export type RuntimeSwitchListResponse = v.InferOutput<typeof RuntimeSwitchListResponseSchema>;

/** Admin 暂停或恢复全部外部访问的稳定命令。
 *
 * `expires_at` 是调用方首次提交时确定的绝对期限，结果未知时必须连同 `command_id`、
 * `expected_revision` 与原因原样重试，不得按重试时刻重新计算。 */
export interface AdminExternalAccessPauseRequest {
  /** 稳定命令 ID（UUID）。 */
  command_id: string;
  /** 读取到的最新版本；未设置为零。 */
  expected_revision: number;
  /** `true` 为暂停，`false` 为恢复（clear）。 */
  paused: boolean;
  /** 暂停的绝对 UTC 期限；恢复时必须为空。 */
  expires_at: string | null;
  /** 单行非敏感原因。 */
  reason: string;
}

/** 数据库已接受的暂停事实；各角色约 5 秒内刷新生效。 */
export const AdminExternalAccessPauseResponseSchema = strictStruct({
  /** 已接受（或原样重放）的最新事实。 */
  accepted: RuntimeSwitchIntentDtoSchema,
});

export type AdminExternalAccessPauseResponse = v.InferOutput<
  typeof AdminExternalAccessPauseResponseSchema
>;
