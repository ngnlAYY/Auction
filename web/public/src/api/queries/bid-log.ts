// 公开竞价日志读取，以及竞价日志与履约进度共用的按状态场次目录遍历。

import type { AuctionSummaryResponse } from "@auction/contracts/dto/public/catalog/auction";
import type { PublicBidLogListResponse } from "@auction/contracts/dto/public/catalog/history";
import { err, ok, type Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { decodeBidLogPage } from "../decoder/catalog";
import { LIVE_TTL_MILLIS } from "../swr-cache";
import { cachedPublicGet } from "./cached-get";
import { fetchAuctionPage, SUMMARY_PAGE_SIZE } from "./directory";

/** 竞价日志的附加字段：代理竞争中落败的授权。缺省响应不含它，旧页面按原字段闭集解码。 */
const BID_LOG_INCLUDE = "defeated_proxy";

/** 竞价日志页的缓存投影标识；同一 URL 的其他查询投影不得命中该条目。 */
const BID_LOG_CACHE_TYPE = "public_bid_log_page";

/** 读取单个场次（可限定 lot）的编号竞价日志页。 */
export async function fetchBidLogPage(
  auctionId: string,
  page: number,
  worldId: string,
  lotId: string | null,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<PublicBidLogListResponse, string>> {
  const query = new URLSearchParams();
  query.append("page", String(Math.max(1, page)));
  query.append("page_size", String(pageSize));
  query.append("include", BID_LOG_INCLUDE);
  if (worldId !== "") {
    query.append("world", worldId);
  }
  if (lotId !== null) {
    query.append("lot", lotId);
  }
  const url = `/api/v1/auctions/${auctionId}/bid-log?${query.toString()}`;
  const read = await cachedPublicGet(
    url,
    LIVE_TTL_MILLIS,
    BID_LOG_CACHE_TYPE,
    (response) => decodeBidLogPage(response),
    signal,
  );
  return signal.aborted ? err("network_error") : read;
}

/** 读取可供公开竞价日志选择的 live、final 和 settled 拍卖，保留已结算场次的历史深链。 */
export async function fetchBidLogAuctions(
  worldId: string,
  signal: AbortSignal,
): Promise<Result<AuctionSummaryResponse[], string>> {
  // 状态目录相互独立，最多并发三个读取；首个错误会中止兄弟读取，与源实现的 `try_join!` 一致。
  const controller = new AbortController();
  const abortFromCaller = () => controller.abort();
  if (signal.aborted) {
    controller.abort();
  } else {
    signal.addEventListener("abort", abortFromCaller, { once: true });
  }
  try {
    const joined = await tryJoinThree(
      fetchBidLogAuctionStatus(worldId, "live", controller.signal),
      fetchBidLogAuctionStatus(worldId, "final", controller.signal),
      fetchBidLogAuctionStatus(worldId, "settled", controller.signal),
      () => controller.abort(),
    );
    if (!joined.ok) {
      // 首个失败发生后，其余分支的请求与后续分页都不再有人需要。
      return joined;
    }
    if (signal.aborted) {
      return err("network_error");
    }
    const [liveAuctions, finalAuctions, settledAuctions] = joined.value;
    const auctions = [...liveAuctions, ...finalAuctions, ...settledAuctions];
    auctions.sort((left, right) => {
      if (left.sequence !== right.sequence) {
        return right.sequence < left.sequence ? -1 : 1;
      }
      return compareText(right.id, left.id);
    });
    const deduped: AuctionSummaryResponse[] = [];
    for (const auction of auctions) {
      const previous = deduped[deduped.length - 1];
      if (previous === undefined || previous.id !== auction.id) {
        deduped.push(auction);
      }
    }
    return ok(deduped);
  } finally {
    signal.removeEventListener("abort", abortFromCaller);
  }
}

/** 按编号顺序逐页读完单一状态的公开拍卖目录；竞价日志与履约进度的场次选择共用。 */
export async function fetchBidLogAuctionStatus(
  worldId: string,
  status: string,
  signal: AbortSignal,
): Promise<Result<AuctionSummaryResponse[], string>> {
  const auctions: AuctionSummaryResponse[] = [];
  let pageNumber = 1;
  for (;;) {
    if (signal.aborted) {
      return err("network_error");
    }
    const page = await fetchAuctionPage(worldId, status, pageNumber, SUMMARY_PAGE_SIZE, signal);
    if (signal.aborted) {
      return err("network_error");
    }
    if (!page.ok) {
      return page;
    }
    const rowCount = page.value.auctions.length;
    // Rust 的数组长度转换不可能失败；保留该失败关闭分支。
    if (!Number.isSafeInteger(rowCount)) {
      return err("拍卖目录页过大");
    }
    const pageSize = page.value.page_size;
    if (pageSize <= 0) {
      return err("拍卖目录分页大小无效");
    }
    const total = page.value.total;
    auctions.push(...page.value.auctions);
    // 满页且缺 total 时继续读取，避免选择器静默截断到第一页。
    if (
      rowCount < pageSize ||
      (total !== null && saturatingMultiply(pageNumber, pageSize) >= total)
    ) {
      return ok(auctions);
    }
    const nextPage = pageNumber + 1;
    if (!Number.isSafeInteger(nextPage)) {
      return err("拍卖目录页码溢出");
    }
    pageNumber = nextPage;
  }
}

/** 读取单个 lot 的编号竞价历史；分页参数沿用公开竞价日志契约。 */
export async function fetchLotBidLog(
  auctionId: string,
  lotId: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<PublicBidLogListResponse, string>> {
  const query = new URLSearchParams();
  query.append("lot", lotId);
  query.append("page", String(Math.max(1, page)));
  query.append("page_size", String(pageSize));
  query.append("include", BID_LOG_INCLUDE);
  const url = `/api/v1/auctions/${auctionId}/bid-log?${query.toString()}`;
  const read = await cachedPublicGet(
    url,
    LIVE_TTL_MILLIS,
    BID_LOG_CACHE_TYPE,
    (response) => decodeBidLogPage(response),
    signal,
  );
  return signal.aborted ? err("network_error") : read;
}

/** 三个互不依赖的读取；首个失败结果立即返回，调用方随后中止其余分支。 */
async function tryJoinThree<T1, T2, T3, E>(
  first: Promise<Result<T1, E>>,
  second: Promise<Result<T2, E>>,
  third: Promise<Result<T3, E>>,
  onFailure: () => void,
): Promise<Result<readonly [T1, T2, T3], E>> {
  return new Promise((resolve) => {
    let firstValue: { value: T1 } | undefined;
    let secondValue: { value: T2 } | undefined;
    let thirdValue: { value: T3 } | undefined;
    let remaining = 3;
    let settled = false;
    const fail = (error: E): void => {
      if (settled) {
        return;
      }
      settled = true;
      // 在首个错误落定的同一微任务内中止兄弟分支，避免它们在调用方恢复前继续翻页。
      onFailure();
      resolve(err(error));
    };
    const finish = (): void => {
      if (settled) {
        return;
      }
      if (firstValue === undefined || secondValue === undefined || thirdValue === undefined) {
        return;
      }
      settled = true;
      resolve(ok([firstValue.value, secondValue.value, thirdValue.value]));
    };
    void first.then((result) => {
      if (settled) {
        return;
      }
      if (!result.ok) {
        fail(result.error);
        return;
      }
      firstValue = { value: result.value };
      remaining -= 1;
      if (remaining === 0) {
        finish();
      }
    });
    void second.then((result) => {
      if (settled) {
        return;
      }
      if (!result.ok) {
        fail(result.error);
        return;
      }
      secondValue = { value: result.value };
      remaining -= 1;
      if (remaining === 0) {
        finish();
      }
    });
    void third.then((result) => {
      if (settled) {
        return;
      }
      if (!result.ok) {
        fail(result.error);
        return;
      }
      thirdValue = { value: result.value };
      remaining -= 1;
      if (remaining === 0) {
        finish();
      }
    });
  });
}

function saturatingMultiply(left: number, right: number): number {
  const value = left * right;
  return Number.isSafeInteger(value) ? value : Number.MAX_SAFE_INTEGER;
}
