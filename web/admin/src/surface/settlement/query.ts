// Admin 结算报表筛选的 URL 状态与浏览器历史同步。
// 不迁到统一的 `useQueryState`：本页 URL 状态拆成四个独立状态
// （world/auction/sequence/page），popstate 经 `useSettlementLocationRevision` 的 revision
// 驱动页面 effect 一次性恢复，并同时复位详情上下文（`selectionGeneration` 防止 recovery
// 确认的异步回写落到已切换的选择上，见 recovery 提交回调里的同代比较）；目录加载后还要
// 按服务端事实 canonical 化地址（auction 与 sequence 必须成对、空目录页钳制到末页、解析失败
// 保留地址显式报错）。这些语义超出通用 hook 的单一状态模型，因此保留本模块的
// read + push/replace + location revision 三件套，底层地址栏读写与 popstate 监听复用
// `@auction/web-shared/net/query` 的 `readQueryPairs` / `writeQueryPairs` / `usePopstateRevision`。

import {
  type QueryPair,
  readQueryPairs,
  usePopstateRevision,
  writeQueryPairs,
} from "@auction/web-shared/net/query";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { err, ok, type Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import { MAX_JSON_SAFE_INTEGER } from "../../api/validation";

/** 结算页地址栏的筛选与目录页码。 */
export interface SettlementQuery {
  worldId: string;
  auctionId: string;
  sequence: number | null;
  page: number;
}

/** 与 Rust `Default` 相同：全部世界、未选场次、第一页。 */
export function settlementQueryDefault(): SettlementQuery {
  return { worldId: "all", auctionId: "", sequence: null, page: 1 };
}

/** 本页在地址栏管理的全部 query 键；写回时只替换这些键，其余键原样保留。 */
const SETTLEMENT_QUERY_KEYS: readonly string[] = ["world", "auction", "sequence", "page"];

export const SETTLEMENT_PAGE_SIZE = "page_size";

/** 显式提供的筛选或场次标识无法解析，不能按缺省查询选中其他场次。 */
export type InvalidSettlementLink = "invalid_settlement_link";

const INVALID_SETTLEMENT_LINK: InvalidSettlementLink = "invalid_settlement_link";
const I64_MAX_TEXT = "9223372036854775807";
const MAX_JSON_SAFE_INTEGER_BIGINT = BigInt(MAX_JSON_SAFE_INTEGER);

/** 读取并校验当前地址栏里的结算筛选；无效链接由调用方保留原文并显示错误。 */
export function readSettlementQuery(): Result<SettlementQuery, InvalidSettlementLink> {
  return parseSettlementPairs(readQueryPairs());
}

function parseSettlementPairs(
  pairs: readonly QueryPair[],
): Result<SettlementQuery, InvalidSettlementLink> {
  const query = settlementQueryDefault();
  const seen = new Set<string>();
  for (const [key, value] of pairs) {
    if (key !== SETTLEMENT_PAGE_SIZE && !SETTLEMENT_QUERY_KEYS.includes(key)) {
      continue;
    }
    if (seen.has(key)) {
      return err(INVALID_SETTLEMENT_LINK);
    }
    seen.add(key);
    switch (key) {
      case "world":
        if (value !== "all" && value !== "main" && value !== "isekai") {
          return err(INVALID_SETTLEMENT_LINK);
        }
        query.worldId = value;
        break;
      case "auction":
        if (!validUuid(value)) {
          return err(INVALID_SETTLEMENT_LINK);
        }
        query.auctionId = value;
        break;
      case "sequence": {
        const sequence = positiveInteger(value);
        if (sequence === null || sequence > MAX_JSON_SAFE_INTEGER_BIGINT) {
          return err(INVALID_SETTLEMENT_LINK);
        }
        query.sequence = Number(sequence);
        break;
      }
      case "page": {
        const page = positiveInteger(value);
        if (page === null) {
          return err(INVALID_SETTLEMENT_LINK);
        }
        query.page = Number(page);
        break;
      }
      // 先拦截重复值，避免分页组件挂载时按首值改写无效地址。单值仍由分页组件解析。
      case SETTLEMENT_PAGE_SIZE:
        break;
      default:
        return err(INVALID_SETTLEMENT_LINK);
    }
  }
  // 按期号定位只兼容未提供 UUID 的旧链接；给出 UUID 时必须具备可核对的期号。
  if (query.auctionId !== "" && query.sequence === null) {
    return err(INVALID_SETTLEMENT_LINK);
  }
  return ok(query);
}

/** 只接受纯 ASCII 十进制数字且落在 `i64` 正数范围内。先按字符与上界判定，避免任意精度构造。 */
function positiveInteger(value: string): bigint | null {
  if (value.length === 0) {
    return null;
  }
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return null;
    }
  }
  // Rust 的整数解析接受前导零；先去掉它们再按十进制位数和 i64 上界判定。
  let firstSignificant = 0;
  while (firstSignificant < value.length && value.charCodeAt(firstSignificant) === 0x30) {
    firstSignificant += 1;
  }
  if (firstSignificant === value.length) {
    return null;
  }
  const significant = value.slice(firstSignificant);
  if (significant.length > I64_MAX_TEXT.length) {
    return null;
  }
  if (significant.length === I64_MAX_TEXT.length && compareText(significant, I64_MAX_TEXT) > 0) {
    return null;
  }
  return BigInt(significant);
}

/** 以 replace 写回筛选；地址已变成无效链接时保留原文。 */
export function replaceSettlementQuery(query: SettlementQuery): void {
  // 位置已改变时，在旧目录响应写回前再次检查，保留无效链接原文。
  if (!readSettlementQuery().ok) {
    return;
  }
  writeQueryPairs(SETTLEMENT_QUERY_KEYS, settlementQueryPairs(query), "replace");
}

/** 用户主动切换筛选或分页时创建可返回的浏览器历史项。 */
export function pushSettlementQuery(query: SettlementQuery): void {
  // Rust 在这里也会对相同地址 push；共享入口一次写入完整目标地址（含片段），
  // 新条目继承等待中的片段落点，随后由分页器 replace 数量。
  writeQueryPairs(SETTLEMENT_QUERY_KEYS, settlementQueryPairs(query), "push", {
    sameAddress: "write",
  });
}

/** 序列化规则与旧版一致：默认值键省略，auction 与 sequence 成对写入。 */
function settlementQueryPairs(query: SettlementQuery): readonly QueryPair[] {
  const pairs: QueryPair[] = [];
  if (query.worldId !== "all") {
    pairs.push(["world", query.worldId]);
  }
  if (query.sequence !== null) {
    pairs.push(["sequence", query.sequence.toString()]);
  }
  if (query.auctionId !== "") {
    pairs.push(["auction", query.auctionId]);
  }
  if (query.page > 1) {
    pairs.push(["page", query.page.toString()]);
  }
  return pairs;
}

/** 监听同一路径的浏览器返回/前进，确保筛选状态不会与地址栏分离。 */
export function useSettlementLocationRevision(): Cell<number> {
  const revision = createCell(0);
  usePopstateRevision(revision);
  return revision;
}
