// 审计地址与表单的九项筛选状态。
// 草稿只在提交时写入 URL；无效历史值保留原样，直到操作员明确更正。

import type { QueryPair } from "@auction/web-shared/net/query";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";

import {
  type AdminAuditFilter,
  adminAuditFilterAbsoluteTime,
  adminAuditFilterNormalizeText,
} from "../../api/audit";
import { parseStrictRfc3339 } from "../../api/session-time";
import { datetimeLocalValue } from "../../api/validation";
import {
  type FilterValue,
  filterValueAppendPairs,
  filterValueRestore,
  normalizeFilterInput,
  normalizeFilterText,
} from "../realtime/filter-field";

/** 审计筛选字段；顺序与 Rust 的 `AuditField::ALL` 一致。 */
export type AuditField =
  | "actor"
  | "entity"
  | "action"
  | "request_id"
  | "actor_type"
  | "source"
  | "result"
  | "from"
  | "to";

/** 审计表单与地址栏共用的字段顺序。 */
export const AUDIT_FIELDS = [
  "actor",
  "entity",
  "action",
  "request_id",
  "actor_type",
  "source",
  "result",
  "from",
  "to",
] as const satisfies readonly AuditField[];

/** 始终展示的四个主字段；其余字段进入高级筛选。 */
export const AUDIT_PRIMARY_FIELDS = ["actor", "entity", "action", "request_id"] as const;

/** 折叠在高级筛选里的五个字段。 */
export const AUDIT_ADVANCED_FIELDS = [
  "actor_type",
  "source",
  "result",
  "from",
  "to",
] as const satisfies readonly AuditField[];

type AuditFilterValues = readonly [
  FilterValue,
  FilterValue,
  FilterValue,
  FilterValue,
  FilterValue,
  FilterValue,
  FilterValue,
  FilterValue,
  FilterValue,
];

/** 审计地址与表单的筛选状态，只有每个字段都有效时才构造 API 筛选。 */
export interface AuditFilterState {
  readonly values: AuditFilterValues;
}

/** 闭集字段的允许值；自由文本字段返回 `null`。 */
export function auditFieldChoices(field: AuditField): readonly string[] | null {
  switch (field) {
    case "actor_type":
      return ["public_user", "admin", "worker", "system"];
    case "source":
      return ["public", "admin", "cli", "worker", "system"];
    case "result":
      return ["success", "rejected", "failure"];
    case "actor":
    case "entity":
    case "action":
    case "request_id":
    case "from":
    case "to":
      return null;
    default:
      return unreachable(field);
  }
}

/** 字段的地址栏键与控件 ID。 */
export function auditFieldNames(field: AuditField): readonly [string, string] {
  switch (field) {
    case "actor":
      return ["actor", "audit-actor"];
    case "entity":
      return ["entity", "audit-entity"];
    case "action":
      return ["action", "audit-action"];
    case "request_id":
      return ["request_id", "audit-request-id"];
    case "actor_type":
      return ["actor_type", "audit-actor-type"];
    case "source":
      return ["source", "audit-source"];
    case "result":
      return ["result", "audit-result"];
    case "from":
      return ["from", "audit-from"];
    case "to":
      return ["to", "audit-to"];
    default:
      return unreachable(field);
  }
}

/** 字段在冻结顺序中的下标。 */
export function auditFieldIndex(field: AuditField): number {
  switch (field) {
    case "actor":
      return 0;
    case "entity":
      return 1;
    case "action":
      return 2;
    case "request_id":
      return 3;
    case "actor_type":
      return 4;
    case "source":
      return 5;
    case "result":
      return 6;
    case "from":
      return 7;
    case "to":
      return 8;
    default:
      return unreachable(field);
  }
}

/** 按字段规则裁剪并校验；返回 `null` 表示该值不能用于请求。 */
export function auditFieldNormalize(field: AuditField, value: string): string | null {
  switch (field) {
    case "actor":
      return normalizeFilterText(value, 256);
    case "entity":
      return normalizeFilterText(value, 385);
    case "action":
    case "request_id":
      return normalizeFilterText(value, 128);
    case "from":
    case "to": {
      const normalized = normalizeFilterInput(value);
      return normalized === null ? null : adminAuditFilterAbsoluteTime(normalized);
    }
    case "actor_type":
    case "source":
    case "result": {
      const normalized = normalizeFilterInput(value);
      if (normalized === null) {
        return null;
      }
      return normalized === "" || auditFieldChoices(field)?.includes(normalized) === true
        ? normalized
        : null;
    }
    default:
      return unreachable(field);
  }
}

/** 空文本筛选。 */
export function auditTextFilter(value: string): FilterValue {
  return { kind: "text", value };
}

/** 默认的九项空筛选。 */
export function auditFilterStateDefault(): AuditFilterState {
  return {
    values: [
      auditTextFilter(""),
      auditTextFilter(""),
      auditTextFilter(""),
      auditTextFilter(""),
      auditTextFilter(""),
      auditTextFilter(""),
      auditTextFilter(""),
      auditTextFilter(""),
      auditTextFilter(""),
    ],
  };
}

/** 提交后的 API 筛选转换为地址状态；先按 Rust 的 `From` 实现去掉文本首尾空白。 */
export function auditFilterStateFromFilter(filter: AdminAuditFilter): AuditFilterState {
  const normalized: AdminAuditFilter = { ...filter };
  adminAuditFilterNormalizeText(normalized);
  return {
    values: [
      auditTextFilter(normalized.actor),
      auditTextFilter(normalized.entity),
      auditTextFilter(normalized.action),
      auditTextFilter(normalized.requestId),
      auditTextFilter(normalized.actorType),
      auditTextFilter(normalized.source),
      auditTextFilter(normalized.result),
      auditTextFilter(normalized.occurredFrom),
      auditTextFilter(normalized.occurredTo),
    ],
  };
}

