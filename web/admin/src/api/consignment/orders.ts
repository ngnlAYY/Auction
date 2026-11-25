// 运营订单读取：列表、详情与证据分页；筛选、页壳与合同身份必须与请求相符。

import {
  AdminOrderPageV2Schema,
  type AdminOrderV2,
  AdminOrderV2Schema,
  OrderEvidencePageV2Schema,
} from "@auction/contracts/dto/admin/consignment/order-v2";
import { OrderStateV2Schema } from "@auction/contracts/dto/public/consignment/order-v2";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { ListingWorldSchema } from "@auction/contracts/dto/public/consignment/states";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, type Result } from "@auction/web-shared/result";
import { type InferOutput, is } from "valibot";

import { invalidResponseError } from "../failure";
import { validate, validPageEcho } from "../pagination";
import type { AdminSessionHost } from "../session";
import { LISTING_MAX_OFFSET } from "./list";
import { readListingResponse } from "./read";

/** 运营订单分页。 */
export type AdminOrderPageV2 = InferOutput<typeof AdminOrderPageV2Schema>;
/** 订单证据分页。 */
export type OrderEvidencePageV2 = InferOutput<typeof OrderEvidencePageV2Schema>;

/** 运营订单集合端点。 */
export const ORDERS_URL = "/api/v2/admin/consignment/orders";

/** 含空串的筛选草稿；非空值须符合各字段的契约。 */
export interface OrderFilters {
  readonly world: string;
  readonly state: string;
}

const ORDER_INTEGERS = [["version"], ["price"], ["fee"], ["seller_amount"]] as const;
const PAGE_INTEGERS = [
  ["data", "total"],
  ["data", "page"],
  ["data", "page_size"],
] as const;
const LIST_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ...PAGE_INTEGERS,
    ...ORDER_INTEGERS.map((path) => ["data", "items", null, ...path]),
  ],
};
const DETAIL_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: ORDER_INTEGERS.map((path) => ["data", ...path]),
};
const EVIDENCE_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ...PAGE_INTEGERS,
    ["data", "items", null, "round_number"],
    ["data", "items", null, "fence"],
    ["data", "items", null, "amount"],
  ],
};

/** 读取一页运营订单。 */
export async function fetchAdminOrders(
  host: AdminSessionHost,
  filters: OrderFilters,
  page: bigint,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminOrderPageV2, string>> {
  if (
    (filters.world !== "" && !is(ListingWorldSchema, filters.world)) ||
    (filters.state !== "" && !is(OrderStateV2Schema, filters.state))
  ) {
    return err("invalid_query");
  }
  const pageNumber = Number(page);
  const bounds = validate(pageNumber, pageSize, MAX_PUBLIC_PAGE_SIZE, LISTING_MAX_OFFSET);
  if (!bounds.ok) return bounds;
  const query = new URLSearchParams({ page: page.toString(), page_size: String(pageSize) });
  if (filters.world !== "") query.set("world", filters.world);
  if (filters.state !== "") query.set("state", filters.state);

  const result = await readListingResponse(
    host,
    `${ORDERS_URL}?${query}`,
    AdminOrderPageV2Schema,
    LIST_POLICY,
    signal,
  );
  if (!result.ok) return result;
  const orders = result.value;
  if (
    !validPageEcho(
      pageNumber,
      pageSize,
      orders.page,
      orders.page_size,
      orders.total,
      orders.items.length,
    ) ||
    new Set(orders.items.map((order) => order.contract_id)).size !== orders.items.length ||
    orders.items.some(
      (order) =>
        (filters.world !== "" && order.world !== filters.world) ||
        (filters.state !== "" && order.order_state !== filters.state),
    )
  ) {
    return err(invalidResponseError("consignment order list projection"));
  }
  return result;
}

/** 按规范合同 UUID 读取运营订单，并核对返回的合同身份。 */
export async function fetchAdminOrder(
  host: AdminSessionHost,
  contractId: string,
  signal: AbortSignal,
): Promise<Result<AdminOrderV2, string>> {
  if (!is(ConsignmentUuidSchema, contractId)) return err("invalid_listing_id");
  const result = await readListingResponse(
    host,
    `${ORDERS_URL}/${contractId}`,
    AdminOrderV2Schema,
    DETAIL_POLICY,
    signal,
  );
  if (result.ok && result.value.contract_id !== contractId) {
    return err(invalidResponseError("consignment order target"));
  }
  return result;
}

/** 读取订单证据的一页；证据不含预约码、装备 key 与原信正文。 */
export async function fetchAdminOrderEvidence(
  host: AdminSessionHost,
  contractId: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<OrderEvidencePageV2, string>> {
  if (!is(ConsignmentUuidSchema, contractId)) return err("invalid_listing_id");
  const bounds = validate(page, pageSize, MAX_PUBLIC_PAGE_SIZE, LISTING_MAX_OFFSET);
  if (!bounds.ok) return bounds;
  const query = new URLSearchParams({ page: String(page), page_size: String(pageSize) });
  const result = await readListingResponse(
    host,
    `${ORDERS_URL}/${contractId}/evidence?${query}`,
    OrderEvidencePageV2Schema,
    EVIDENCE_POLICY,
    signal,
  );
  if (!result.ok) return result;
  const evidence = result.value;
  if (
    !validPageEcho(
      page,
      pageSize,
      evidence.page,
      evidence.page_size,
      evidence.total,
      evidence.items.length,
    )
  ) {
    return err(invalidResponseError("consignment order evidence projection"));
  }
  return result;
}
