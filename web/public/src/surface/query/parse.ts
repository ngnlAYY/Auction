// 地址栏 query 的解析：按键分派、值规范化与非法值处理；未声明的键原样透传。
// 只在浏览器中读取，宿主构建不参与这里的地址栏状态。

import { parseQuerySearch } from "@auction/web-shared/net/query";
import { utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import { DEFAULT_PAGE_SIZE, parsePublicPageSize } from "../pagination";
import { normalizeWorldQuery, type PublicBoardQuery, publicBoardQueryDefault } from "./board";
import { normalizeCatalogCategories } from "./category";
import { rustTrim } from "./text";

const INT64_MAX = 9_223_372_036_854_775_807n;

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

/** 解析 i64 十进制；不能用 `Number`，否则会接受十六进制、指数与空白。 */
function parsePositivePage(value: string): number | null {
  if (!/^[+-]?[0-9]+$/u.test(value)) {
    return null;
  }
  const parsed = BigInt(value);
  if (parsed <= 0n || parsed > INT64_MAX) {
    return null;
  }
  // 已知差异：页面状态用 JS number 保存，超过 2^53−1 的合法 i64 页码无法精确表示，
  // 这里按非法值处理并回到默认页；请求与 URL 契约仍以 Rust 的 i64 范围为准。
  if (parsed > BigInt(Number.MAX_SAFE_INTEGER)) {
    return null;
  }
  return Number(parsed);
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

export function parsePublicQuery(pathname: string, search: string): PublicBoardQuery {
  const query = publicBoardQueryDefault();
  const params = parseQuerySearch(search.startsWith("?") ? search : "");
  const auctions = params.getAll("auction");
  const [auction] = auctions;
  if (auction !== undefined) {
    // 多值地址没有唯一目标；保留每个原值，让看板失败关闭。
    if (auctions.length === 1 && validUuid(auction)) {
      query.auctionId = auction;
    } else {
      query.invalidAuction = auctions;
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
  let canonicalPage: number | null = null;
  let legacyBidLogPage: number | null = null;
  for (const [key, value] of params) {
    switch (key) {
      case "world":
        query.world = normalizeWorldQuery(value);
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
        canonicalPage = parsePositivePage(value) ?? canonicalPage;
        break;
      case "page_size":
        query.pageSize = parsePublicPageSize(value) ?? DEFAULT_PAGE_SIZE;
        break;
      case "movement_page": {
        const page = parsePositivePage(value);
        if (page !== null) {
          query.movementPage = page;
        }
        break;
      }
      case "movement_page_size":
        query.movementPageSize = parsePublicPageSize(value) ?? DEFAULT_PAGE_SIZE;
        break;
      case "bidlog_page": {
        const page = parsePositivePage(value);
        if (page !== null) {
          legacyBidLogPage = page;
        }
        break;
      }
      case "bid_page": {
        const page = parsePositivePage(value);
        if (page !== null) {
          query.bidPage = page;
        }
        break;
      }
      case "proxy_page": {
        const page = parsePositivePage(value);
        if (page !== null) {
          query.proxyPage = page;
        }
        break;
      }
      default:
        query.passthrough.push([key, value]);
    }
  }
  query.page =
    canonicalPage ?? (pathname === "/bidlog" && legacyBidLogPage !== null ? legacyBidLogPage : 1);
  query.bidLogPageAlias = pathname === "/bidlog" && legacyBidLogPage !== null;
  return query;
}
