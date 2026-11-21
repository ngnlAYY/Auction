// 寄售公开详情的请求与挂单身份绑定。

import {
  type ListingDetail,
  ListingDetailSchema,
} from "@auction/contracts/dto/public/consignment/catalog";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { decode } from "@auction/contracts/wire";
import { err, type Result } from "@auction/web-shared/result";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";

import { type ConsignmentReadError, readPublicConsignment } from "./consignment-status";

/** 以规范挂单 UUID 匿名读取公开详情，拒绝其他挂单或夹带私有字段的投影。 */
export async function fetchConsignmentDetail(
  listingId: string,
  signal: AbortSignal,
): Promise<Result<ListingDetail, ConsignmentReadError>> {
  if (!decode(ConsignmentUuidSchema, listingId).ok) return err({ kind: "not-found" });
  const response = await readPublicConsignment(
    `/api/v1/consignment/listings/${listingId}`,
    ListingDetailSchema,
    signal,
  );
  if (response.ok && response.value.summary.listing_id !== listingId) {
    reportBrowserError("dto_decode");
    return err({ kind: "invalid-response" });
  }
  return response;
}
