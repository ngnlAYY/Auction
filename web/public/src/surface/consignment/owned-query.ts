// 本人挂单的寄售专属书签与有效状态筛选。

import { ListingStateV2Schema } from "@auction/contracts/dto/public/consignment/listing-v2";
import {
  ListingModeSchema,
  ListingWorldSchema,
} from "@auction/contracts/dto/public/consignment/states";
import { type QueryPair, useQueryState } from "@auction/web-shared/net/query";

import {
  type OwnedConsignmentQueryV2,
  validOwnedConsignmentQueryV2,
} from "../../api/consignment-owned-v2";
import { DEFAULT_PAGE_SIZE } from "../pagination";

const KEYS = [
  "consignment_world",
  "consignment_mode",
  "consignment_state",
  "consignment_page",
  "consignment_page_size",
];
const DEFAULT_QUERY: OwnedConsignmentQueryV2 = {
  world: null,
  mode: null,
  state: null,
  page: 1,
  pageSize: DEFAULT_PAGE_SIZE,
};

function positiveQueryNumber(text: string): number {
  return text !== "" && !/[^0-9]/u.test(text) ? Number(text) : 0;
}

function parse(pairs: readonly QueryPair[]): OwnedConsignmentQueryV2 | null {
  const relevant = pairs.filter(([key]) => KEYS.includes(key));
  const values = new Map(relevant);
  if (values.size !== relevant.length) return null;
  const world = values.get("consignment_world");
  const mode = values.get("consignment_mode");
  const state = values.get("consignment_state");
  const page = values.get("consignment_page") ?? "1";
  const pageSize = values.get("consignment_page_size") ?? String(DEFAULT_PAGE_SIZE);
  const query = {
    world: ListingWorldSchema.options.find((value) => value === world) ?? null,
    mode: ListingModeSchema.options.find((value) => value === mode) ?? null,
    state: ListingStateV2Schema.options.find((value) => value === state) ?? null,
    page: positiveQueryNumber(page),
    pageSize: positiveQueryNumber(pageSize),
  };
  if (
    (world !== undefined && query.world === null) ||
    (mode !== undefined && query.mode === null) ||
    (state !== undefined && query.state === null) ||
    !validOwnedConsignmentQueryV2(query)
  )
    return null;
  return query;
}

function serialize(query: OwnedConsignmentQueryV2 | null): readonly QueryPair[] {
  if (query === null) return [];
  const pairs: QueryPair[] = [];
  if (query.world !== null) pairs.push(["consignment_world", query.world]);
  if (query.mode !== null) pairs.push(["consignment_mode", query.mode]);
  if (query.state !== null) pairs.push(["consignment_state", query.state]);
  if (query.page !== 1) pairs.push(["consignment_page", String(query.page)]);
  if (query.pageSize !== DEFAULT_PAGE_SIZE)
    pairs.push(["consignment_page_size", String(query.pageSize)]);
  return pairs;
}

/** 只替换寄售查询键，不读取或更新拍卖页长偏好。 */
export function useOwnedConsignmentQuery() {
  const location = useQueryState(KEYS, parse, serialize);
  return { ...location, clear: () => location.replace(DEFAULT_QUERY) };
}
