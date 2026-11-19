// 地址栏 query 的解析：按键分派、值规范化与非法值处理；未声明的键原样透传。
// 只在浏览器中读取，宿主构建不参与这里的地址栏状态。

import { parseQuerySearch } from "@auction/web-shared/net/query";
import { utf8ByteLength } from "@auction/web-shared/text";

import { MAX_FULFILLMENT_PAGE_SIZE } from "../../api/fulfillment";
import { DEFAULT_PAGE_SIZE, parsePublicPageSize } from "../pagination";
import { publicOffsetLastPage, readPublicPageParameters } from "../pagination/parameters";
import { parseAuctionParameter } from "./auction";
import { normalizeWorldQuery, type PublicBoardQuery, publicBoardQueryDefault } from "./board";
import { normalizeCatalogCategories } from "./category";
import { rustTrim } from "./text";

/** Rust `char::is_control` 对应 Unicode Cc：C0、DEL 与 C1。 */
function hasUnicodeControl(value: string): boolean {
  for (const character of value) {
    const codePoint = character.codePointAt(0) ?? 0;
    if (codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f)) {
      return true;
    }
  }
  return false;
}

/** 只交给页面可表示的有界页码；超限的原值仍由分页门禁从同一地址取值并写回。 */
function boundedPage(page: bigint | null, maximum: number): number {
  if (page === null) return 1;
  return page > BigInt(maximum) ? maximum : Number(page);
}

function isHistoryStatus(value: string): boolean {
  return value === "live" || value === "final" || value === "settled";
}

function normalizeOptionalQuery(value: string, maximum: number): string | null {
  const trimmed = rustTrim(value);
  if (trimmed === "" || utf8ByteLength(trimmed) > maximum || hasUnicodeControl(trimmed)) {
    return null;
  }
  return trimmed;
}

/** lot 与 name 由页面校验，这里只去空白：非法值留给页面报字段错误且不发请求。 */
function retainedQueryValue(value: string): string | null {
  const trimmed = rustTrim(value);
  return trimmed === "" ? null : trimmed;
}

/** 解析已匹配页面的查询；页面名由路由提供，原始地址只用于取参数。 */
export function parsePublicQuery(routeName: string | null, search: string): PublicBoardQuery {
  const query = publicBoardQueryDefault();
  const params = parseQuerySearch(search.startsWith("?") ? search : "");
  query.auction = parseAuctionParameter(params.getAll("auction"));
  const roundPage = routeName === "BidLog" || routeName === "Fulfillment";
  if (roundPage) {
    const worlds = params.getAll("world");
    const [world] = worlds;
    if (worlds.length === 1 && (world === "main" || world === "isekai")) {
      query.world = world;
    } else if (worlds.length > 0) {
      query.invalidWorld = worlds;
    }
  }
  const types = params.getAll("type");
  const [type] = types;
  if (type !== undefined) {
    query.category = types.length === 1 ? normalizeCatalogCategories(type) : null;
    if (query.category === null) {
      query.invalidCategory = types;
    }
  }
  for (const [key, value] of params) {
    switch (key) {
      case "world":
        if (!roundPage) query.world = normalizeWorldQuery(value);
        break;
      case "auction":
        break;
      case "type":
        break;
      case "equipment_group":
        query.equipmentGroup = value === "rare" || value === "fusion" ? value : null;
        break;
      case "max_level":
        query.maxLevel = value;
        break;
      case "include_unleveled":
        query.includeUnleveled = value;
        break;
      case "status":
        if (isHistoryStatus(value)) {
          query.status = value;
        } else {
          query.passthrough.push([key, value]);
        }
        break;
      case "name":
        query.historyName = retainedQueryValue(value);
        break;
      case "from":
        query.historyFrom = normalizeOptionalQuery(value, 10);
        break;
      case "to":
        query.historyTo = normalizeOptionalQuery(value, 10);
        break;
      case "min_price":
        query.historyMinPrice = value === "" ? null : value;
        break;
      case "max_price":
        query.historyMaxPrice = value === "" ? null : value;
        break;
      case "buyer":
        query.historyBuyer = normalizeOptionalQuery(value, 256);
        break;
      case "seller":
        query.historySeller = normalizeOptionalQuery(value, 256);
        break;
      case "history_sort":
        if (
          value === "closed_desc" ||
          value === "closed_asc" ||
          value === "price_desc" ||
          value === "price_asc"
        ) {
          query.historySort = value;
        }
        break;
      case "history_group":
        if (value === "flat" || value === "equipment" || value === "buyer" || value === "seller") {
          query.historyGroup = value;
        }
        break;
      case "lot":
        query.bidLogLot = retainedQueryValue(value);
        break;
      case "page":
      case "movement_page":
      case "bidlog_page":
      case "bid_page":
      case "proxy_page":
        break;
      case "page_size":
        query.pageSize = parsePublicPageSize(value) ?? DEFAULT_PAGE_SIZE;
        break;
      case "movement_page_size":
        query.movementPageSize = parsePublicPageSize(value) ?? DEFAULT_PAGE_SIZE;
        break;
      default:
        query.passthrough.push([key, value]);
    }
  }
  const blacklist = routeName === "Blacklist";
  const pages = readPublicPageParameters(params, routeName === "BidLog", blacklist);
  const size =
    routeName === "Fulfillment"
      ? Math.min(query.pageSize, MAX_FULFILLMENT_PAGE_SIZE)
      : query.pageSize;
  // 黑名单沿用原来的安全整数筛选及分页边界，不参加拍卖列表的偏移纠偏。
  const maximum = (size: number) =>
    blacklist ? Number.MAX_SAFE_INTEGER : publicOffsetLastPage(size);
  query.page = boundedPage(pages.page, maximum(size));
  query.movementPage = boundedPage(pages.movementPage, maximum(query.movementPageSize));
  query.bidPage = boundedPage(pages.bidPage, maximum(10));
  query.proxyPage = boundedPage(pages.proxyPage, maximum(10));
  query.bidLogPageAlias = pages.bidLogPageAlias;
  return query;
}
