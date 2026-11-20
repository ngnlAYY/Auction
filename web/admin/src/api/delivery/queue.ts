// 投递队列的检索与人工命令。
// 列表只提交显式筛选与分页；释放/重试/reconcile 都携带按输入复用的 `command_id` 与版本
// fence，浏览器不推导状态机，幂等与审计归服务端所有。

import {
  ADMIN_DELIVERY_DECISION_FIELD_POLICY,
  ADMIN_DELIVERY_LIST_FIELD_POLICY,
  type AdminDeliveryCommandRequest,
  AdminDeliveryCommandResponseSchema,
  AdminDeliveryListResponseSchema,
} from "@auction/contracts/dto/admin/delivery";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import {
  type AdminWriteFailure,
  type AdminWriteNotSent,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
  writeOutcomeUnknown,
} from "../failure";
import { validate as validatePagination } from "../pagination";
import type { AdminSessionHost } from "../session";
import { trimRustWhitespace } from "../session-time";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendPreparedJsonWrite,
} from "../transport";
import {
  type Delivery,
  type DeliveryPage,
  decodeDeliveryCommand,
  decodeDeliveryList,
  parseDeliveryTime,
  strictDeliveryEnvelopeSchema,
} from "./decode";
import { DELIVERY_PROJECTION_INCLUDE } from "./index";

/** 五个投递动作；线上路径与 reconcile 的子动作由 `deliveryActionPath` 与
 * `deliveryActionRequestAction` 决定。 */
export type DeliveryAction =
  | "release"
  | "retry"
  | "reconcile_retry"
  | "reconcile_succeeded"
  | "reconcile_failed";

/** 动作的 URL 路径段；三种 reconcile 共用一个端点。 */
export function deliveryActionPath(action: DeliveryAction): "release" | "retry" | "reconcile" {
  switch (action) {
    case "release":
      return "release";
    case "retry":
      return "retry";
    case "reconcile_retry":
    case "reconcile_succeeded":
    case "reconcile_failed":
      return "reconcile";
    default:
      return unreachable(action);
  }
}

/** reconcile 请求正文里的子动作；释放与普通重试省略该键。 */
export function deliveryActionRequestAction(
  action: DeliveryAction,
): "retry" | "succeeded" | "failed" | null {
  switch (action) {
    case "release":
    case "retry":
      return null;
    case "reconcile_retry":
      return "retry";
    case "reconcile_succeeded":
      return "succeeded";
    case "reconcile_failed":
      return "failed";
    default:
      return unreachable(action);
  }
}

/** 一次投递处置命令的冻结输入；发送 URL、正文与回执核对都只读这一份值。 */
export interface DeliveryCommandSnapshot {
  readonly deliveryId: string;
  readonly action: DeliveryAction;
  readonly expectedFencingVersion: number;
  readonly externalReference: string | null;
  readonly failureCode: string | null;
}

/** 已冻结输入的命令准备函数；同一个 `command_id` 的重试返回同一份正文。 */
export type DeliveryCommandPreparation = ((
  commandId: string,
) => Result<PreparedJsonCommand, AdminWriteNotSent>) & {
  readonly snapshot: DeliveryCommandSnapshot;
};

/** 在提交时复制并冻结输入；执行体第一次调用返回的函数时才构造正文，后续重试复用同一文本。 */
export function prepareDeliveryCommandOnce(
  snapshot: DeliveryCommandSnapshot,
): DeliveryCommandPreparation {
  const frozen: DeliveryCommandSnapshot = Object.freeze({
    deliveryId: snapshot.deliveryId,
    action: snapshot.action,
    expectedFencingVersion: snapshot.expectedFencingVersion,
    externalReference: snapshot.externalReference,
    failureCode: snapshot.failureCode,
  });
  const prepare = prepareJsonCommandOnce((stableCommandId) => {
    const requestAction = deliveryActionRequestAction(frozen.action);
    const request: AdminDeliveryCommandRequest = {
      command_id: stableCommandId,
      expected_fencing_version: frozen.expectedFencingVersion,
      ...(requestAction === null ? {} : { action: requestAction }),
      ...(frozen.externalReference === null
        ? {}
        : { external_reference: frozen.externalReference }),
      ...(frozen.failureCode === null ? {} : { failure_code: frozen.failureCode }),
    };
    return request;
  });
  return Object.freeze(
    Object.assign((commandId: string) => prepare(commandId), { snapshot: frozen }),
  );
}

/** 投递检索条件；空串表示该维度未设置，由页面从地址栏恢复。 */
export interface DeliverySearchFilters {
  readonly recipient: string;
  readonly reference: string;
  readonly template: string;
  readonly category: string;
  readonly world: string;
  readonly createdFrom: string;
  readonly createdTo: string;
}

/** 与 Rust `Default` 相同的空筛选。 */
export function deliverySearchFiltersDefault(): DeliverySearchFilters {
  return {
    recipient: "",
    reference: "",
    template: "",
    category: "",
    world: "",
    createdFrom: "",
    createdTo: "",
  };
}

