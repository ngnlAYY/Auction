// 实时事件 wire 投影：统一页、合并结果、结构化错误，以及订阅 URL/序列校验。
// 这里不建立连接、不做防重；只有 `subscription` 才知道 HTTP 与 WebSocket 的传输细节。

import type { PublicRealtimeEventResponse as RealtimeEvent } from "@auction/contracts/dto/response";
import { err, ok, type Result } from "@auction/web-shared/result";

/** 已通过服务端事件注册表校验的实时事件 DTO。 */
export type { RealtimeEvent };

/**
 * 页面消费的统一实时页；wire 上的单拍卖 catch-up 页在完成目标校验后才投影到这里，
 * WebSocket 单事件帧与翻页超限的降级刷新也包装成同一形状。
 */
export interface RealtimePage {
  events: readonly RealtimeEvent[];
  next_after: number;
  has_more: boolean;
  refetch_required: boolean;
}

/** 一页实时事件对页面资源失效范围的投影。 */
export interface RealtimePageMerge {
  /** 拍卖级变化或恢复缺口要求重读全部权威快照。 */
  refresh_required: boolean;
  /** 仅已通过防重、事实和版本校验的竞价通知对应资源失效。 */
  bids_changed: boolean;
  continue_catch_up: boolean;
  degraded: boolean;
}

/** 实时订阅失败的结构化错误：页面层按 `kind` 选择重连策略，禁止对错误文案做字符串比较。 */
export interface RealtimeError {
  kind: "permanent" | "transient";
  /** 失败明细只用于诊断日志，不参与重连决策。 */
  detail: string;
  /** 浏览器已收到 WebSocket 关闭事件时的关闭码；其他错误为 null。 */
  closeCode: number | null;
}

/** 服务端通过策略关闭通知会话需要权威复核。 */
export const REALTIME_SESSION_CLOSE_CODE = 1008;

/** 重连无法自愈：游标或拍卖 ID 无效、认证/权限被拒绝、目标拍卖不存在。 */
export function realtimeErrorPermanent(detail: string): RealtimeError {
  return { kind: "permanent", detail, closeCode: null };
}

/** 网络抖动、服务端瞬时故障、毒帧超限或握手超时等可重试失败。 */
export function realtimeErrorTransient(
  detail: string,
  closeCode: number | null = null,
): RealtimeError {
  return { kind: "transient", detail, closeCode };
}

/** 失败是否属于重连无法自愈的永久错误。 */
export function realtimeErrorIsPermanent(error: RealtimeError): boolean {
  return error.kind === "permanent";
}

/** 读取仅用于诊断的失败明细。 */
export function realtimeErrorDetail(error: RealtimeError): string {
  return error.detail;
}

/** 由当前页面的协议与主机生成同源 WebSocket 地址；非 http(s) 页面失败关闭。 */
export function websocketUrl(auctionId: string): Result<string, string> {
  if (typeof window === "undefined") {
    return err("浏览器窗口不可用");
  }
  let protocol: string;
  try {
    protocol = window.location.protocol;
  } catch {
    return err("读取浏览器协议失败");
  }
  let host: string;
  try {
    host = window.location.host;
  } catch {
    return err("读取浏览器主机失败");
  }
  let websocketProtocol: string;
  if (protocol === "http:") {
    websocketProtocol = "ws:";
  } else if (protocol === "https:") {
    websocketProtocol = "wss:";
  } else {
    return err("浏览器协议不支持 WebSocket");
  }
  const query = new URLSearchParams();
  query.append("auction_id", auctionId);
  return ok(`${websocketProtocol}//${host}/api/v1/ws?${query.toString()}`);
}

/** 校验 catch-up 页没有回退、重复或越过 `next_after` 的事件序列。 */
export function validateRealtimePageSequence(
  page: RealtimePage,
  after: number,
): Result<null, string> {
  if (page.next_after < after) {
    return err("实时游标发生回退");
  }
  let previous = after;
  for (const event of page.events) {
    if (event.stream_sequence <= previous || event.stream_sequence > page.next_after) {
      return err("实时事件页序列无效");
    }
    previous = event.stream_sequence;
  }
  return ok(null);
}

/** 只接受规范的小写 UUID 文本；nil、非规范形式与其他 UUID 语法均拒绝。 */
export function canonicalAuctionId(value: string): Result<string, string> {
  if (
    value.length !== 36 ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(value) ||
    value === "00000000-0000-0000-0000-000000000000"
  ) {
    return err("拍卖 ID 无效");
  }
  return ok(value);
}
