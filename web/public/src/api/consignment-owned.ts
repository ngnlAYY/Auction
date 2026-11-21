// 寄售本人列表、本人详情与命令回执的读取和请求目标核对。

import {
  type ListingMode,
  type ListingState,
  type ListingWorld,
  type OwnedListing,
  type OwnedListingPage,
  OwnedListingPageSchema,
  OwnedListingSchema,
  type StoredCommandResult,
  StoredCommandResultSchema,
} from "@auction/contracts/dto/public/consignment";
import { ListingStateSchema } from "@auction/contracts/dto/public/consignment/states";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { err, type Result } from "@auction/web-shared/result";

import {
  type ConsignmentPrivateError,
  requestPrivateConsignment,
  validConsignmentUuid,
} from "./consignment-private";

const OWNED = "/api/v1/me/consignment";
const OWNED_INTEGERS = [
  ["version"],
  ["price"],
  ["pricing_version"],
  ["fee_bps"],
  ["estimated_fee"],
  ["estimated_seller_amount"],
  ["view", "progress", "attempts"],
  ["view", "progress", "max_attempts"],
  ["view", "equipment", "level", "value"],
] as const;
const PAGE_INTEGERS = [
  ["data", "total"],
  ["data", "page"],
  ["data", "page_size"],
  ...OWNED_INTEGERS.map((path) => ["data", "items", null, ...path]),
];
const DETAIL_INTEGERS = OWNED_INTEGERS.map((path) => ["data", ...path]);
const STORED_RECEIPT_INTEGERS = [
  ["data", "http_status"],
  ["data", "receipt", "listing_version"],
  ["data", "receipt", "price"],
] as const;

/** 本人页的筛选参数，其中 state 始终匹配有效态。 */
export interface OwnedConsignmentQuery {
  readonly world: ListingWorld | null;
  readonly mode: ListingMode | null;
  readonly state: ListingState | null;
  readonly page: number;
  readonly pageSize: number;
}

/** 同一份闭集和分页边界供书签及请求编码使用。 */
export function validOwnedConsignmentQuery(query: OwnedConsignmentQuery): boolean {
  return (
    (query.world === null || query.world === "main" || query.world === "isekai") &&
    (query.mode === null || query.mode === "direct" || query.mode === "consigned") &&
    (query.state === null || ListingStateSchema.options.includes(query.state)) &&
    Number.isSafeInteger(query.page) &&
    query.page >= 1 &&
    Number.isSafeInteger(query.pageSize) &&
    query.pageSize >= 1 &&
    query.pageSize <= MAX_PUBLIC_PAGE_SIZE &&
    (query.page - 1) * query.pageSize <= 10_000
  );
}

/** 读取本人历史，服务关闭时仍走同一个 API。 */
export async function fetchOwnedConsignmentListings(
  userId: string,
  query: OwnedConsignmentQuery,
  signal: AbortSignal,
): Promise<Result<OwnedListingPage, ConsignmentPrivateError>> {
  if (!validOwnedConsignmentQuery(query)) return err({ status: 400, code: "invalid_query" });
  const params = new URLSearchParams({
    page: String(query.page),
    page_size: String(query.pageSize),
  });
  if (query.world !== null) params.set("world", query.world);
  if (query.mode !== null) params.set("mode", query.mode);
  if (query.state !== null) params.set("state", query.state);
  const result = await requestPrivateConsignment(
    userId,
    `${OWNED}/listings?${params}`,
    { method: "GET" },
    200,
    OwnedListingPageSchema,
    signal,
    PAGE_INTEGERS,
  );
  if (!result.ok) return result;
  const { items, page, page_size } = result.value;
  if (
    page !== query.page ||
    page_size !== query.pageSize ||
    new Set(items.map((listing) => listing.listing_id)).size !== items.length ||
    items.some(
      (listing) =>
        (query.world !== null && listing.world_id !== query.world) ||
        (query.mode !== null && listing.mode !== query.mode) ||
        (query.state !== null && listing.effective_state !== query.state),
    )
  )
    return err({ status: 200, code: "invalid_response" });
  return result;
}

/** 只接受所请求的本人挂单投影，不把 404 当作空详情。 */
export async function fetchOwnedConsignmentListing(
  userId: string,
  listingId: string,
  signal: AbortSignal,
): Promise<Result<OwnedListing, ConsignmentPrivateError>> {
  if (!validConsignmentUuid(listingId)) return err({ status: 400, code: "invalid_listing_id" });
  const result = await requestPrivateConsignment(
    userId,
    `${OWNED}/listings/${listingId}`,
    { method: "GET" },
    200,
    OwnedListingSchema,
    signal,
    DETAIL_INTEGERS,
  );
  return result.ok && result.value.listing_id !== listingId
    ? err({ status: 200, code: "invalid_response" })
    : result;
}

/** 账本查询的 GET 200 只包裹原写入结果；只有匹配的命令可交给恢复界面。 */
export async function fetchConsignmentCommand(
  userId: string,
  commandId: string,
  signal: AbortSignal,
): Promise<Result<StoredCommandResult, ConsignmentPrivateError>> {
  if (!validConsignmentUuid(commandId)) return err({ status: 400, code: "invalid_command_id" });
  const result = await requestPrivateConsignment(
    userId,
    `${OWNED}/commands/${commandId}`,
    { method: "GET" },
    200,
    StoredCommandResultSchema,
    signal,
    STORED_RECEIPT_INTEGERS,
  );
  return result.ok &&
    (result.value.receipt.command_id !== commandId ||
      result.value.receipt.operation === "admin_delist")
    ? err({ status: 200, code: "invalid_response" })
    : result;
}
