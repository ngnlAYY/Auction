// 已应用的历史筛选、可编辑草稿与提交校验。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createEffect, untrack } from "solid-js";

import type { PriceHistoryQuery } from "../../../../api/model";
import { updatePublicQueryPairs, usePublicQuerySearch } from "../../../pagination/location";
import {
  currentBrowserSearch,
  type PublicBoardQuery,
  readPublicQuery,
  type WorldSwitchOption,
} from "../../../query";
import {
  HISTORY_FILTER_KEYS,
  historyApplyChangesQuery,
  historyDateParts,
  historyFormFilters,
  historyPriceParts,
  historyQueryErrors,
  historyQueryFromUrl,
  historyResetChangesQuery,
  historyWorldDraftForApply,
} from "../values";
import type {
  HistoryFilterError,
  HistoryFilterErrors,
  HistoryFilterField,
  HistoryPriceUnit,
} from "./model";
import { historyDateValue, historyNameValue, historyPriceValue } from "./validation";

/** 历史表单使用年月与金额单位，地址栏使用日期与整数 Credits。 */
export interface HistoryDraft {
  readonly world: string;
  readonly name: string;
  readonly fromMonth: string;
  readonly fromYear: string;
  readonly toMonth: string;
  readonly toYear: string;
  readonly minimumPrice: string;
  readonly minimumPriceUnit: HistoryPriceUnit;
  readonly maximumPrice: string;
  readonly maximumPriceUnit: HistoryPriceUnit;
  readonly buyer: string;
  readonly seller: string;
}

function draftFromQuery(query: PriceHistoryQuery): HistoryDraft {
  const [fromMonth, fromYear] = historyDateParts(query.from);
  const [toMonth, toYear] = historyDateParts(query.to);
  const [minimumPrice, minimumPriceUnit] = historyPriceParts(query.minPrice);
  const [maximumPrice, maximumPriceUnit] = historyPriceParts(query.maxPrice);
  return {
    world: query.world ?? "all",
    name: query.name ?? "",
    fromMonth,
    fromYear,
    toMonth,
    toYear,
    minimumPrice,
    minimumPriceUnit,
    maximumPrice,
    maximumPriceUnit,
    buyer: query.buyer ?? "",
    seller: query.seller ?? "",
  };
}

interface HistoryFieldIssue {
  readonly code: HistoryFilterError;
  readonly input: string | readonly string[] | null;
}

type HistoryFilterIssues = Partial<Record<HistoryFilterField, HistoryFieldIssue>>;
type AddressFilters = ReturnType<typeof historyQueryFromUrl>;

function isRangeError(code: HistoryFilterError | undefined): boolean {
  return code === "date_range" || code === "price_range";
}

function issuesFromValidation(
  codes: HistoryFilterErrors,
  inputFor: (field: HistoryFilterField) => HistoryFieldIssue["input"],
): HistoryFilterIssues {
  const issues: HistoryFilterIssues = {};
  for (const field of FILTER_FIELDS) {
    const code = codes[field];
    if (code !== undefined) issues[field] = { code, input: inputFor(field) };
  }
  return issues;
}

function addressInput(
  current: AddressFilters,
  field: HistoryFilterField,
): HistoryFieldIssue["input"] {
  return current.duplicates[field] ?? current.query[field];
}

function issuesFromAddress(current: AddressFilters): HistoryFilterIssues {
  return issuesFromValidation(current.errors, (field) => addressInput(current, field));
}

