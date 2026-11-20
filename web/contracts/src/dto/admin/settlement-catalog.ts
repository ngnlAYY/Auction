// Admin 结算报表选择目录 DTO。

import * as v from "valibot";
import { integer, strictStruct } from "../../wire";
import type { AdminStrictPaginationResponse } from "./pagination";
import { AdminStrictPaginationResponseSchema } from "./pagination";

export const AdminSettlementCatalogEntryResponseSchema = strictStruct({
  auction_id: v.string(),
  world_id: v.string(),
  sequence: integer,
  settled_at: v.string(),
});

export type AdminSettlementCatalogEntryResponse = v.InferOutput<
  typeof AdminSettlementCatalogEntryResponseSchema
>;

/** 结算报表选择目录的数字分页。 */
export type AdminSettlementCatalogPaginationResponse = AdminStrictPaginationResponse;

export const AdminSettlementCatalogResponseSchema = strictStruct({
  auctions: v.pipe(v.array(AdminSettlementCatalogEntryResponseSchema), v.readonly()),
  pagination: AdminStrictPaginationResponseSchema,
});

export type AdminSettlementCatalogResponse = v.InferOutput<
  typeof AdminSettlementCatalogResponseSchema
>;
