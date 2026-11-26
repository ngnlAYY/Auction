// 寄售 v2 公开目录与详情、买家购买、本人订单列表与详情的请求与响应绑定。

import { PUBLIC_EXPECTED_USER_ID_HEADER } from "@auction/contracts/dto/public/commands";
import type { ListingWorld } from "@auction/contracts/dto/public/consignment";
import { ListingSummaryV2PageSchema } from "@auction/contracts/dto/public/consignment/catalog-v2";
import {
  type ListingDetailV2,
  ListingDetailV2Schema,
  type ListingSummaryV2,
} from "@auction/contracts/dto/public/consignment/listing-v2";
import {
  type OrderPageV2,
  OrderPageV2Schema,
  type OrderRoleV2,
  OrderRoleV2Schema,
  type OrderStateV2,
  OrderStateV2Schema,
  type OrderV2,
  OrderV2Schema,
} from "@auction/contracts/dto/public/consignment/order-v2";
import {
  isTrustedPurchaseRejection,
  type PurchaseReceiptV2,
  PurchaseReceiptV2Schema,
  PurchaseRejectionCodeV2Schema,
  type PurchaseRequestV2,
  PurchaseRequestV2Schema,
  purchaseReceiptAnswers,
  purchaseRejectionStatus,
} from "@auction/contracts/dto/public/consignment/purchase-v2";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { ApiErrorResponseSchema, apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { type TransportResponse, transportErrorCode } from "@auction/web-shared/net/transport";
import { type JsonFieldPolicy, jsonHasExactKeys } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import type { GenericSchema } from "valibot";

import { sendRequest } from "./clock";
import { COMMAND_NOT_SENT, type CommandError, commandNotSent } from "./commands";
import {
  type ConsignmentCatalogQuery,
  categoryAccepts,
  consignmentCatalogParams,
} from "./consignment-catalog";
import { type ConsignmentReadError, readPublicConsignment } from "./consignment-status";
import { observePublicSessionFailure } from "./session";

const CATALOG_V2 = "/api/v2/consignment/listings";
const ORDERS = "/api/v2/me/consignment/orders";
const ERROR_KEYS = ["code", "message", "data", "request_id"];
const SUCCESS_KEYS = ["code", "message", "data"];
const ORDER_MAX_OFFSET = 10_000;

const SUMMARY_INTEGERS = [["version"], ["price"], ["generation"], ["level", "value"]] as const;
const ORDER_INTEGERS = [["version"], ["price"], ["fee"], ["seller_amount"]] as const;

/** 会话守卫外壳的拒绝：请求被守卫拦截，不会产生业务结果。 */
const GUARD_REJECTIONS: Readonly<Record<string, number>> = {
  invalid_expected_user_id: 400,
  unauthenticated: 401,
  session_changed: 409,
  rate_limited: 429,
};

/** 购买处理器的固定外壳码；其余状态与码组合都不能证明首次写入已被拒绝。 */
const PURCHASE_SHELL: Readonly<Record<string, number>> = {
  ...GUARD_REJECTIONS,
  // 处理器在调用购买用例前就拒绝（路径、正文、价格解码），不会写账本，是确定的写前拒绝。
  invalid_listing_request: 400,
  invalid_listing_id: 400,
  invalid_command_id: 400,
  invalid_price: 400,
};

const READ_ERRORS = new Set([
  "consignment_disabled",
  "consignment_unavailable",
  "invalid_listing_id",
  "invalid_query",
  "listing_not_found",
  "storage_unavailable",
  "timeout",
  ...Object.keys(GUARD_REJECTIONS),
]);
const ERROR_CODES = new Set([
  ...READ_ERRORS,
  ...Object.keys(PURCHASE_SHELL),
  ...PurchaseRejectionCodeV2Schema.options,
]);

/** 只暴露 HTTP 状态与安全分类；未知协议码保留为 unknown_error。 */
export type ConsignmentOrderError = CommandError;

function pageIntegers(fields: readonly (readonly string[])[]) {
  return [
    ["data", "total"],
    ["data", "page"],
    ["data", "page_size"],
    ...fields.map((path) => ["data", "items", null, ...path]),
  ];
}

function summaryMatches(query: ConsignmentCatalogQuery, listing: ListingSummaryV2): boolean {
  return (
    (query.world === null || listing.world_id === query.world) &&
    (query.mode === null || listing.mode === query.mode) &&
    categoryAccepts(query, listing.category)
  );
}

/** 公开 v2 目录；按 HTTP 契约编码筛选并核对页壳与筛选回显。 */
export async function fetchConsignmentCatalogV2(
  query: ConsignmentCatalogQuery,
  signal: AbortSignal,
): Promise<Result<CatalogPageV2, ConsignmentReadError>> {
  const params = consignmentCatalogParams(query);
  const response = await readPublicConsignment(
    `${CATALOG_V2}?${params}`,
    ListingSummaryV2PageSchema,
    signal,
    pageIntegers(SUMMARY_INTEGERS),
  );
  if (
    response.ok &&
    (response.value.page !== query.page ||
      response.value.page_size !== query.pageSize ||
      response.value.items.some((listing) => !summaryMatches(query, listing)))
  ) {
    reportBrowserError("dto_decode");
    return err({ kind: "invalid-response" });
  }
  return response;
}

/** 公开 v2 目录页的已解码形状。 */
export interface CatalogPageV2 {
  readonly items: readonly ListingSummaryV2[];
  readonly total: number;
  readonly page: number;
  readonly page_size: number;
}

/** 以规范挂单 UUID 匿名读取 v2 公开详情，拒绝其他挂单的投影。 */
export async function fetchConsignmentDetailV2(
  listingId: string,
  signal: AbortSignal,
): Promise<Result<ListingDetailV2, ConsignmentReadError>> {
  if (!decode(ConsignmentUuidSchema, listingId).ok) return err({ kind: "not-found" });
  const response = await readPublicConsignment(
    `${CATALOG_V2}/${listingId}`,
    ListingDetailV2Schema,
    signal,
    SUMMARY_INTEGERS.map((path) => ["data", "summary", ...path]),
  );
  if (response.ok && response.value.summary.listing_id !== listingId) {
    reportBrowserError("dto_decode");
    return err({ kind: "invalid-response" });
  }
  return response;
}

type Request =
  | { readonly method: "GET" }
  | {
      readonly method: "POST";
      readonly body: object;
      readonly onSessionGuardRejected: (() => void) | undefined;
    };

/** 不变量 1、6、7：绑定预期身份，只发同源请求，不暴露正文或底层异常。 */
async function requestOrders<T>(
  userId: string,
  url: string,
  request: Request,
  status: 200 | 201,
  schema: GenericSchema<unknown, T>,
  signal: AbortSignal,
  integerPaths: NonNullable<JsonFieldPolicy["integerPaths"]>,
): Promise<Result<T, ConsignmentOrderError>> {
  if (!decode(ConsignmentUuidSchema, userId).ok) {
    return err({ status: 400, code: "invalid_expected_user_id" });
  }
  let dispatched = false;
  let unauthorized = false;
  const deferUnauthorized =
    request.method !== "GET" && request.onSessionGuardRejected !== undefined;
  const sent = await sendRequest(
    (requestSignal) => {
      if (requestSignal.aborted) return err({ kind: "network", status: null });
      dispatched = true;
      const headers = { [PUBLIC_EXPECTED_USER_ID_HEADER]: userId, Accept: "application/json" };
      return ok({
        url,
        init: {
          method: request.method,
          credentials: "same-origin",
          cache: "no-store",
          redirect: "error",
          ...(request.method === "GET"
            ? { headers }
            : {
                headers: { ...headers, "content-type": "application/json" },
                body: JSON.stringify(request.body),
              }),
        },
      });
    },
    signal,
    (received) => {
      if (received === 401 && !signal.aborted) {
        unauthorized = true;
        if (!deferUnauthorized) observePublicSessionFailure(received, "unauthenticated", userId);
      }
    },
  );
  if (!dispatched) return err(COMMAND_NOT_SENT);
  if (signal.aborted) return err({ status: 0, code: "network_error" });
  if (unauthorized && request.method === "GET") {
    return err({ status: 401, code: "unauthenticated" });
  }
  const result = sent.ok
    ? decodeOrderResponse(sent.value, status, request.method, schema, integerPaths)
    : err({
        status: sent.error.kind === "network" ? (sent.error.status ?? 0) : 0,
        code: transportErrorCode(sent.error),
      });
  if (!result.ok) {
    const { status: rejectedStatus, code } = result.error;
    const sessionChanged = rejectedStatus === 409 && code === "session_changed";
    if (
      request.method !== "GET" &&
      (sessionChanged || (rejectedStatus === 401 && code === "unauthenticated"))
    ) {
      request.onSessionGuardRejected?.();
    }
    if (deferUnauthorized && unauthorized) {
      observePublicSessionFailure(401, "unauthenticated", userId);
    } else if (sessionChanged) {
      observePublicSessionFailure(rejectedStatus, code, userId);
    }
  }
  return result;
}

function decodeOrderResponse<T>(
  response: TransportResponse,
  status: 200 | 201,
  method: Request["method"],
  schema: GenericSchema<unknown, T>,
  integerPaths: NonNullable<JsonFieldPolicy["integerPaths"]>,
): Result<T, ConsignmentOrderError> {
  const invalid = { status: response.status, code: "invalid_response" };
  const json = response.json({ uniqueKeys: "all", integerPaths });
  if (!json.ok) return err(invalid);
  if (response.status === status) {
    if (method !== "GET" && !jsonHasExactKeys(json.value, SUCCESS_KEYS)) return err(invalid);
    const decoded = decode(apiResponseSchema(schema), json.value);
    return decoded.ok && decoded.value.code === (status === 201 ? "created" : "ok")
      ? ok(decoded.value.data)
      : err(invalid);
  }
  if (response.status < 400) return err(invalid);
  // 不变量 1：写入只有收到完整错误外壳，才可据状态码和协议码判定原命令。
  if (method !== "GET" && !jsonHasExactKeys(json.value, ERROR_KEYS)) return err(invalid);
  const decoded = decode(ApiErrorResponseSchema, json.value);
  if (!decoded.ok || decoded.value.data !== null) return err(invalid);
  const code = ERROR_CODES.has(decoded.value.code) ? decoded.value.code : "unknown_error";
  return err({ status: response.status, code });
}

/** 购买请求字段与回执的校验失败都按结果未知处理，只能原样重放。 */
export type PurchaseResult = Result<PurchaseReceiptV2, ConsignmentOrderError>;

/**
 * 不变量 1、6：提交冻结的购买请求。回执必须回答同一命令、同一挂单与同一标价，
 * 否则按结果未知（invalid_response）处理，调用方只能原样重放，不改成新命令。
 */
export async function purchaseConsignmentListing(
  userId: string,
  listingId: string,
  command: PurchaseRequestV2,
  signal: AbortSignal,
  onSessionGuardRejected?: () => void,
): Promise<PurchaseResult> {
  if (!decode(ConsignmentUuidSchema, listingId).ok) {
    return err({ status: 400, code: "invalid_listing_request" });
  }
  const decoded = decode(PurchaseRequestV2Schema, command);
  if (!decoded.ok) return err({ status: 400, code: "invalid_listing_request" });
  const result = await requestOrders(
    userId,
    `${CATALOG_V2}/${listingId}/purchase`,
    { method: "POST", body: decoded.value, onSessionGuardRejected },
    201,
    PurchaseReceiptV2Schema,
    signal,
    [
      ["data", "listing_version"],
      ["data", "contract_version"],
      ["data", "price"],
    ],
  );
  return result.ok &&
    (result.value.listing_id !== listingId || !purchaseReceiptAnswers(command, result.value))
    ? err({ status: 201, code: "invalid_response" })
    : result;
}

/**
 * 状态与码恰为契约配对的确定业务拒绝。503 `outcome_unknown`、504、断线、解码失败与
 * 未知码都不在此列；503 只有写前的 `funding_unavailable` 可信。
 */
export function purchaseBusinessRejected(error: ConsignmentOrderError): boolean {
  const code = PurchaseRejectionCodeV2Schema.options.find((option) => option === error.code);
  if (code === undefined || code === "outcome_unknown") return false;
  if (purchaseRejectionStatus(code) !== error.status) return false;
  return error.status !== 503 || isTrustedPurchaseRejection(code);
}

/**
 * 不变量 1：首次提交只有本地未发送、会话守卫外壳或确定业务拒绝才可结案。
 * 重放时守卫与未发送只说明本次被拦截，原命令结果仍未知，只认业务拒绝。
 */
export function purchaseWriteRejected(error: ConsignmentOrderError): boolean {
  return (
    commandNotSent(error) ||
    PURCHASE_SHELL[error.code] === error.status ||
    purchaseBusinessRejected(error)
  );
}

/** 本人订单页的筛选参数。 */
export interface OwnedOrderQuery {
  readonly role: OrderRoleV2 | null;
  readonly world: ListingWorld | null;
  readonly state: OrderStateV2 | null;
  readonly page: number;
  readonly pageSize: number;
}

/** 书签与请求编码共用同一份闭集和分页边界。 */
export function validOwnedOrderQuery(query: OwnedOrderQuery): boolean {
  return (
    (query.role === null || OrderRoleV2Schema.options.includes(query.role)) &&
    (query.world === null || query.world === "main" || query.world === "isekai") &&
    (query.state === null || OrderStateV2Schema.options.includes(query.state)) &&
    Number.isSafeInteger(query.page) &&
    query.page >= 1 &&
    Number.isSafeInteger(query.pageSize) &&
    query.pageSize >= 1 &&
    query.pageSize <= MAX_PUBLIC_PAGE_SIZE &&
    (query.page - 1) * query.pageSize <= ORDER_MAX_OFFSET
  );
}

/** 读取本人订单页；响应的页壳与筛选必须与请求相符。 */
export async function fetchOwnedOrders(
  userId: string,
  query: OwnedOrderQuery,
  signal: AbortSignal,
): Promise<Result<OrderPageV2, ConsignmentOrderError>> {
  if (!validOwnedOrderQuery(query)) return err({ status: 400, code: "invalid_query" });
  const params = new URLSearchParams({
    page: String(query.page),
    page_size: String(query.pageSize),
  });
  if (query.role !== null) params.set("role", query.role);
  if (query.world !== null) params.set("world", query.world);
  if (query.state !== null) params.set("state", query.state);
  const result = await requestOrders(
    userId,
    `${ORDERS}?${params}`,
    { method: "GET" },
    200,
    OrderPageV2Schema,
    signal,
    pageIntegers(ORDER_INTEGERS),
  );
  if (!result.ok) return result;
  const { items, page, page_size: pageSize } = result.value;
  if (
    page !== query.page ||
    pageSize !== query.pageSize ||
    new Set(items.map((order) => order.contract_id)).size !== items.length ||
    items.some(
      (order) =>
        (query.role !== null && order.role !== query.role) ||
        (query.world !== null && order.world !== query.world) ||
        (query.state !== null && order.order_state !== query.state),
    )
  ) {
    return err({ status: 200, code: "invalid_response" });
  }
  return result;
}

/** 只接受所请求合同的本人订单投影；非本人与不存在同为 404 listing_not_found。 */
export async function fetchOwnedOrder(
  userId: string,
  contractId: string,
  signal: AbortSignal,
): Promise<Result<OrderV2, ConsignmentOrderError>> {
  if (!decode(ConsignmentUuidSchema, contractId).ok) {
    return err({ status: 400, code: "invalid_listing_id" });
  }
  const result = await requestOrders(
    userId,
    `${ORDERS}/${contractId}`,
    { method: "GET" },
    200,
    OrderV2Schema,
    signal,
    ORDER_INTEGERS.map((path) => ["data", ...path]),
  );
  return result.ok && result.value.contract_id !== contractId
    ? err({ status: 200, code: "invalid_response" })
    : result;
}
