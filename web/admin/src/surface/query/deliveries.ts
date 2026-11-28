// Delivery 列表的 query 状态：收件人/引用/模板/类别/世界/时间筛选，页码与排序。
// 筛选输入框是提交前草稿，不入 URL；动作对话框与对账草稿是瞬态，不入 URL。

import { type QueryPair, type QueryState, useQueryState } from "@auction/web-shared/net/query";

import type { DeliverySearchFilters } from "../../api/delivery/queue";
import { parseStrictRfc3339 } from "../../api/session-time";
import { DELIVERY_STATUS_VALUES } from "../deliveries/labels";
import type { SortDirection } from "../sort";
import {
  parseChoice,
  parsePage,
  parseSortColumn,
  parseSortDirection,
  parseText,
  parseUuidText,
  pushPage,
  pushSort,
  pushText,
} from "./primitives";

const DELIVERIES_KEYS: readonly string[] = [
  "status",
  "recipient",
  "reference",
  "template",
  "category",
  "world",
  "from",
  "to",
  "page",
  "sort",
  "dir",
];

const DELIVERY_CATEGORY_VALUES: readonly string[] = [
  "outbid",
  "account",
  "intake",
  "settlement",
  "equipment",
  "operations",
  "manual",
];

/** Delivery 列表可排序列：投递 ID、模板、类别、到付金额、状态、尝试次数。 */
const DELIVERIES_SORT_COLUMNS: readonly string[] = [
  "id",
  "template",
  "category",
  "cod",
  "status",
  "attempts",
];

/** Delivery 列表的筛选、页码与排序。 */
export interface DeliveriesQuery {
  /** 空串表示全部状态；合法值见 {@link DELIVERY_STATUS_VALUES}。 */
  status: string;
  /** 收件人精确匹配（大小写不敏感）；空串表示不筛选。 */
  recipient: string;
  /** 投递、义务或通知 ID；非规范 UUID 视为未设置。 */
  reference: string;
  /** 冻结模板键；仅允许模板键字符。 */
  template: string;
  /** 发送类别闭集；空串表示全部。 */
  category: string;
  /** `main` / `isekai`；空串表示全部。 */
  world: string;
  /** 创建时间 RFC3339；空串表示不设界。 */
  createdFrom: string;
  createdTo: string;
  /** 共享页码原语保留完整 `i64` 文本；不可请求的页码仍由 API 的 offset 上限拒绝。 */
  page: bigint;
  /** 排序列标识；空串表示保持服务端原始顺序。合法值见 `DELIVERIES_SORT_COLUMNS`。 */
  sort: string;
  direction: SortDirection;
}

/** 映射为 API 查询结构；`status` 由页面单独传入。 */
export function deliveriesQueryFilters(query: DeliveriesQuery): DeliverySearchFilters {
  return {
    recipient: query.recipient,
    reference: query.reference,
    template: query.template,
    category: query.category,
    world: query.world,
    createdFrom: query.createdFrom,
    createdTo: query.createdTo,
  };
}

/** 模板键只允许小写字母、数字、下划线与连字符，最长 64 字符。 */
function parseTemplateText(pairs: readonly QueryPair[]): string {
  const value = parseText(pairs, "template", 64);
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (
      !(
        (code >= 0x61 && code <= 0x7a) ||
        (code >= 0x30 && code <= 0x39) ||
        code === 0x5f ||
        code === 0x2d
      )
    ) {
      return "";
    }
  }
  return value;
}

/** 只接受可解析的 RFC3339；非法值按未设置处理。 */
function parseTimestampText(pairs: readonly QueryPair[], key: string): string {
  const value = parseText(pairs, key, 40);
  return parseStrictRfc3339(value) === null ? "" : value;
}

/** Delivery 列表的筛选、页码与排序；从地址栏读取并写回。 */
export function useDeliveriesQueryState(): QueryState<DeliveriesQuery> {
  return useQueryState(
    DELIVERIES_KEYS,
    (pairs) => ({
      status: parseChoice(pairs, "status", DELIVERY_STATUS_VALUES),
      recipient: parseText(pairs, "recipient", 256),
      reference: parseUuidText(pairs, "reference"),
      template: parseTemplateText(pairs),
      category: parseChoice(pairs, "category", DELIVERY_CATEGORY_VALUES),
      world: parseChoice(pairs, "world", ["main", "isekai"]),
      createdFrom: parseTimestampText(pairs, "from"),
      createdTo: parseTimestampText(pairs, "to"),
      page: parsePage(pairs, "page"),
      sort: parseSortColumn(pairs, DELIVERIES_SORT_COLUMNS),
      direction: parseSortDirection(pairs),
    }),
    (query) => {
      const pairs: QueryPair[] = [];
      pushText(pairs, "status", query.status);
      pushText(pairs, "recipient", query.recipient);
      pushText(pairs, "reference", query.reference);
      pushText(pairs, "template", query.template);
      pushText(pairs, "category", query.category);
      pushText(pairs, "world", query.world);
      pushText(pairs, "from", query.createdFrom);
      pushText(pairs, "to", query.createdTo);
      pushPage(pairs, "page", query.page);
      pushSort(pairs, query.sort, query.direction);
      return pairs;
    },
    { sameAddress: "write" },
  );
}
