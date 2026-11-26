// 寄售目录的专用 URL 查询与筛选区：外观与交互沿用拍卖页（世界、分类）和成交历史页（名称搜索）。
//
// 地址栏键：`world` 归外壳管理（寄售目录与拍卖列表一样是跨世界页面，缺省为全部世界），
// 本文件只拥有 `mode`、`type`、`name`、`consignment_page`、`consignment_page_size`。
// `type` 与拍卖页同为分类闭集代码的逗号列表；请求时由 api/consignment-catalog 映射成后端的规范分类名。

import { consignmentTextSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { decode } from "@auction/contracts/wire";
import { type QueryPair, useQueryState, writeQueryPairs } from "@auction/web-shared/net/query";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, onSettled, Show } from "solid-js";

import type { ConsignmentCatalogQuery } from "../../api/consignment-catalog";
import { WorldTabs } from "../board/world";
import { DEFAULT_PAGE_SIZE } from "../pagination";
import { publicOffsetLastPage } from "../pagination/parameters";
import { CategoryFilter as DesktopCategoryFilter } from "../presentation/desktop";
import { CategoryFilter as MobileCategoryFilter } from "../presentation/mobile";
import { normalizeCatalogCategories } from "../query";
import { listingModeKey } from "./labels";

const KEYS = ["mode", "type", "name", "consignment_page", "consignment_page_size"];

/** 寄售页自己管理的筛选与分页；世界由外壳的 `world` 键给出，请求前再并入。 */
export type ConsignmentFilterQuery = Omit<ConsignmentCatalogQuery, "world">;

/** 独立查询的空筛选和契约分页默认值。 */
export const DEFAULT_CONSIGNMENT_QUERY: ConsignmentFilterQuery = {
  mode: null,
  category: "",
  q: "",
  page: 1,
  pageSize: DEFAULT_PAGE_SIZE,
};

const SEARCH = consignmentTextSchema(320);
const NAME_ID = "public-consignment-name";
const NAME_HINT_ID = "public-consignment-name-hint";
const NAME_ERROR_ID = "public-consignment-name-error";
/** 唯一可能带凭证片段的自由文本筛选；分类是闭集代码，不会携带。 */
type RejectedFilter = "name";

function hasCredentialFragment(value: string): boolean {
  // 不变量 7：沿用下架说明的 ASCII 片段检查，并拦截其他带协议的链接。
  return /:\/\/|\/equip[/?]|key=/i.test(value);
}

function validSearch(q: string): boolean {
  return q === "" || (decode(SEARCH, q).ok && !/[\u2028\u2029]/u.test(q) && [...q].length <= 80);
}

function positiveInteger(value: string, fallback: number): number {
  if (value === "") return fallback;
  return /^[0-9]+$/u.test(value) ? Number(value) : 0;
}

/** 读取寄售专属查询；重复、非法值和越界偏移失败关闭。 */
export function parseConsignmentQuery(pairs: readonly QueryPair[]): ConsignmentFilterQuery | null {
  const relevant = pairs.filter(([key]) => KEYS.includes(key));
  const values = new Map(relevant);
  if (values.size !== relevant.length || relevant.some(([, value]) => value === "")) return null;

  const mode = values.get("mode") ?? null;
  const type = values.get("type");
  const category = type === undefined ? "" : normalizeCatalogCategories(type);
  const q = values.get("name") ?? "";
  const page = positiveInteger(values.get("consignment_page") ?? "", 1);
  const pageSize = positiveInteger(values.get("consignment_page_size") ?? "", DEFAULT_PAGE_SIZE);
  if (
    (mode !== null && mode !== "direct" && mode !== "consigned") ||
    category === null ||
    hasCredentialFragment(q) ||
    !validSearch(q) ||
    pageSize < 1 ||
    pageSize > MAX_PUBLIC_PAGE_SIZE ||
    page < 1 ||
    page > publicOffsetLastPage(pageSize)
  ) {
    return null;
  }
  return { mode, category, q, page, pageSize };
}

function serialize(query: ConsignmentFilterQuery | RejectedFilter | null): readonly QueryPair[] {
  if (query === null || typeof query === "string") return [];
  const pairs: QueryPair[] = [];
  if (query.mode !== null) pairs.push(["mode", query.mode]);
  if (query.category !== "") pairs.push(["type", query.category]);
  if (query.q !== "") pairs.push(["name", query.q]);
  if (query.page !== 1) pairs.push(["consignment_page", String(query.page)]);
  if (query.pageSize !== DEFAULT_PAGE_SIZE) {
    pairs.push(["consignment_page_size", String(query.pageSize)]);
  }
  return pairs;
}

/** 读取、写回寄售筛选并在浏览器前进、后退时恢复。 */
export function useConsignmentCatalogQuery() {
  return useQueryState(
    KEYS,
    (pairs): ConsignmentFilterQuery | RejectedFilter | null => {
      const query = parseConsignmentQuery(pairs);
      if (query !== null) return query;
      const names = pairs.filter(([key]) => key === "name");
      if (!names.some(([, value]) => hasCredentialFragment(value))) return null;
      // 书签里的凭证已在地址栏。替换当前历史项，同时保留无效状态供用户清除。
      writeQueryPairs(
        ["name"],
        names.filter(([, value]) => !hasCredentialFragment(value)),
        "replace",
      );
      return "name";
    },
    serialize,
  );
}

