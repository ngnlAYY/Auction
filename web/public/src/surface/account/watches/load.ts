//! 组合已有只读 API，完整求出一个世界当前公开目录与账号标记的交集。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import type { PublicLotWatchesResponse } from "@auction/contracts/dto/public/watch";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { err, ok, type Result } from "@auction/web-shared/result";

import {
  type CatalogRow,
  catalogLevelFilterDefault,
  lotEnvelopeTakeRows,
} from "../../../api/model";
import { fetchCurrentAuction } from "../../../api/queries/current";
import { fetchCurrentLots } from "../../../api/queries/lots";
import { startPublicSessionIdentityObservation } from "../../../api/session/revision";
import { fetchLotWatches } from "../../../api/watches";

/** 当前场次及其完整公开标记结果；总数由真实匹配行数产生。 */
export interface WatchedAuction {
  readonly auction: CurrentAuctionItemResponse;
  readonly rows: readonly CatalogRow[];
}

/** 按 Public 页长上限逐页读取整份目录。仅供测试使用。 */
export const PAGE_SIZE = MAX_PUBLIC_PAGE_SIZE;
/** 既有目录 API 允许 offset <= 10000；末页从 10000 起，仍以一页为界。仅供测试使用。 */
export const MAX_CATALOG_ROWS = 10_000 + PAGE_SIZE;

/** 按 [`PAGE_SIZE`] 逐页读取时，`total` 行目录第 `pageNumber` 页应有的行数，以及读完本页后是否
 * 已读完整份目录；页码非正、总数为负或超出 offset 上限可达的行数时为 `null`。
 * 仅供测试使用。 */
export function catalogPageShape(
  pageNumber: number,
  total: number,
): readonly [number, boolean] | null {
  if (
    !Number.isSafeInteger(pageNumber) ||
    pageNumber < 1 ||
    !Number.isSafeInteger(total) ||
    total < 0 ||
    total > MAX_CATALOG_ROWS
  ) {
    return null;
  }
  const offset = (pageNumber - 1) * PAGE_SIZE;
  if (!Number.isSafeInteger(offset)) {
    return null;
  }
  return [Math.min(Math.max(total - offset, 0), PAGE_SIZE), offset >= total - PAGE_SIZE];
}

/** 读取一个固定账号与世界的完整公开标记，返回前复核当前场次和集合版本。 */
export async function load(
  userId: string,
  world: string,
  signal: AbortSignal,
): Promise<Result<WatchedAuction | null, null>> {
  const observation = startPublicSessionIdentityObservation();
  const auctionResult = await fetchCurrentAuction(world, signal);
  if (!auctionResult.ok) {
    return err(null);
  }
  const auction = auctionResult.value;
  if (auction === null) {
    return ok(null);
  }
  const watchesResult = await fetchLotWatches(userId, auction.id, signal, observation);
  if (!watchesResult.ok) {
    return err(null);
  }
  const expected = watchVersions(watchesResult.value);
  const watched = new Set(
    watchesResult.value.watches.filter((watch) => watch.watched).map((watch) => watch.lot_id),
  );
  // 未标记时不读取任何装备目录，场次和账号集合仍在返回前重新核对。
  const rowsResult =
    watched.size === 0
      ? ok<readonly CatalogRow[]>([])
      : await loadCatalog(auction, watched, signal);
  if (!rowsResult.ok) {
    return err(null);
  }
  const [currentResult, currentWatchesResult] = await Promise.all([
    fetchCurrentAuction(world, signal),
    fetchLotWatches(userId, auction.id, signal, observation),
  ]);
  if (!currentResult.ok || !currentWatchesResult.ok) {
    return err(null);
  }
  const current = currentResult.value;
  if (
    current === null ||
    !structuralEquals(current, auction) ||
    !structuralEquals(watchVersions(currentWatchesResult.value), expected)
  ) {
    return err(null);
  }
  if (!observation.current()) return err(null);
  return ok({ auction, rows: rowsResult.value });
}

function watchVersions(
  watches: PublicLotWatchesResponse,
): ReadonlyMap<string, readonly [boolean, number]> {
  return new Map(
    watches.watches.map(
      (watch) => [watch.lot_id, [watch.watched, watch.version] as const] as const,
    ),
  );
}

async function loadCatalog(
  auction: CurrentAuctionItemResponse,
  watched: ReadonlySet<string>,
  signal: AbortSignal,
): Promise<Result<readonly CatalogRow[], null>> {
  let pageNumber = 1;
  let expectedTotal: number | null = null;
  const seen = new Set<string>();
  const result: CatalogRow[] = [];
  for (;;) {
    const envelopeResult = await fetchCurrentLots(
      auction.id,
      null,
      null,
      catalogLevelFilterDefault(),
      pageNumber,
      PAGE_SIZE,
      signal,
    );
    if (!envelopeResult.ok) {
      return err(null);
    }
    const envelope = envelopeResult.value;
    const total = envelope.total;
    if (total === null) {
      return err(null);
    }
    const shape = catalogPageShape(pageNumber, total);
    if (shape === null) {
      return err(null);
    }
    const [expectedRows, lastPage] = shape;
    if (
      envelope.auction_id !== auction.id ||
      envelope.auction_status !== auction.status ||
      envelope.preview_revision !== auction.preview_revision ||
      envelope.presentation_mode !== auction.presentation_mode ||
      envelope.bidding_open !== auction.bidding_open ||
      envelope.opens_at !== auction.opens_at ||
      envelope.page !== pageNumber ||
      envelope.page_size !== PAGE_SIZE ||
      (expectedTotal !== null && expectedTotal !== total)
    ) {
      return err(null);
    }
    expectedTotal = total;
    const rows = lotEnvelopeTakeRows(envelope);
    if (rows.length !== expectedRows) {
      return err(null);
    }
    for (const row of rows) {
      if (seen.has(row.lotId)) {
        return err(null);
      }
      seen.add(row.lotId);
      if (watched.has(row.lotId)) {
        result.push(row);
      }
    }
    if (lastPage) {
      // 撤回等非公开标的沿用目录可见性，不从旧缓存补名称或伪造详情。
      return ok(result);
    }
    if (pageNumber === Number.MAX_SAFE_INTEGER) {
      return err(null);
    }
    pageNumber += 1;
  }
}
