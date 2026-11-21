// 运营挂单列表；筛选与响应中的有效状态、卖家及分页必须相符。

import {
  type AdminListingPage,
  AdminListingPageSchema,
} from "@auction/contracts/dto/admin/consignment";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import {
  ListingModeSchema,
  ListingStateSchema,
  ListingWorldSchema,
} from "@auction/contracts/dto/public/consignment/states";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { err, type Result } from "@auction/web-shared/result";
import { is } from "valibot";

import { invalidResponseError } from "../failure";
import { validate, validPageEcho } from "../pagination";
import type { AdminSessionHost } from "../session";
import { LISTINGS_URL, listingIntegerPaths, readListingResponse } from "./read";

/** 含空串的筛选草稿；非空值须符合各字段的契约。 */
export interface ListingFilters {
  readonly world: string;
  readonly mode: string;
  readonly state: string;
  readonly sellerUserId: string;
}

/** 与寄售读取端的偏移预算一致。 */
export const LISTING_MAX_OFFSET = 10_000;
const LIST_POLICY = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "total"],
    ["data", "page"],
    ["data", "page_size"],
    ...listingIntegerPaths(["data", "items", null, "listing"]),
  ],
} as const;

/** 读取一页运营挂单，包括关闭或尚无解析快照的记录。 */
export async function fetchAdminListings(
  host: AdminSessionHost,
  filters: ListingFilters,
  page: bigint,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminListingPage, string>> {
  if (
    (filters.world !== "" && !is(ListingWorldSchema, filters.world)) ||
    (filters.mode !== "" && !is(ListingModeSchema, filters.mode)) ||
    (filters.state !== "" && !is(ListingStateSchema, filters.state)) ||
    (filters.sellerUserId !== "" && !is(ConsignmentUuidSchema, filters.sellerUserId))
  ) {
    return err("invalid_query");
  }
  const pageNumber = Number(page);
  const bounds = validate(pageNumber, pageSize, MAX_PUBLIC_PAGE_SIZE, LISTING_MAX_OFFSET);
  if (!bounds.ok) return bounds;

  const query = new URLSearchParams({ page: page.toString(), page_size: String(pageSize) });
  const { world, mode, state, sellerUserId } = filters;
  if (world !== "") query.set("world", world);
  if (mode !== "") query.set("mode", mode);
  if (state !== "") query.set("state", state);
  if (sellerUserId !== "") query.set("seller_user_id", sellerUserId);

  const result = await readListingResponse(
    host,
    `${LISTINGS_URL}?${query}`,
    AdminListingPageSchema,
    LIST_POLICY,
    signal,
  );
  if (!result.ok) return result;
  const listingPage = result.value;
  if (
    !validPageEcho(
      pageNumber,
      pageSize,
      listingPage.page,
      listingPage.page_size,
      listingPage.total,
      listingPage.items.length,
    ) ||
    listingPage.items.some(
      ({ listing, seller_user_id: seller }) =>
        (world !== "" && listing.world_id !== world) ||
        (mode !== "" && listing.mode !== mode) ||
        (state !== "" && listing.effective_state !== state) ||
        (sellerUserId !== "" && seller !== sellerUserId),
    )
  ) {
    return err(invalidResponseError("consignment list projection"));
  }
  return result;
}
