// Public 补偿池 HTTP DTO 与请求。

import type {
  PublicCompensationLotPageResponse,
  PublicCompensationLotResponse,
  PublicCompensationResponse,
  PublicCompensationRoundResponse,
} from "@auction/contracts/dto/public/catalog/compensation";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import { err, type Result } from "@auction/web-shared/result";
import { sendGet } from "./clock";
import { decodeErrorCode } from "./decoder";
import { decodeCompensationLots, decodeCompensationPool } from "./decoder/catalog";

/** 单件补偿池拍品。 */
export type CompensationLot = PublicCompensationLotResponse;
/** 补偿池拍品的一页。 */
export type CompensationLotPage = PublicCompensationLotPageResponse;
/** 补偿池页面的元数据。 */
export type CompensationPoolPage = PublicCompensationResponse;
/** 补偿池的轮次投影。 */
export type CompensationRound = PublicCompensationRoundResponse;

/** 读取公开补偿池的一页。 */
export async function fetchCompensationPool(
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<PublicCompensationResponse, string>> {
  const normalizedPage = Math.max(1, page);
  const url = `/api/v1/compensation-pool?page=${normalizedPage}&page_size=${pageSize}`;
  const sent = await sendGet(url, signal);
  if (!sent.ok) {
    return err(transportErrorCode(sent.error));
  }
  if (signal.aborted) {
    return err("network_error");
  }
  const response = sent.value;
  if (!responseOk(response)) {
    return err(decodeErrorCode(response));
  }
  return decodeCompensationPool(response);
}

/** 读取补偿池中指定场次的拍品页。 */
export async function fetchCompensationLots(
  auctionId: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<PublicCompensationLotPageResponse, string>> {
  const normalizedPage = Math.max(1, page);
  const url = `/api/v1/compensation-pool/${encodeURIComponent(auctionId)}/lots?page=${normalizedPage}&page_size=${pageSize}`;
  const sent = await sendGet(url, signal);
  if (!sent.ok) {
    return err(transportErrorCode(sent.error));
  }
  if (signal.aborted) {
    return err("network_error");
  }
  const response = sent.value;
  if (!responseOk(response)) {
    return err(decodeErrorCode(response));
  }
  return decodeCompensationLots(response);
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status < 300;
}