const DELIVERY_CATEGORIES: readonly string[] = [
  "outbid",
  "account",
  "intake",
  "settlement",
  "equipment",
  "operations",
  "manual",
];

const DELIVERY_WORLDS: readonly string[] = ["main", "isekai"];

/** 客户端只做格式与上限检查；服务端仍按闭集和精确匹配失败关闭。 */
function deliverySearchFiltersValid(filters: DeliverySearchFilters): boolean {
  return (
    validFilterText(filters.recipient, 256) &&
    (filters.reference === "" || validUuid(filters.reference)) &&
    validFilterText(filters.template, 64) &&
    (filters.category === "" || DELIVERY_CATEGORIES.includes(filters.category)) &&
    (filters.world === "" || DELIVERY_WORLDS.includes(filters.world)) &&
    validFilterTime(filters.createdFrom) &&
    validFilterTime(filters.createdTo)
  );
}

function validFilterText(value: string, maximum: number): boolean {
  if (value === "") {
    return true;
  }
  return (
    utf8ByteLength(value) <= maximum && value === trimRustWhitespace(value) && !hasControl(value)
  );
}

function hasControl(value: string): boolean {
  for (const character of value) {
    const codePoint = character.codePointAt(0) ?? 0;
    if (codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f)) {
      return true;
    }
  }
  return false;
}

function validFilterTime(value: string): boolean {
  return value === "" || parseDeliveryTime(value) !== null;
}

/** 列表的唯一 URL 构造点：只提交显式筛选与分页，并总是请求扩展投影，本页面的队列才会列出
 * 结算退件 Take 投递（缺省投影不含它们）。 */
export function deliveryListUrl(
  status: string,
  filters: DeliverySearchFilters,
  page: number,
  pageSize: number,
): string {
  const query = new URLSearchParams();
  query.append("page", Math.max(page, 1).toString());
  query.append("page_size", pageSize.toString());
  if (status !== "") {
    query.append("status", trimRustWhitespace(status));
  }
  const selected: readonly (readonly [string, string])[] = [
    ["recipient", filters.recipient],
    ["reference", filters.reference],
    ["template", filters.template],
    ["category", filters.category],
    ["world", filters.world],
    ["created_from", filters.createdFrom],
    ["created_to", filters.createdTo],
  ];
  for (const [key, value] of selected) {
    if (value !== "") {
      query.append(key, value);
    }
  }
  query.append("include", DELIVERY_PROJECTION_INCLUDE);
  return `/api/v1/admin/deliveries?${query.toString()}`;
}

/** 读取指定状态、筛选与页码的 delivery 安全投影。 */
export async function fetchDeliveries(
  host: AdminSessionHost,
  status: string,
  filters: DeliverySearchFilters,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<DeliveryPage, string>> {
  const bounds = validatePagination(page, pageSize, 100, 9_223_372_036_854_775_807n);
  if (!bounds.ok) {
    return err(bounds.error);
  }
  if (!deliverySearchFiltersValid(filters)) {
    return err("invalid_delivery_filter");
  }
  const url = deliveryListUrl(status, filters, page, pageSize);
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(response.error);
  }
  if (!responseOk(response.value.status)) {
    return err(await responseError(response.value, signal, host));
  }
  const payload = decodeAdminReceipt(
    response.value,
    strictDeliveryEnvelopeSchema(AdminDeliveryListResponseSchema),
    ADMIN_DELIVERY_LIST_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const decoded = decodeDeliveryList(payload.value, page, pageSize);
  if (!decoded.ok) {
    return err(invalidResponseError("delivery_list"));
  }
  return ok(decoded.value);
}

/** 提交一次投递处置；目标 URL、动作与回执核对都取自准备函数持有的冻结输入。 */
export async function executeDelivery(
  host: AdminSessionHost,
  preparation: DeliveryCommandPreparation,
  commandId: string,
  signal: AbortSignal,
): Promise<Result<Delivery, AdminWriteFailure>> {
  const prepared = preparation(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const { deliveryId, action, expectedFencingVersion } = preparation.snapshot;
  const url = `/api/v1/admin/deliveries/${deliveryId}/${deliveryActionPath(action)}`;
  const response = await sendPreparedJsonWrite(
    host,
    { url, method: "POST", endpoint: "general" },
    prepared.value,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    strictDeliveryEnvelopeSchema(AdminDeliveryCommandResponseSchema),
    ADMIN_DELIVERY_DECISION_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const decoded = decodeDeliveryCommand(payload.value, deliveryId, action, expectedFencingVersion);
  if (!decoded.ok) {
    return err(writeOutcomeUnknown(invalidResponseError("delivery_result")));
  }
  return ok(decoded.value);
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(status: number): boolean {
  return status >= 200 && status <= 299;
}
