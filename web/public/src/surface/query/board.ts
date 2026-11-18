// Public 拍卖看板的查询状态、默认值与实时连接状态。

import type {
  AuctionSummaryResponse,
  CurrentAuctionItemResponse,
} from "@auction/contracts/dto/public/catalog/auction";
import type { Result } from "@auction/web-shared/result";

import { storedPublicPageSize } from "../pagination/preferences";
import { type AuctionParameter, auctionParameter } from "./auction";
import { rustTrim } from "./text";

/** 公开拍卖看板的 URL 状态；筛选和分页必须能被刷新、分享和返回操作恢复。 */
export interface PublicBoardQuery {
  world: string | null;
  auction: AuctionParameter;
  category: string | null;
  /** 分类原文只有在玩家修正后才能从地址移除。 */
  invalidCategory: readonly string[] | null;
  equipmentGroup: string | null;
  /** 非法等级参数保留原文供筛选栏修正，不能先丢条件再发出宽查询。 */
  maxLevel: string | null;
  includeUnleveled: string;
  status: string | null;
  historyName: string | null;
  historyFrom: string | null;
  historyTo: string | null;
  historyMinPrice: string | null;
  historyMaxPrice: string | null;
  historyBuyer: string | null;
  historySeller: string | null;
  /** 成交历史独立的全局排序与本页展示方式，不复用目录的 type/status。 */
  historySort: string;
  historyGroup: string;
  bidLogLot: string | null;
  page: number;
  pageSize: number;
  /** 补偿池展开流水的独立分页；批次分页与流水分页不能互相覆盖。 */
  movementPage: number;
  movementPageSize: number;
  /** 是否从迁移期间写出的 `bidlog_page` 别名恢复了竞价日志页码。 */
  bidLogPageAlias: boolean;
  bidPage: number;
  proxyPage: number;
  /** 当前页面不拥有的 query 对必须原样保留，避免筛选或翻页时删除外部入口参数。 */
  passthrough: [string, string][];
}

export function publicBoardQueryDefault(): PublicBoardQuery {
  return {
    world: null,
    auction: auctionParameter(null),
    category: null,
    invalidCategory: null,
    equipmentGroup: null,
    maxLevel: null,
    includeUnleveled: "true",
    status: null,
    historyName: null,
    historyFrom: null,
    historyTo: null,
    historyMinPrice: null,
    historyMaxPrice: null,
    historyBuyer: null,
    historySeller: null,
    historySort: "closed_desc",
    historyGroup: "equipment",
    bidLogLot: null,
    page: 1,
    pageSize: storedPublicPageSize(false),
    movementPage: 1,
    movementPageSize: storedPublicPageSize(true),
    bidLogPageAlias: false,
    bidPage: 1,
    proxyPage: 1,
    passthrough: [],
  };
}

/** 已通过查询校验的场次 UUID，从唯一的场次参数派生。 */
export function queryAuctionId(query: PublicBoardQuery): string | null {
  return query.auction.kind === "selected" ? query.auction.id : null;
}

export function normalizeWorldQuery(value: string): string | null {
  const trimmed = rustTrim(value);
  return trimmed === "main" || trimmed === "isekai" ? trimmed : null;
}

/** 公开端实时连接状态。 */
export type LiveConnectionState = "connecting" | "degraded" | "live" | "manual";

/** 看板预览历史读取；当前拍卖与历史摘要分开，失败沿用页面可本地化的字符串。 */
export type PreviewHistoryLoad = Result<
  readonly [CurrentAuctionItemResponse | null, readonly AuctionSummaryResponse[]] | null,
  string
>;
