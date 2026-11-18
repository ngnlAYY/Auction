// 价格历史筛选器的单位和错误模型。

/** 价格输入支持 Credits（C）、千（K）和百万（M），最终均提交整数 Credits。 */
export type HistoryPriceUnit = "C" | "K" | "M";

/** 日期筛选输入的局部错误。 */
export type HistoryDateError = "incomplete" | "invalid";

/** 价格筛选输入的局部错误。 */
export type HistoryPriceError = "invalid";

/** 价格历史筛选器向页面反馈的稳定错误分类。 */
export type HistoryFilterError =
  | "duplicate"
  | "world_invalid"
  | "player_length"
  | "player_invalid"
  | "name_too_many_terms"
  | "name_term_too_long"
  | "name_too_long"
  | "name_invalid"
  | "from_date_incomplete"
  | "from_date_invalid"
  | "to_date_incomplete"
  | "to_date_invalid"
  | "date_range"
  | "minimum_price_c"
  | "minimum_price_k"
  | "minimum_price_m"
  | "maximum_price_c"
  | "maximum_price_k"
  | "maximum_price_m"
  | "price_range";

/** URL 与表单共用的价格历史字段。 */
export type HistoryFilterField =
  | "world"
  | "name"
  | "from"
  | "to"
  | "minPrice"
  | "maxPrice"
  | "buyer"
  | "seller";

/** 同时保留每个字段的错误，修正一项不掩盖其他无效条件。 */
export type HistoryFilterErrors = Partial<Record<HistoryFilterField, HistoryFilterError>>;

/** 重复参数的全部原文，保留顺序供用户确认要提交的值。 */
export type HistoryFilterDuplicates = Partial<Record<HistoryFilterField, readonly string[]>>;

/** 价格单位对应的线上字符串。 */
export function historyPriceUnitAsStr(unit: HistoryPriceUnit): string {
  return unit;
}

/** 从选择控件的值恢复价格单位；未知值沿用源码的 K 回退。 */
export function historyPriceUnitFromStr(value: string): HistoryPriceUnit {
  if (value === "C") {
    return "C";
  }
  if (value === "M") {
    return "M";
  }
  return "K";
}
