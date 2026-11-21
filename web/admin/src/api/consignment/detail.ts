// 读取挂单私有详情，核对挂单身份并保留服务端的碰撞顺序。

import {
  type AdminListingDetail,
  AdminListingDetailSchema,
} from "@auction/contracts/dto/admin/consignment";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { err, type Result } from "@auction/web-shared/result";
import { is } from "valibot";

import { invalidResponseError } from "../failure";
import type { AdminSessionHost } from "../session";
import { LISTINGS_URL, listingIntegerPaths, readListingResponse } from "./read";

const DETAIL_POLICY = {
  uniqueKeys: "all",
  integerPaths: listingIntegerPaths(["data", "record", "listing"]),
} as const;

/** 按规范挂单 UUID 读取运营详情。 */
export async function fetchAdminListing(
  host: AdminSessionHost,
  listingId: string,
  signal: AbortSignal,
): Promise<Result<AdminListingDetail, string>> {
  if (!is(ConsignmentUuidSchema, listingId)) return err("invalid_listing_id");
  const result = await readListingResponse(
    host,
    `${LISTINGS_URL}/${listingId}`,
    AdminListingDetailSchema,
    DETAIL_POLICY,
    signal,
  );
  if (result.ok && result.value.record.listing.listing_id !== listingId) {
    return err(invalidResponseError("consignment detail target"));
  }
  return result;
}
