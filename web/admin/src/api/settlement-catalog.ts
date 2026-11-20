// Admin 结算报表选择目录客户端。

import {
  type AdminSettlementCatalogResponse,
  AdminSettlementCatalogResponseSchema,
} from "@auction/contracts/dto/admin/settlement-catalog";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import { decodeAdminReceipt, invalidResponseError, responseError } from "./failure";
import { validate, validPageEcho } from "./pagination";
import { type AdminSessionHost, adminSessionHostLive } from "./session";
import { parseStrictRfc3339 } from "./session-time";
import { sendGet } from "./transport";
import { MAX_JSON_SAFE_INTEGER } from "./validation";

/** contracts DTO 拒绝未知字段；此处补充排序、去重与筛选回显等跨字段校验。 */

export interface AdminSettlementCatalogEntry {
  readonly auctionId: string;
  readonly worldId: string;
  readonly sequence: number;
  readonly settledAt: string;
}

export interface AdminSettlementCatalogPage {
  readonly auctions: readonly AdminSettlementCatalogEntry[];
  readonly page: number;
  readonly pageSize: number;
  readonly total: number;
}

/** 目录条目决定选中哪一条场次；页码、页大小与期号都按 serde 的整数 visitor 拒绝小数词法，
 * 任意层的重复键（例如重复的 `auction_id`）也必须在解码前拒绝。 */
const ADMIN_SETTLEMENT_CATALOG_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

/** 读取已结算拍卖目录；`world_id = all` 只存在于页面状态，不进入服务端查询。 */
export async function fetchAdminSettlementCatalog(
  host: AdminSessionHost,
  worldId: string,
  sequence: number | null,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminSettlementCatalogPage, string>> {
  const bounds = validate(page, pageSize, 100, 100_000);
  if (!bounds.ok) {
    return bounds;
  }
  const query = new URLSearchParams();
  query.append("page", page.toString());
  query.append("page_size", pageSize.toString());
  if (worldId !== "all") {
    query.append("world_id", worldId);
  }
  if (sequence !== null) {
    query.append("sequence", sequence.toString());
  }
  const url = `/api/v1/admin/settlement-catalog?${query.toString()}`;
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(response.error);
  }
  // 传输层最后一次存活检查与本次恢复之间仍可能取消或注销；不再消费缓冲的成功响应。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err("network_error");
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminSettlementCatalogResponseSchema),
    ADMIN_SETTLEMENT_CATALOG_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  if (payload.value.code !== "ok" || trimmedEmptyRust(payload.value.message)) {
    return err(invalidResponseError("settlement_catalog_envelope"));
  }
  const decoded = decodeCatalog(payload.value.data, worldId, sequence, page, pageSize);
  if (!decoded.ok) {
    return err(invalidResponseError(decoded.error));
  }
  return ok(decoded.value);
}

function decodeCatalog(
  data: AdminSettlementCatalogResponse,
  worldId: string,
  sequence: number | null,
  requestedPage: number,
  requestedSize: number,
): Result<AdminSettlementCatalogPage, string> {
  const pagination = data.pagination;
  // 目录页大小上限 100 是本端点的特例，其余回显与条数约束与其他列表一致。
  if (
    pagination.page_size > 100 ||
    !validPageEcho(
      requestedPage,
      requestedSize,
      pagination.page,
      pagination.page_size,
      pagination.total,
      data.auctions.length,
    )
  ) {
    return err("settlement_catalog_pagination");
  }

  const auctionIds = new Set<string>();
  const periods = new Set<string>();
  let previousKey: CatalogOrderKey | null = null;
  // 校验阶段先验证完整响应的顺序键，成功后再转移字段。
  for (const entry of data.auctions) {
    // 后端按微秒排序，只有时刻真正相等才使用后续排序键。
    const settledAt = parseStrictRfc3339(entry.settled_at);
    if (settledAt === null) {
      return err("settlement_catalog_time");
    }
    const periodKey = `${entry.world_id}\u0000${entry.sequence}`;
    if (
      !validUuid(entry.auction_id) ||
      (entry.world_id !== "main" && entry.world_id !== "isekai") ||
      entry.sequence <= 0 ||
      entry.sequence > MAX_JSON_SAFE_INTEGER ||
      (worldId !== "all" && entry.world_id !== worldId) ||
      (sequence !== null && entry.sequence !== sequence) ||
      auctionIds.has(entry.auction_id) ||
      periods.has(periodKey)
    ) {
      return err("settlement_catalog_entry");
    }
    auctionIds.add(entry.auction_id);
    periods.add(periodKey);
    const key: CatalogOrderKey = {
      settledAt,
      worldId: entry.world_id,
      sequence: entry.sequence,
      auctionId: entry.auction_id,
    };
    if (previousKey !== null && !catalogOrderValid(previousKey, key, worldId !== "all")) {
      return err("settlement_catalog_order");
    }
    previousKey = key;
  }
  const auctions: AdminSettlementCatalogEntry[] = data.auctions.map((entry) => ({
    auctionId: entry.auction_id,
    worldId: entry.world_id,
    sequence: entry.sequence,
    settledAt: entry.settled_at,
  }));

  return ok({
    auctions,
    page: pagination.page,
    pageSize: pagination.page_size,
    total: pagination.total,
  });
}

interface CatalogOrderKey {
  readonly settledAt: bigint;
  readonly worldId: string;
  readonly sequence: number;
  readonly auctionId: string;
}

function catalogOrderValid(
  previous: CatalogOrderKey,
  current: CatalogOrderKey,
  singleWorld: boolean,
): boolean {
  if (singleWorld && previous.sequence !== current.sequence) {
    return previous.sequence > current.sequence;
  }
  if (previous.settledAt !== current.settledAt) {
    return previous.settledAt > current.settledAt;
  }
  if (!singleWorld && previous.worldId !== current.worldId) {
    return compareText(previous.worldId, current.worldId) < 0;
  }
  if (previous.sequence !== current.sequence) {
    return previous.sequence > current.sequence;
  }
  return compareText(previous.auctionId, current.auctionId) >= 0;
}

/** Rust `str::trim` 是否会把整段文本去除；JS `trim` 会额外去掉 U+FEFF，不能替代。 */
function trimmedEmptyRust(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    if (!isRustWhitespace(value.charCodeAt(index))) {
      return false;
    }
  }
  return true;
}

function isRustWhitespace(code: number): boolean {
  return (
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