/** 从地址栏恢复九项筛选；重复键或非法值保留为不可请求的 `invalid`。 */
export function auditFilterStateRestore(pairs: readonly QueryPair[]): AuditFilterState {
  return {
    values: [
      restoreField(pairs, "actor"),
      restoreField(pairs, "entity"),
      restoreField(pairs, "action"),
      restoreField(pairs, "request_id"),
      restoreField(pairs, "actor_type"),
      restoreField(pairs, "source"),
      restoreField(pairs, "result"),
      restoreField(pairs, "from"),
      restoreField(pairs, "to"),
    ],
  };
}

/** 写回地址栏时按九字段顺序追加；非法原值一个都不丢弃。 */
export function auditFilterStateAppendPairs(
  state: AuditFilterState,
  pairs: [string, string][],
): void {
  for (const [index, field] of AUDIT_FIELDS.entries()) {
    const value = state.values[index];
    if (value !== undefined) {
      filterValueAppendPairs(value, pairs, auditFieldNames(field)[0]);
    }
  }
}

/** 只有九项全部有效时才构造 API 筛选。 */
export function auditFilterStatePrepare(
  state: AuditFilterState,
): Result<AdminAuditFilter, AuditField> {
  const actor = prepareField(state, "actor");
  if (!actor.ok) {
    return actor;
  }
  const entity = prepareField(state, "entity");
  if (!entity.ok) {
    return entity;
  }
  const action = prepareField(state, "action");
  if (!action.ok) {
    return action;
  }
  const requestId = prepareField(state, "request_id");
  if (!requestId.ok) {
    return requestId;
  }
  const actorType = prepareField(state, "actor_type");
  if (!actorType.ok) {
    return actorType;
  }
  const source = prepareField(state, "source");
  if (!source.ok) {
    return source;
  }
  const result = prepareField(state, "result");
  if (!result.ok) {
    return result;
  }
  const occurredFrom = prepareField(state, "from");
  if (!occurredFrom.ok) {
    return occurredFrom;
  }
  const occurredTo = prepareField(state, "to");
  if (!occurredTo.ok) {
    return occurredTo;
  }
  return ok({
    actor: actor.value,
    entity: entity.value,
    action: action.value,
    requestId: requestId.value,
    actorType: actorType.value,
    source: source.value,
    result: result.value,
    occurredFrom: occurredFrom.value,
    occurredTo: occurredTo.value,
  });
}

/** 读取请求使用的 API 筛选；任一字段无效时返回 `null`。 */
export function auditFilterStateRequest(state: AuditFilterState): AdminAuditFilter | null {
  const prepared = auditFilterStatePrepare(state);
  return prepared.ok ? prepared.value : null;
}

/** 替换单个字段，其余字段保持原引用。 */
export function auditFilterStateWithValue(
  state: AuditFilterState,
  field: AuditField,
  value: FilterValue,
): AuditFilterState {
  return {
    values: [
      field === "actor" ? value : state.values[0],
      field === "entity" ? value : state.values[1],
      field === "action" ? value : state.values[2],
      field === "request_id" ? value : state.values[3],
      field === "actor_type" ? value : state.values[4],
      field === "source" ? value : state.values[5],
      field === "result" ? value : state.values[6],
      field === "from" ? value : state.values[7],
      field === "to" ? value : state.values[8],
    ],
  };
}

/** 与 Rust 的 `PartialEq` 对应；非法字段比较每个原始值。 */
export function auditFilterStateEquals(left: AuditFilterState, right: AuditFilterState): boolean {
  return left.values.every((value, index) => filterValueEquals(value, right.values[index]));
}

/** `Text` 字段的文本；`Invalid` 不能派生出任何请求或已应用条件。 */
export function auditFilterValueText(value: FilterValue): string | null {
  return value.kind === "text" ? value.value : null;
}

/** 保留单个原值供编辑。 */
export function auditFilterValueInput(value: FilterValue): string {
  return value.kind === "text" ? value.value : (value.values[0] ?? "");
}

/** 获得错误区域要显示的原值；提交校验失败的草稿也走同一路径。 */
export function auditFilterValueErrorValues(value: FilterValue, invalidDraft: boolean): string[] {
  if (value.kind === "invalid") {
    return [...value.values];
  }
  return invalidDraft ? [value.value] : [];
}

/** 本地文本只作为输入框显示值；未编辑的时间边界仍按链接里的精确时刻提交。 */
export function auditTimeInput(value: string): string {
  if (parseStrictRfc3339(value) !== null) {
    return datetimeLocalValue(value) ?? "";
  }
  return value;
}

function restoreField(pairs: readonly QueryPair[], field: AuditField): FilterValue {
  const key = auditFieldNames(field)[0];
  return filterValueRestore(pairs, key, (value) => auditFieldNormalize(field, value));
}

function prepareField(state: AuditFilterState, field: AuditField): Result<string, AuditField> {
  const value = state.values[auditFieldIndex(field)];
  if (value === undefined) {
    return err(field);
  }
  const text = auditFilterValueText(value);
  if (text === null) {
    return err(field);
  }
  const normalized = auditFieldNormalize(field, text);
  return normalized === null ? err(field) : ok(normalized);
}

function filterValueEquals(left: FilterValue, right: FilterValue | undefined): boolean {
  if (right === undefined || left.kind !== right.kind) {
    return false;
  }
  if (left.kind === "text" && right.kind === "text") {
    return left.value === right.value;
  }
  if (left.kind === "invalid" && right.kind === "invalid") {
    return (
      left.values.length === right.values.length &&
      left.values.every((value, index) => value === right.values[index])
    );
  }
  return false;
}
