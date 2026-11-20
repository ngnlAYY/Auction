// Admin 规则文档与 runtime 配置 DTO。

import * as v from "valibot";
import { integer, jsonValue, optionalNullable, strictStruct } from "../../wire";
import { EquipmentTaxonomyDtoSchema } from "./equipment";

/** 请求里必填的自由 JSON 值；排除 `undefined`，避免必填属性在序列化时被静默丢弃。
 *
 * Rust 对应字段是 `serde_json::Value`：显式 `null` 合法，缺键由服务端拒绝。递归结构同时
 * 覆盖对象与数组内部，防止嵌套属性带着 `undefined` 进入 `JSON.stringify`。 */
export type AdminJsonValue =
  | null
  | boolean
  | number
  | string
  | readonly AdminJsonValue[]
  | { readonly [key: string]: AdminJsonValue };

/** Admin 规则候选校验请求。 */
export interface AdminRulesCandidateRequest {
  locale: string;
  title: string;
  markdown: string;
}

export interface AdminRulesPublishRequest {
  /** 稳定命令 ID（UUID）。 */
  command_id: string;
  expected_revision: number;
  locale: string;
  title: string;
  markdown: string;
}

/** Admin runtime 配置候选校验请求；文档值由应用层反序列化为严格的非敏感 TOML 模型。 */
export interface AdminRuntimeConfigValidateRequest {
  document: AdminJsonValue;
}

/** Admin runtime 配置更新请求；fingerprint 是当前文件快照的版本 fence。 */
export interface AdminRuntimeConfigUpdateRequest {
  /** 稳定命令 ID（UUID）。 */
  command_id: string;
  expected_fingerprint: string;
  document: AdminJsonValue;
}

/** Admin 规则文档安全响应；包含发布者，但不包含数据库内部模型。 */
export const AdminRulesResponseSchema = strictStruct({
  locale: v.string(),
  title: v.string(),
  markdown: v.string(),
  published_by: v.string(),
  revision: integer,
  published_at: v.string(),
});

export type AdminRulesResponse = v.InferOutput<typeof AdminRulesResponseSchema>;

/** Admin 规则候选校验响应。 */
export const AdminRulesCandidateResponseSchema = strictStruct({
  locale: v.string(),
  title: v.string(),
  markdown: v.string(),
});

export type AdminRulesCandidateResponse = v.InferOutput<typeof AdminRulesCandidateResponseSchema>;

/** Worker 冷配置与当前文件的比较结果；仅由显式请求的 GET 返回。 */
export const AdminWorkerRestartStateDtoSchema = v.picklist([
  "restart_required",
  "consistent",
  "unknown",
]);

export type AdminWorkerRestartStateDto = v.InferOutput<typeof AdminWorkerRestartStateDtoSchema>;

/** 普通读取的 Admin runtime 配置安全响应；不出现 `worker_restart_state`。更新成功仍明确要求
 * 重启才能让进程读取新文档。 */
const RUNTIME_CONFIG_RESPONSE_ENTRIES = {
  document: jsonValue,
  fingerprint: v.string(),
  restart_required: v.boolean(),
  /** 仅显式请求词典投影时返回；省略时保持原 v1 响应字段集合。 */
  equipment_taxonomy: optionalNullable(EquipmentTaxonomyDtoSchema),
};

export const AdminRuntimeConfigResponseSchema = strictStruct(RUNTIME_CONFIG_RESPONSE_ENTRIES);

export type AdminRuntimeConfigResponse = v.InferOutput<typeof AdminRuntimeConfigResponseSchema>;

/** 带 Worker 冷配置状态的配置读取；`worker_restart_state` 必须出现，取闭集三态之一。 */
export const AdminRuntimeConfigResponseWithWorkerRestartStateSchema = strictStruct({
  ...RUNTIME_CONFIG_RESPONSE_ENTRIES,
  worker_restart_state: AdminWorkerRestartStateDtoSchema,
});

export type AdminRuntimeConfigResponseWithWorkerRestartState = v.InferOutput<
  typeof AdminRuntimeConfigResponseWithWorkerRestartStateSchema
>;