function queryFromDraft(
  draft: HistoryDraft,
  current: AddressFilters,
  edited: ReadonlySet<HistoryFilterField>,
) {
  const applied = current.query;
  const name = historyNameValue(draft.name);
  const from = historyDateValue(draft.fromMonth, draft.fromYear, false, applied.from);
  const to = historyDateValue(draft.toMonth, draft.toYear, true, applied.to);
  const minPrice = historyPriceValue(draft.minimumPrice, draft.minimumPriceUnit);
  const maxPrice = historyPriceValue(draft.maximumPrice, draft.maximumPriceUnit);
  const query: PriceHistoryQuery = {
    world: draft.world === "all" ? null : draft.world,
    name: name.ok ? name.value : draft.name,
    from: from.ok ? from.value : null,
    to: to.ok ? to.value : null,
    minPrice: minPrice.ok ? minPrice.value : null,
    maxPrice: maxPrice.ok ? maxPrice.value : null,
    buyer: draft.buyer === "" ? null : draft.buyer,
    seller: draft.seller === "" ? null : draft.seller,
    sort: applied.sort,
    page: 1,
  };
  const restoredErrors: HistoryFilterErrors = {};
  for (const field of FILTER_FIELDS) {
    const restored = current.errors[field];
    // 地址原文的字段错误只由该字段的编辑解除。区间错误始终由两端重算。
    if (!edited.has(field) && restored !== undefined && !isRangeError(restored)) {
      restoredErrors[field] = restored;
    }
  }
  const fieldErrors = { ...restoredErrors };
  // 查询用规范化结果；名称的总字节上限仍须校验用户输入原文。
  if (!name.ok) fieldErrors.name ??= name.error;
  if (!from.ok)
    fieldErrors.from ??= from.error === "invalid" ? "from_date_invalid" : "from_date_incomplete";
  if (!to.ok) fieldErrors.to ??= to.error === "invalid" ? "to_date_invalid" : "to_date_incomplete";
  if (!minPrice.ok) {
    const unit = draft.minimumPriceUnit;
    fieldErrors.minPrice ??=
      unit === "C" ? "minimum_price_c" : unit === "K" ? "minimum_price_k" : "minimum_price_m";
  }
  if (!maxPrice.ok) {
    const unit = draft.maximumPriceUnit;
    fieldErrors.maxPrice ??=
      unit === "C" ? "maximum_price_c" : unit === "K" ? "maximum_price_k" : "maximum_price_m";
  }
  const errors = issuesFromValidation(historyQueryErrors(query, fieldErrors), (field) => {
    if (restoredErrors[field] !== undefined) return addressInput(current, field);
    // 提示与本次校验取同一份值。未通过格式校验的金额仍保留输入原文。
    if (field === "minPrice") return query.minPrice ?? draft.minimumPrice;
    if (field === "maxPrice") return query.maxPrice ?? draft.maximumPrice;
    return query[field];
  });
  return { query, errors };
}

const FILTER_FIELDS: readonly HistoryFilterField[] = [
  "name",
  "world",
  "from",
  "to",
  "minPrice",
  "maxPrice",
  "buyer",
  "seller",
];
const FILTER_QUERY_KEYS = [...HISTORY_FILTER_KEYS.map(([, key]) => key), "page"];
const RESET_QUERY_KEYS = [...FILTER_QUERY_KEYS, "history_sort", "history_group"];

const RANGE_PARTNERS: Partial<Record<HistoryFilterField, HistoryFilterField>> = {
  from: "to",
  to: "from",
  minPrice: "maxPrice",
  maxPrice: "minPrice",
};

function invalidSettings(errors: HistoryFilterIssues): boolean {
  return FILTER_FIELDS.some((field) => field !== "name" && errors[field] !== undefined);
}