/** 两个世界的既有 Public 译文。 */
export function consignmentWorldLabel(world: "main" | "isekai"): string {
  return world === "main" ? t("auction-world-main") : t("auction-world-isekai");
}

const MODES = ["direct", "consigned"] as const;

/** 世界、挂单模式、装备分类即点即生效，名称搜索以一次表单提交生效；都回到第一页。 */
export function Filters(props: {
  readonly query: ConsignmentFilterQuery | null;
  readonly rejected: RejectedFilter | null;
  readonly onApply: (query: ConsignmentFilterQuery) => void;
  readonly onReject: (field: RejectedFilter) => void;
}): JSX.Element {
  const applied = (): ConsignmentFilterQuery => props.query ?? DEFAULT_CONSIGNMENT_QUERY;
  const apply = (patch: Partial<ConsignmentFilterQuery>): void =>
    props.onApply({ ...applied(), ...patch, page: 1 });

  // 名称草稿只在已应用的名称改变时恢复；切换其他筛选不能打断正在输入的文字。
  const appliedName = createValueMemo(() => props.query?.q ?? "");
  const name = createCell(appliedName());
  const invalid = createCell(false);
  createEffect(appliedName, (value) => {
    name.set(value);
    invalid.set(false);
  });
  const nameInvalid = () => invalid() && props.rejected === null;
  const categoryFocusRequest = createCell(false);
  const activeCategory = () => (applied().category === "" ? "all" : applied().category);
  const changeCategory = (category: string): boolean => {
    apply({ category: category === "all" ? "" : category });
    return true;
  };

  return (
    <div class="public-auction-live">
      <div class="public-auction-group-toolbar">
        <WorldTabs />
        <nav class="public-auction-filter" aria-label={t("consignment-mode")}>
          <span class="public-auction-filter__label">{t("consignment-mode")}</span>
          <button
            type="button"
            class="public-auction-filter__tab"
            aria-pressed={applied().mode === null ? "true" : "false"}
            aria-current={applied().mode === null ? "true" : "false"}
            onClick={() => apply({ mode: null })}
          >
            {t("filter-all")}
          </button>
          <For each={MODES} keyed={false}>
            {(mode) => (
              <button
                type="button"
                class="public-auction-filter__tab"
                aria-pressed={applied().mode === mode() ? "true" : "false"}
                aria-current={applied().mode === mode() ? "true" : "false"}
                onClick={() => apply({ mode: mode() })}
              >
                {t(listingModeKey(mode()))}
              </button>
            )}
          </For>
        </nav>
      </div>
      <div class="public-auction-type-toolbar">
        <DesktopCategoryFilter active={activeCategory()} onChange={changeCategory} />
        <MobileCategoryFilter
          active={activeCategory()}
          focusRequest={categoryFocusRequest}
          onChange={changeCategory}
        />
        {/* 名称搜索与装备分类同在类型栏一行；说明文字作输入框的无障碍描述与悬停提示。 */}
        <form
          class="public-consignment-search"
          autocomplete="off"
          novalidate
          onSubmit={(event) => {
            event.preventDefault();
            const q = name.peek();
            if (hasCredentialFragment(q)) {
              name.set("");
              props.onReject("name");
            } else if (!validSearch(q)) {
              invalid.set(true);
              const form = event.currentTarget;
              onSettled(() => form.querySelector<HTMLElement>(`#${NAME_ID}`)?.focus());
            } else {
              invalid.set(false);
              apply({ q });
            }
          }}
        >
          <div class="public-history-primary-filters">
            <label for={NAME_ID}>{t("history-filter-name")}</label>
            <input
              id={NAME_ID}
              name="name"
              type="search"
              enterkeyhint="search"
              autocapitalize="none"
              spellcheck="false"
              title={t("consignment-name-hint")}
              value={name()}
              aria-invalid={nameInvalid() || props.rejected !== null ? "true" : "false"}
              aria-describedby={
                nameInvalid() || props.rejected !== null
                  ? `${NAME_HINT_ID} ${NAME_ERROR_ID}`
                  : NAME_HINT_ID
              }
              onInput={(event) => {
                name.set(event.currentTarget.value);
                invalid.set(false);
              }}
              onBlur={() =>
                invalid.set(!hasCredentialFragment(name.peek()) && !validSearch(name.peek()))
              }
            />
            <div class="public-history-filter-toolbar">
              <button type="submit" class="public-history-filter-actions__apply">
                {t("history-filter-apply")}
              </button>
            </div>
          </div>
          <p id={NAME_HINT_ID} class="public-consignment-search__hint">
            {t("consignment-name-hint")}
          </p>
          <Show when={nameInvalid() || props.rejected !== null}>
            <p id={NAME_ERROR_ID} class="public-history-field-error" role="alert">
              {t("history-filter-error-name-invalid")}
            </p>
          </Show>
        </form>
      </div>
    </div>
  );
}
