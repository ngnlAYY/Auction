// 寄售公开目录的查询编码、请求与响应绑定。

import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import type { ListingMode, ListingWorld } from "@auction/contracts/dto/public/consignment";
import {
  type ListingSummaryPage,
  ListingSummaryPageSchema,
} from "@auction/contracts/dto/public/consignment/catalog";
import { err, type Result } from "@auction/web-shared/result";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import { type ConsignmentReadError, readPublicConsignment } from "./consignment-status";

/** 公开目录的六项查询条件；空筛选不发到接口。
 * `category` 是拍卖页同款的分类代码逗号列表（地址栏 `type`，如 "one,sta"），空串表示不筛选；
 * 请求时由 `consignmentCatalogParams` 映射为后端的规范分类名。 */
export interface ConsignmentCatalogQuery {
  readonly world: ListingWorld | null;
  readonly mode: ListingMode | null;
  readonly category: string;
  readonly q: string;
  readonly page: number;
  readonly pageSize: number;
}

/** 分类代码列表对应的规范分类名；空串得到空数组。代码已由地址栏解析校验过闭集。 */
function categoryNames(category: string): readonly string[] {
  if (category === "") return [];
  return category
    .split(",")
    .map((code) => EQUIPMENT_CATEGORY_CODES.find(([key]) => key === code)?.[1] ?? code);
}

/** 响应条目的分类是否落在所选分类内；未选分类时任何条目都符合。 */
export function categoryAccepts(query: ConsignmentCatalogQuery, category: string): boolean {
  const names = categoryNames(query.category);
  return names.length === 0 || names.includes(category);
}

/** 目录请求的查询参数：分类按后端格式写成逗号分隔的规范分类名。 */
export function consignmentCatalogParams(query: ConsignmentCatalogQuery): URLSearchParams {
  const params = new URLSearchParams();
  if (query.world !== null) params.set("world", query.world);
  if (query.mode !== null) params.set("mode", query.mode);
  if (query.category !== "") params.set("category", categoryNames(query.category).join(","));
  if (query.q !== "") params.set("q", query.q);
  params.set("page", String(query.page));
  params.set("page_size", String(query.pageSize));
  return params;
}

/** 按 HTTP 契约编码文本、闭集筛选和编号分页。 */
export async function fetchConsignmentCatalog(
  query: ConsignmentCatalogQuery,
  signal: AbortSignal,
): Promise<Result<ListingSummaryPage, ConsignmentReadError>> {
  const params = consignmentCatalogParams(query);
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
          !categoryAccepts(query, listing.category),
      ))
  ) {
    reportBrowserError("dto_decode");
    return err({ kind: "invalid-response" });
  }
  return response;
}
