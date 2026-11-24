// 寄售公开目录的查询编码、请求与响应绑定。

import type { ListingMode, ListingWorld } from "@auction/contracts/dto/public/consignment";
import {
  type ListingSummaryPage,
  ListingSummaryPageSchema,
} from "@auction/contracts/dto/public/consignment/catalog";
import { err, type Result } from "@auction/web-shared/result";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import { type ConsignmentReadError, readPublicConsignment } from "./consignment-status";

/** 公开目录的六项查询条件；空筛选不发到接口。 */
export interface ConsignmentCatalogQuery {
  readonly world: ListingWorld | null;
  readonly mode: ListingMode | null;
  readonly category: string;
  readonly q: string;
  readonly page: number;
  readonly pageSize: number;
}

/** 按 HTTP 契约编码文本、闭集筛选和编号分页。 */
export async function fetchConsignmentCatalog(
  query: ConsignmentCatalogQuery,
  signal: AbortSignal,
): Promise<Result<ListingSummaryPage, ConsignmentReadError>> {
  const params = new URLSearchParams();
  if (query.world !== null) params.set("world", query.world);
  if (query.mode !== null) params.set("mode", query.mode);
  if (query.category !== "") params.set("category", query.category);
  if (query.q !== "") params.set("q", query.q);
  params.set("page", String(query.page));
  params.set("page_size", String(query.pageSize));
  const response = await readPublicConsignment(
    `/api/v1/consignment/listings?${params}`,
    ListingSummaryPageSchema,
    signal,
    [
      ["data", "total"],
      ["data", "page"],
      ["data", "page_size"],
      ["data", "items", null, "version"],
      ["data", "items", null, "price"],
      ["data", "items", null, "level", "value"],
    ],
  );
  if (
    response.ok &&
    (response.value.page !== query.page ||
      response.value.page_size !== query.pageSize ||
      response.value.items.some(
        (listing) =>
          (query.world !== null && listing.world_id !== query.world) ||
          (query.mode !== null && listing.mode !== query.mode) ||
          (query.category !== "" && listing.category !== query.category),
      ))
  ) {
    reportBrowserError("dto_decode");
    return err({ kind: "invalid-response" });
  }
  return response;
}
