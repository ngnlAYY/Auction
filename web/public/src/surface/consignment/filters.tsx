// 寄售目录的专用 URL 查询与筛选表单。

import { consignmentTextSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { decode } from "@auction/contracts/wire";
import { type QueryPair, useQueryState } from "@auction/web-shared/net/query";
import { createCell } from "@auction/web-shared/reactive/cell";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect } from "solid-js";

import type { ConsignmentCatalogQuery } from "../../api/consignment-catalog";
import { DEFAULT_PAGE_SIZE } from "../pagination";
import { publicOffsetLastPage } from "../pagination/parameters";
import { listingModeKey } from "./labels";

const KEYS = [
  "consignment_world",
  "consignment_mode",
  "consignment_category",
  "consignment_q",
  "consignment_page",
  "consignment_page_size",
];

/** 独立查询的空筛选和契约分页默认值。 */
export const DEFAULT_CONSIGNMENT_QUERY: ConsignmentCatalogQuery = {
  world: null,
  mode: null,
  category: "",
  q: "",
  page: 1,
  pageSize: DEFAULT_PAGE_SIZE,
};

const CATEGORY = consignmentTextSchema(128);
const SEARCH = consignmentTextSchema(320);

function validCategory(category: string): boolean {
  return category === "" || decode(CATEGORY, category).ok;
}

function validSearch(q: string): boolean {
  return q === "" || (decode(SEARCH, q).ok && !/[\u2028\u2029]/u.test(q) && [...q].length <= 80);
}

function positiveInteger(value: string, fallback: number): number {
  if (value === "") return fallback;
  return /^[0-9]+$/u.test(value) ? Number(value) : 0;
}

/** 读取寄售专属查询；重复、非法值和越界偏移失败关闭。 */
export function parseConsignmentQuery(pairs: readonly QueryPair[]): ConsignmentCatalogQuery | null {
  const relevant = pairs.filter(([key]) => KEYS.includes(key));
  const values = new Map(relevant);
  if (values.size !== relevant.length || relevant.some(([, value]) => value === "")) return null;

  const world = values.get("consignment_world") ?? null;
  const mode = values.get("consignment_mode") ?? null;
  const category = values.get("consignment_category") ?? "";
  const q = values.get("consignment_q") ?? "";
  const page = positiveInteger(values.get("consignment_page") ?? "", 1);
  const pageSize = positiveInteger(values.get("consignment_page_size") ?? "", DEFAULT_PAGE_SIZE);
  if (
    (world !== null && world !== "main" && world !== "isekai") ||
    (mode !== null && mode !== "direct" && mode !== "consigned") ||
    !validCategory(category) ||
    !validSearch(q) ||
    pageSize < 1 ||
    pageSize > MAX_PUBLIC_PAGE_SIZE ||
    page < 1 ||
    page > publicOffsetLastPage(pageSize)
  ) {
    return null;
  }
  return { world, mode, category, q, page, pageSize };
}

function serialize(query: ConsignmentCatalogQuery | null): readonly QueryPair[] {
  if (query === null) return [];
  const pairs: QueryPair[] = [];
  if (query.world !== null) pairs.push(["consignment_world", query.world]);
  if (query.mode !== null) pairs.push(["consignment_mode", query.mode]);
  if (query.category !== "") pairs.push(["consignment_category", query.category]);
  if (query.q !== "") pairs.push(["consignment_q", query.q]);
  if (query.page !== 1) pairs.push(["consignment_page", String(query.page)]);
  if (query.pageSize !== DEFAULT_PAGE_SIZE) {
    pairs.push(["consignment_page_size", String(query.pageSize)]);
  }
  return pairs;
}

/** 读取、写回寄售筛选并在浏览器前进、后退时恢复。 */
export function useConsignmentCatalogQuery() {
  return useQueryState(KEYS, parseConsignmentQuery, serialize);
}

/** 两个世界的既有 Public 译文。 */
export function consignmentWorldLabel(world: "main" | "isekai"): string {
  return world === "main" ? t("auction-world-main") : t("auction-world-isekai");
}

/** 以一次表单提交应用筛选并回到第一页。 */
export function Filters(props: {
  readonly query: ConsignmentCatalogQuery | null;
  readonly onApply: (query: ConsignmentCatalogQuery) => void;
}): JSX.Element {
  const draft = createCell(DEFAULT_CONSIGNMENT_QUERY);
  const invalid = createCell<"category" | "q" | null>(null);
  createEffect(
    () => props.query,
    (query) => {
      draft.set(query ?? DEFAULT_CONSIGNMENT_QUERY);
      invalid.set(null);
    },
  );

  return (
    <form
      class="public-consignment-filters"
      onSubmit={(event) => {
        event.preventDefault();
        const query = { ...draft.peek(), page: 1 };
        if (!validCategory(query.category)) {
          invalid.set("category");
        } else if (!validSearch(query.q)) {
          invalid.set("q");
        } else {
          invalid.set(null);
          props.onApply(query);
        }
      }}
    >
      <label>
        {t("consignment-world")}
        <select
          name="world"
          value={draft().world ?? ""}
          onChange={(event) => {
            const value = event.currentTarget.value;
            draft.update((query) => ({
              ...query,
              world: value === "main" || value === "isekai" ? value : null,
            }));
          }}
        >
          <option value="">{t("filter-all")}</option>
          <option value="main">{consignmentWorldLabel("main")}</option>
          <option value="isekai">{consignmentWorldLabel("isekai")}</option>
        </select>
      </label>
      <label>
        {t("consignment-mode")}
        <select
          name="mode"
          value={draft().mode ?? ""}
          onChange={(event) => {
            const value = event.currentTarget.value;
            draft.update((query) => ({
              ...query,
              mode: value === "direct" || value === "consigned" ? value : null,
            }));
          }}
        >
          <option value="">{t("filter-all")}</option>
          <option value="direct">{t(listingModeKey("direct"))}</option>
          <option value="consigned">{t(listingModeKey("consigned"))}</option>
        </select>
      </label>
      {/* 契约只定义来源分类的精确文本，暂没有可枚举的寄售分类闭集。 */}
      <label>
        {t("consignment-category")}
        <input
          name="category"
          value={draft().category}
          onInput={(event) => {
            const category = event.currentTarget.value;
            draft.update((query) => ({ ...query, category }));
          }}
        />
      </label>
      <label>
        {t("consignment-search")}
        <input
          name="q"
          type="search"
          value={draft().q}
          onInput={(event) => {
            const q = event.currentTarget.value;
            draft.update((query) => ({ ...query, q }));
          }}
        />
      </label>
      <button type="submit">{t("history-filter-apply")}</button>
      {invalid() !== null && (
        <p role="alert">
          {invalid() === "category"
            ? t("filter-category-invalid")
            : t("history-filter-error-name-invalid")}
        </p>
      )}
    </form>
  );
}