/** 每个入口使用同一校验，地址栏拒绝写入后恢复已应用筛选。 */
export function useHistoryFilters(
  state: Cell<PublicBoardQuery>,
  worldOptions: () => readonly WorldSwitchOption[],
) {
  const search = usePublicQuerySearch();
  const applied = createValueMemo(() => historyQueryFromUrl(state(), search()));
  // 翻页、排序与分组不覆盖未提交的草稿。
  const appliedFilters = createValueMemo(() => {
    const { query, errors, duplicates } = applied();
    return { query: historyFormFilters(query), errors, duplicates };
  });
  const initial = untrack(appliedFilters);
  const draft = createCell(draftFromQuery(initial.query));
  const errors = createCell(issuesFromAddress(initial));
  const settingsOpen = createCell(invalidSettings(errors.peek()));
  const interacted = createCell<readonly HistoryFilterField[]>([]);
  const edited = new Set<HistoryFilterField>();
  const restore = (current: ReturnType<typeof appliedFilters>) => {
    const issues = issuesFromAddress(current);
    draft.set(draftFromQuery(current.query));
    errors.set(issues);
    interacted.set([]);
    edited.clear();
    if (invalidSettings(issues)) {
      settingsOpen.set(true);
    }
  };
  createEffect(appliedFilters, restore);
  createEffect(worldOptions, (options) => {
    const currentWorld = draft.peek().world;
    const appliedWorld = untrack(appliedFilters).query.world;
    const world = historyWorldDraftForApply(currentWorld, options, appliedWorld);
    if (world !== currentWorld) {
      draft.update((current) => ({ ...current, world }));
    }
  });

  const writeFilters = (candidate: PriceHistoryQuery, reset = false) =>
    updatePublicQueryPairs(
      state,
      reset ? RESET_QUERY_KEYS : FILTER_QUERY_KEYS,
      HISTORY_FILTER_KEYS.flatMap(([field, key]) => {
        const value = candidate[field];
        return value === null ? [] : [[key, value] as const];
      }),
      "push",
    );

  const validateDraft = (current: AddressFilters = untrack(applied)) => {
    const original = draft.peek();
    const world = historyWorldDraftForApply(
      original.world,
      untrack(worldOptions),
      current.query.world,
    );
    return queryFromDraft({ ...original, world }, current, edited);
  };

  const updateField = (field: HistoryFilterField, patch: Partial<HistoryDraft>) => {
    edited.add(field);
    draft.update((current) => ({ ...current, ...patch }));
    if (Object.keys(errors.peek()).length === 0) {
      return;
    }
    const nextErrors = validateDraft().errors;
    const remaining = { ...errors.peek() };
    for (const key of FILTER_FIELDS) {
      // 输入期间只清除失效的提示。新的错误由失焦或提交显示。
      const updated = nextErrors[key];
      if (updated === undefined || remaining[key]?.code !== updated.code) {
        delete remaining[key];
      } else {
        remaining[key] = updated;
      }
    }
    errors.set(remaining);
  };

  const blur = (field: HistoryFilterField) => {
    const partner = RANGE_PARTNERS[field];
    if (!edited.has(field) && (partner === undefined || !edited.has(partner))) {
      return null;
    }
    const validated = validateDraft().errors;
    const next = { ...errors.peek() };
    const touched = new Set(interacted.peek());
    touched.add(field);
    let announcement: HistoryFilterError | null = null;
    for (const key of partner === undefined ? [field] : [field, partner]) {
      const previous = next[key]?.code;
      const issue = validated[key];
      // 区间提示挂在下界。任一端失焦都要刷新它，对端自己的字段提示仍由对端交互决定。
      if (key !== field && !isRangeError(previous) && !isRangeError(issue?.code)) continue;
      if (issue === undefined) {
        delete next[key];
      } else {
        next[key] = issue;
        if (issue.code !== previous) announcement ??= issue.code;
        if (isRangeError(issue.code)) touched.add(key);
      }
    }
    errors.set(next);
    interacted.set([...touched]);
    return announcement;
  };

  const apply = (): HistoryFilterField | null => {
    // 以真实地址为锚点，保留同一月份中未修改的有效精确日期。
    const current = historyQueryFromUrl(readPublicQuery(), currentBrowserSearch());
    const candidate = validateDraft(current);
    const firstError = FILTER_FIELDS.find((field) => candidate.errors[field] !== undefined);
    if (firstError !== undefined) {
      errors.set(candidate.errors);
      interacted.set(FILTER_FIELDS);
      if (invalidSettings(candidate.errors)) {
        settingsOpen.set(true);
      }
      return firstError;
    }
    const changes =
      historyApplyChangesQuery(current.query, candidate.query) ||
      Object.keys(current.errors).length !== 0;
    // URL 是已应用筛选的提交点；写入被拒时表单继续对应同一份已应用查询。
    if (changes && !writeFilters(candidate.query)) {
      restore(current);
      return null;
    }
    errors.set({});
    const original = draft.peek();
    const world = candidate.query.world ?? "all";
    if (world !== original.world) {
      draft.set({ ...original, world });
    }
    return null;
  };

  const reset = () => {
    const currentBoard = readPublicQuery();
    const current = historyQueryFromUrl(currentBoard, currentBrowserSearch());
    const cleared: PriceHistoryQuery = {
      world: null,
      name: null,
      from: null,
      to: null,
      minPrice: null,
      maxPrice: null,
      buyer: null,
      seller: null,
      sort: "closed_desc",
      page: 1,
    };
    const changes = historyResetChangesQuery(current, currentBoard.historyGroup);
    if (changes && !writeFilters(cleared, true)) {
      restore(current);
      return;
    }
    restore({ query: cleared, errors: {}, duplicates: {} });
    settingsOpen.set(false);
  };

  return { applied, draft, errors, settingsOpen, interacted, updateField, blur, apply, reset };
}
