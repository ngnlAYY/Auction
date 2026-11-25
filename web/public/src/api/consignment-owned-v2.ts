// 寄售本人 v2 列表与详情：含代管态、处分代与服务器给出的允许动作。

import type { ListingMode, ListingWorld } from "@auction/contracts/dto/public/consignment";
import {
  type ListingStateV2,
  ListingStateV2Schema,
  type OwnedListingPageV2,
  OwnedListingPageV2Schema,
  type OwnedListingV2,
  OwnedListingV2Schema,
} from "@auction/contracts/dto/public/consignment/listing-v2";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { err, type Result } from "@auction/web-shared/result";

import {
  type ConsignmentPrivateError,
  requestPrivateConsignment,
  validConsignmentUuid,
} from "./consignment-private";

const OWNED_V2 = "/api/v2/me/consignment";
const OWNED_INTEGERS = [
  ["version"],
  ["price"],
  ["pricing_version"],
  ["fee_bps"],
  ["estimated_fee"],
  ["estimated_seller_amount"],
  ["generation"],
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

/** 本人 v2 页的筛选参数，其中 state 始终匹配有效态。 */
export interface OwnedConsignmentQueryV2 {
  readonly world: ListingWorld | null;
  readonly mode: ListingMode | null;
  readonly state: ListingStateV2 | null;
  readonly page: number;
  readonly pageSize: number;
}

/** 同一份闭集和分页边界供书签及请求编码使用。 */
export function validOwnedConsignmentQueryV2(query: OwnedConsignmentQueryV2): boolean {
  return (
    (query.world === null || query.world === "main" || query.world === "isekai") &&
    (query.mode === null || query.mode === "direct" || query.mode === "consigned") &&
    (query.state === null || ListingStateV2Schema.options.includes(query.state)) &&
    Number.isSafeInteger(query.page) &&
    query.page >= 1 &&
    Number.isSafeInteger(query.pageSize) &&
    query.pageSize >= 1 &&
    query.pageSize <= MAX_PUBLIC_PAGE_SIZE &&
    (query.page - 1) * query.pageSize <= 10_000
  );
}

/** 读取本人 v2 历史；寄售停用时仍走同一个 API。 */
export async function fetchOwnedConsignmentListingsV2(
  userId: string,
  query: OwnedConsignmentQueryV2,
  signal: AbortSignal,
): Promise<Result<OwnedListingPageV2, ConsignmentPrivateError>> {
  if (!validOwnedConsignmentQueryV2(query)) return err({ status: 400, code: "invalid_query" });
  const params = new URLSearchParams({
    page: String(query.page),
    page_size: String(query.pageSize),
  });
  if (query.world !== null) params.set("world", query.world);
  if (query.mode !== null) params.set("mode", query.mode);
  if (query.state !== null) params.set("state", query.state);
  const result = await requestPrivateConsignment(
    userId,
    `${OWNED_V2}/listings?${params}`,
    { method: "GET" },
    200,
    OwnedListingPageV2Schema,
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

/** 只接受所请求的本人 v2 挂单投影，不把 404 当作空详情。 */
export async function fetchOwnedConsignmentListingV2(
  userId: string,
  listingId: string,
  signal: AbortSignal,
): Promise<Result<OwnedListingV2, ConsignmentPrivateError>> {
  if (!validConsignmentUuid(listingId)) return err({ status: 400, code: "invalid_listing_id" });
  const result = await requestPrivateConsignment(
    userId,
    `${OWNED_V2}/listings/${listingId}`,
    { method: "GET" },
    200,
    OwnedListingV2Schema,
    signal,
    DETAIL_INTEGERS,
  );
  return result.ok && result.value.listing_id !== listingId
    ? err({ status: 200, code: "invalid_response" })
    : result;
}
