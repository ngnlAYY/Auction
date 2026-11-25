// 运营挂单 v2 读取：列表与详情含代管态、有效止售与 position 摘要，筛选与响应须相符。

import {
  type AdminListingDetailV2,
  AdminListingDetailV2Schema,
  type AdminListingPageV2,
  AdminListingPageV2Schema,
} from "@auction/contracts/dto/admin/consignment/listing-v2";
import { ListingStateV2Schema } from "@auction/contracts/dto/public/consignment/listing-v2";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import {
  ListingModeSchema,
  ListingWorldSchema,
} from "@auction/contracts/dto/public/consignment/states";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, type Result } from "@auction/web-shared/result";
import { is } from "valibot";

import { invalidResponseError } from "../failure";
import { validate, validPageEcho } from "../pagination";
import type { AdminSessionHost } from "../session";
import { LISTING_MAX_OFFSET, type ListingFilters } from "./list";
import { listingIntegerPaths, readListingResponse } from "./read";

/** v2 运营挂单集合端点；与 v1 同形，另带代管态。 */
export const LISTINGS_V2_URL = "/api/v2/admin/consignment/listings";

function recordIntegerPaths(
  prefix: readonly (string | null)[],
): NonNullable<JsonFieldPolicy["integerPaths"]> {
  return [
    ...listingIntegerPaths([...prefix, "listing"]),
    [...prefix, "listing", "generation"],
    [...prefix, "position", "version"],
    [...prefix, "position", "generation"],
  ];
}

const LIST_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "total"],
    ["data", "page"],
    ["data", "page_size"],
    ...recordIntegerPaths(["data", "items", null]),
  ],
};
const DETAIL_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: recordIntegerPaths(["data", "record"]),
};

/** 读取一页 v2 运营挂单，有效态筛选取 v2 闭集。 */
export async function fetchAdminListingsV2(
  host: AdminSessionHost,
  filters: ListingFilters,
  page: bigint,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminListingPageV2, string>> {
  if (
    (filters.world !== "" && !is(ListingWorldSchema, filters.world)) ||
    (filters.mode !== "" && !is(ListingModeSchema, filters.mode)) ||
    (filters.state !== "" && !is(ListingStateV2Schema, filters.state)) ||
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
    `${LISTINGS_V2_URL}?${query}`,
    AdminListingPageV2Schema,
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
    new Set(listingPage.items.map(({ listing }) => listing.listing_id)).size !==
      listingPage.items.length ||
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

/** 按规范挂单 UUID 读取 v2 运营详情，并核对返回的挂单身份。 */
export async function fetchAdminListingV2(
  host: AdminSessionHost,
  listingId: string,
  signal: AbortSignal,
): Promise<Result<AdminListingDetailV2, string>> {
  if (!is(ConsignmentUuidSchema, listingId)) return err("invalid_listing_id");
  const result = await readListingResponse(
    host,
    `${LISTINGS_V2_URL}/${listingId}`,
    AdminListingDetailV2Schema,
    DETAIL_POLICY,
    signal,
  );
  if (result.ok && result.value.record.listing.listing_id !== listingId) {
    return err(invalidResponseError("consignment detail target"));
  }
  return result;
}
