// lot 路由（目录、详情、竞价与代理授权命令）的对外解码入口。
// 字段闭集与跨字段谓词按职责放在同目录子模块；代理授权的配额与转换历史谓词由 account 模块共享，
// 两处投影必须保持同一条严格边界。

import {
  type PlaceBidResultResponse as BidResult,
  PlaceBidResponseSchema,
  type PublicProxyCommandAuthorizationResponse as ProxyCommandAuthorization,
  PublicProxyCommandResponseSchema,
} from "@auction/contracts/dto/public/account";
import {
  type PublicLotEnvelopeResponse as LotEnvelope,
  PublicLotEnvelopeResponseSchema,
} from "@auction/contracts/dto/public/catalog/lot";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { err, ok, type Result } from "@auction/web-shared/result";

import type { CatalogLevelFilter } from "../../model";
import { decodeSuccess, object, string } from "..";
import { validBidCommand, validProxyCommand } from "./command-results";
import { validLotDetail, validLotList } from "./list";

/** 解码 lot 目录页，并绑定请求的场次、分类、分组与等级筛选。 */
export function decodeLotList(
  response: TransportResponse,
  auctionId: string,
  category: string | null,
  equipmentGroup: string | null,
  levels: CatalogLevelFilter,
): Result<LotEnvelope, string> {
  return decodeSuccess(response, PublicLotEnvelopeResponseSchema, (value) =>
    validLotList(value, auctionId, category, equipmentGroup, levels),
  );
}

/** 解码单个 lot 详情，并绑定请求的场次与 lot。 */
export function decodeLotDetail(
  response: TransportResponse,
  auctionId: string | null,
  lotId: string,
): Result<LotEnvelope, string> {
  return decodeSuccess(response, PublicLotEnvelopeResponseSchema, (value) =>
    validLotDetail(value, auctionId, lotId),
  );
}

/** 先完成严格解码，再核对命令目标；错 lot 的成功响应仍属于未知结果。 */
export function decodeBidCommand(
  response: TransportResponse,
  lotId: string,
): Result<BidResult, string> {
  const decoded = decodeSuccess(response, PlaceBidResponseSchema, validBidCommand);
  if (!decoded.ok) return decoded;
  return decoded.value.bid.lot_id === lotId ? ok(decoded.value.bid) : err("invalid_response");
}

/** 解码新建代理授权命令，并绑定请求 lot、上限与模式。 */
export function decodeCreateProxyCommand(
  response: TransportResponse,
  lotId: string,
  maximumAmount: number,
  mode: string,
): Result<ProxyCommandAuthorization, string> {
  const decoded = decodeSuccess(response, PublicProxyCommandResponseSchema, (value) =>
    validProxyCommand(value, lotId, maximumAmount, mode, false),
  );
  return decoded.ok ? ok(decoded.value.authorization) : decoded;
}

/** 解码降低代理上限命令，并绑定 lot、上限与授权编号。 */
export function decodeLowerProxyCommand(
  response: TransportResponse,
  lotId: string,
  authorizationId: string,
  maximumAmount: number,
): Result<ProxyCommandAuthorization, string> {
  const decoded = decodeSuccess(response, PublicProxyCommandResponseSchema, (value) => {
    if (!validProxyCommand(value, lotId, maximumAmount, null, true)) return false;
    const envelope = object(value);
    const authorization = envelope === null ? null : object(envelope.authorization);
    return authorization !== null && string(authorization.authorization_id) === authorizationId;
  });
  return decoded.ok ? ok(decoded.value.authorization) : decoded;
}
