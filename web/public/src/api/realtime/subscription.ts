// HTTP catch-up 与 WebSocket 订阅生命周期。
// 连接建立只受握手超时约束；业务消息空闲不能证明断线，协议 Ping/Pong 不暴露给浏览器 API。
// HTTP 补读由自己的结果结束，之后读循环按队列处理已到达的套接字事件。
// 每帧先解码，序列跳号时回到 HTTP 补发，缺口仍存在才降级重连。

import { transportErrorCode } from "@auction/web-shared/net/transport";
import { createPromiseResolvers } from "@auction/web-shared/platform/promise";
import { createTimeoutFuture } from "@auction/web-shared/platform/timer";
import { err, ok, type Result } from "@auction/web-shared/result";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import { observeServerBuildId } from "@auction/web-shared/ui/build-notice";
import { sendGet } from "../clock";

import { decodeBuildHello } from "../decoder/build-hello";
import { decodeAuctionRealtimePage, decodeRealtimeEvent } from "../decoder/realtime";
import {
  canonicalAuctionId,
  type RealtimeError,
  type RealtimePage,
  realtimeErrorPermanent,
  realtimeErrorTransient,
  validateRealtimePageSequence,
  websocketUrl,
} from "./protocol";

/** 握手超时；只限制连接建立阶段（见模块文档）。 */
const REALTIME_CONNECT_TIMEOUT_MILLIS = 10_000;
/** 连续毒帧上限：单帧解码失败只计数降级，连续 5 帧失败才断开整条订阅。 */
const MAX_CONSECUTIVE_BAD_FRAMES = 5;
/** 握手与 HTTP 补读期间最多缓存 256 帧；溢出后重连并从持久事件游标补读。 */
const MAX_QUEUED_FRAMES = 256;
/** HTTP catch-up 翻页上限：落后超过 50 页（5000 事件）说明逐页重放代价过大，
 * 转为全量 refetch 降级，由各 resource 重读权威快照。 */
const MAX_CATCH_UP_PAGES = 50;
const PAGE_SIZE = 100;

/** 读取单拍卖的一页持久化事件，并校验目标、页码、序列与失败类别。 */
async function fetchRealtimePage(
  after: number,
  auctionId: string,
  signal: AbortSignal,
): Promise<Result<RealtimePage, RealtimeError>> {
  if (after < 0) {
    return err(realtimeErrorPermanent("实时事件游标无效"));
  }
  const canonical = canonicalAuctionId(auctionId);
  if (!canonical.ok) {
    return err(realtimeErrorPermanent(canonical.error));
  }
  const query = new URLSearchParams();
  query.append("auction_id", canonical.value);
  query.append("after", String(after));
  query.append("page_size", String(PAGE_SIZE));
  let observedStatus: number | null = null;
  const response = await sendGet(`/api/v1/events?${query.toString()}`, signal, (status) => {
    observedStatus = status;
  }).catch(() => null);
  if (response === null) {
    if (signal.aborted) {
      return err(realtimeErrorTransient("network_error"));
    }
    const status = observedStatus;
    if (status === 401 || status === 403 || status === 404) {
      return err(realtimeErrorPermanent(`HTTP ${status}`));
    }
    return err(realtimeErrorTransient("invalid_response"));
  }
  if (signal.aborted) {
    return err(realtimeErrorTransient("network_error"));
  }
  // 响应头已确认拒绝时，正文断流或超时仍按该 HTTP 状态结束订阅。
  const status = response.ok ? response.value.status : observedStatus;
  if (status === 401 || status === 403 || status === 404) {
    return err(realtimeErrorPermanent(`HTTP ${status}`));
  }
  if (!response.ok) {
    return err(realtimeErrorTransient(transportErrorCode(response.error)));
  }
  if (!responseOk(response.value)) {
    return err(realtimeErrorTransient(`HTTP ${response.value.status}`));
  }
  const payload = decodeAuctionRealtimePage(response.value, canonical.value);
  if (!payload.ok) {
    return err(realtimeErrorTransient(payload.error));
  }
  const page: RealtimePage = {
    events: payload.value.events,
    next_after: payload.value.next_after,
    has_more: payload.value.has_more,
    refetch_required: payload.value.refetch_required,
  };
  const sequence = validateRealtimePageSequence(page, after);
  if (!sequence.ok) {
    return err(realtimeErrorTransient(sequence.error));
  }
  return ok(page);
}

type Incoming =
  | { kind: "text"; text: string }
  | { kind: "binary" }
  | { kind: "failed"; detail: string }
  | { kind: "closed" };

interface Inbox {
  next(signal: AbortSignal): Promise<Incoming | null>;
  dispose(): void;
}

function responseOk(response: { status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}

function signalAborted(signal: AbortSignal | undefined): boolean {
  return signal?.aborted === true;
}

/**
 * 订阅单个拍卖的实时流，并把服务端事件页交给页面替换当前安全快照；拍卖页面用它对齐旧版的
 * lot/竞价即时刷新边界。浏览器 WebSocket 自动携带同源 HttpOnly session Cookie，匿名客户端由
 * 服务端拒绝。连接建立后先走 HTTP catch-up，期间到达的单事件帧由 WebSocket 缓冲，避免补发与
 * 订阅间隙。
 */
export async function subscribeAuctionRealtime(
  after: number,
  auctionId: string,
  world: string,
  onPage: (page: RealtimePage) => boolean,
  signal: AbortSignal,
): Promise<Result<null, RealtimeError>> {
  if (signal.aborted) {
    return ok(null);
  }
  if (after < 0) {
    return err(realtimeErrorPermanent("实时事件游标无效"));
  }
  const canonical = canonicalAuctionId(auctionId);
  if (!canonical.ok) {
    return err(realtimeErrorPermanent(canonical.error));
  }
  const url = websocketUrl(canonical.value);
  if (!url.ok) {
    return err(realtimeErrorPermanent(url.error));
  }
  const openedSocket = openSocket(url.value);
  if (!openedSocket.ok) {
    return openedSocket;
  }
  const connection = openedSocket.value;
  try {
    const result = await receiveRealtime(connection, after, canonical.value, world, onPage, signal);
    // 退出原因决定错误类别；关闭码只附在这一次返回的错误上。
    return result.ok ? result : err({ ...result.error, closeCode: connection.closeCode() });
  } finally {
    connection.inbox.dispose();
  }
}

async function receiveRealtime(
  connection: SocketConnection,
  after: number,
  auctionId: string,
  world: string,
  onPage: (page: RealtimePage) => boolean,
  signal: AbortSignal,
): Promise<Result<null, RealtimeError>> {
  const outcome = await waitForSocketOpen(connection.opened, signal);

  if (outcome === "aborted" || signalAborted(signal)) {
    return ok(null);
  }
  if (outcome === "timeout") {
    return err(realtimeErrorTransient("实时 WebSocket 握手超时"));
  }
  if (outcome === "failed" || !connection.isOpen()) {
    return err(realtimeErrorTransient("实时 WebSocket 建立失败"));
  }

  const expectedAfter = { value: after };
  const caughtUp = await catchUpRealtime(auctionId, expectedAfter, onPage, signal);
  if (signalAborted(signal)) {
    return ok(null);
  }
  if (!caughtUp.ok) {
    return caughtUp;
  }

  let consecutiveBadFrames = 0;
  // 存活检查由服务端 Ping/Pong 拥有；静默拍卖保持连接，关闭/错误才触发恢复。
  for (;;) {
    const message = await connection.inbox.next(signal);
    if (signalAborted(signal) || message === null) {
      return ok(null);
    }
    if (message.kind === "closed") {
      return err(realtimeErrorTransient("实时 WebSocket 已关闭"));
    }
    if (message.kind === "failed") {
      return err(realtimeErrorTransient(`实时 WebSocket 读取失败：${message.detail}`));
    }
    if (message.kind === "binary") {
      // 二进制帧同样按毒帧计数，不直接终止订阅。
      reportBrowserError("realtime_protocol");
      consecutiveBadFrames += 1;
      if (consecutiveBadFrames >= MAX_CONSECUTIVE_BAD_FRAMES) {
        return err(realtimeErrorTransient("实时 WebSocket 连续返回不支持的二进制帧"));
      }
      continue;
    }
    // 建连首帧报告服务端构建号；识别后只更新偏差提示，不进入事件序列。
    const serverBuild = decodeBuildHello(message.text);
    if (serverBuild !== null) {
      observeServerBuildId(serverBuild);
      continue;
    }
    const decoded = decodeRealtimeEvent(message.text, auctionId, world);
    if (!decoded.ok) {
      reportBrowserError("realtime_protocol");
      consecutiveBadFrames += 1;
      if (consecutiveBadFrames >= MAX_CONSECUTIVE_BAD_FRAMES) {
        return err(realtimeErrorTransient(`实时事件帧连续解码失败：${decoded.error}`));
      }
      continue;
    }
    const event = decoded.value;
    consecutiveBadFrames = 0;
    if (event.stream_sequence <= expectedAfter.value) {
      continue;
    }
    const nextSequence = checkedAddOne(expectedAfter.value);
    if (nextSequence === null) {
      return err(realtimeErrorPermanent("实时事件序列超过可表示范围"));
    }
    if (event.stream_sequence !== nextSequence) {
      const catchUp = await catchUpRealtime(auctionId, expectedAfter, onPage, signal);
      if (signalAborted(signal)) {
        return ok(null);
      }
      if (!catchUp.ok) {
        return catchUp;
      }
    }
    if (event.stream_sequence <= expectedAfter.value) {
      continue;
    }
    const nextAfterCatchUp = checkedAddOne(expectedAfter.value);
    if (nextAfterCatchUp === null) {
      return err(realtimeErrorPermanent("实时事件序列超过可表示范围"));
    }
    if (event.stream_sequence !== nextAfterCatchUp) {
      reportBrowserError("realtime_protocol");
      return err(realtimeErrorTransient("实时事件序列出现断档"));
    }
    expectedAfter.value = event.stream_sequence;
    const page: RealtimePage = {
      events: [event],
      next_after: expectedAfter.value,
      has_more: false,
      refetch_required: false,
    };
    onPage(page);
  }
}

async function waitForSocketOpen(
  opened: SocketConnection["opened"],
  signal: AbortSignal,
): Promise<"open" | "failed" | "aborted" | "timeout"> {
  const timeout = createTimeoutFuture(REALTIME_CONNECT_TIMEOUT_MILLIS);
  let abortListener: (() => void) | null = null;
  const abortedGate = createPromiseResolvers<"aborted">();
  const aborted = abortedGate.promise;
  if (signal.aborted) {
    abortedGate.resolve("aborted");
  } else {
    abortListener = () => abortedGate.resolve("aborted");
    signal.addEventListener("abort", abortListener, { once: true });
  }
  try {
    return await Promise.race([opened, timeout.promise.then(() => "timeout" as const), aborted]);
  } finally {
    timeout.cancel();
    if (abortListener !== null) {
      signal.removeEventListener("abort", abortListener);
    }
  }
}

async function catchUpRealtime(
  auctionId: string,
  expectedAfter: { value: number },
  onPage: (page: RealtimePage) => boolean,
  signal: AbortSignal,
): Promise<Result<null, RealtimeError>> {
  let fetchedPages = 0;
  for (;;) {
    if (signal.aborted) {
      return ok(null);
    }
    const requestedAfter = expectedAfter.value;
    const pageResult = await fetchRealtimePage(requestedAfter, auctionId, signal);
    if (signalAborted(signal)) {
      return ok(null);
    }
    if (!pageResult.ok) {
      return pageResult;
    }
    const page = pageResult.value;
    const nextAfter = page.next_after;
    const hasMore = page.has_more;
    const refetchRequired = page.refetch_required;
    if (hasMore && nextAfter === requestedAfter) {
      return err(realtimeErrorTransient("实时事件补发游标未前进"));
    }
    expectedAfter.value = nextAfter;
    const continueCatchUp = onPage(page);
    if (refetchRequired || !continueCatchUp) {
      return ok(null);
    }
    if (!hasMore) {
      return ok(null);
    }
    fetchedPages += 1;
    if (fetchedPages >= MAX_CATCH_UP_PAGES) {
      if (signalAborted(signal)) {
        return ok(null);
      }
      // 落后页数超过上限：放弃逐页补齐，合成 refetch 页让各 resource 全量重读权威快照。
      console.warn("实时 catch-up 翻页超限，降级为全量 refetch", {
        auction_id: auctionId,
        after: expectedAfter.value,
        fetched_pages: fetchedPages,
      });
      const degradedPage: RealtimePage = {
        events: [],
        next_after: expectedAfter.value,
        has_more: false,
        refetch_required: true,
      };
      onPage(degradedPage);
      return ok(null);
    }
  }
}

function checkedAddOne(value: number): number | null {
  return value < Number.MAX_SAFE_INTEGER ? value + 1 : null;
}

interface SocketConnection {
  opened: Promise<"open" | "failed">;
  inbox: Inbox;
  isOpen(): boolean;
  closeCode(): number | null;
}

/** 建立原生 WebSocket，并在握手完成前就开始缓存帧，避免慢 catch-up 丢失订阅间隙事件。 */
function openSocket(url: string): Result<SocketConnection, RealtimeError> {
  if (typeof WebSocket === "undefined") {
    return err(realtimeErrorPermanent("实时 WebSocket 仅在 wasm32 浏览器目标可用"));
  }
  let socket: WebSocket;
  try {
    socket = new WebSocket(url);
  } catch (error) {
    return err(realtimeErrorTransient(error instanceof Error ? error.message : String(error)));
  }

  const queue: Incoming[] = [];
  const waiters: Array<(message: Incoming | null) => void> = [];
  let closed = false;
  let closeCode: number | null = null;
  let opened = false;
  const openGate = createPromiseResolvers<"open" | "failed">();
  let resolveOpened: typeof openGate.resolve | null = openGate.resolve;
  const openedPromise = openGate.promise;

  const settleOpen = (state: "open" | "failed"): void => {
    if (resolveOpened !== null) {
      resolveOpened(state);
      resolveOpened = null;
    }
  };
  const failQueueOverflow = (): void => {
    closed = true;
    queue.length = 0;
    queue.push({ kind: "failed", detail: "WebSocket 接收队列超过上限" });
    settleOpen("failed");
    try {
      socket.close();
    } catch {
      // 关闭失败不恢复已丢弃的缓冲帧；下一次订阅会从持久游标补读。
    }
  };
  const push = (message: Incoming): void => {
    if (closed) return;
    const waiter = waiters.shift();
    if (waiter !== undefined) {
      waiter(message);
      return;
    }
    if (queue.length >= MAX_QUEUED_FRAMES) {
      failQueueOverflow();
      return;
    }
    queue.push(message);
  };
  const markClosed = (): void => {
    if (!closed) {
      closed = true;
      const waiter = waiters.shift();
      waiter?.({ kind: "closed" });
    }
  };

  const onMessage = (event: MessageEvent): void => {
    if (typeof event.data === "string") {
      push({ kind: "text", text: event.data });
    } else {
      push({ kind: "binary" });
    }
  };
  const onOpen = (): void => {
    opened = true;
    settleOpen("open");
  };
  const onError = (): void => {
    if (!opened) {
      settleOpen("failed");
    } else {
      push({ kind: "failed", detail: "WebSocket 错误" });
    }
  };
  const recordCloseCode = (event: CloseEvent): void => {
    closeCode ??= event.code;
  };
  const onClose = (): void => {
    if (!opened) {
      settleOpen("failed");
    }
    markClosed();
  };

  socket.addEventListener("message", onMessage);
  socket.addEventListener("open", onOpen);
  socket.addEventListener("error", onError);
  socket.addEventListener("close", recordCloseCode);
  socket.addEventListener("close", onClose);
  if (socket.readyState === WebSocket.OPEN) {
    onOpen();
  }

  const inbox: Inbox = {
    next: (signal) => {
      if (signalAborted(signal)) {
        return Promise.resolve(null);
      }
      const queued = queue.shift();
      if (queued !== undefined) {
        return Promise.resolve(queued);
      }
      if (closed) {
        return Promise.resolve({ kind: "closed" });
      }
      let settled = false;
      const pending = createPromiseResolvers<Incoming | null>();
      const finish = (message: Incoming | null): void => {
        if (settled) {
          return;
        }
        settled = true;
        signal?.removeEventListener("abort", onAbort);
        const index = waiters.indexOf(waiter);
        if (index >= 0) {
          waiters.splice(index, 1);
        }
        pending.resolve(message);
      };
      const onAbort = (): void => finish(null);
      const waiter = (message: Incoming | null): void => finish(message);
      waiters.push(waiter);
      signal?.addEventListener("abort", onAbort, { once: true });
      if (signalAborted(signal)) {
        onAbort();
      }
      return pending.promise;
    },
    dispose: () => {
      socket.removeEventListener("message", onMessage);
      socket.removeEventListener("open", onOpen);
      socket.removeEventListener("error", onError);
      socket.removeEventListener("close", recordCloseCode);
      socket.removeEventListener("close", onClose);
      markClosed();
      for (const waiter of waiters.splice(0)) {
        waiter({ kind: "closed" });
      }
      if (socket.readyState === WebSocket.CONNECTING || socket.readyState === WebSocket.OPEN) {
        try {
          socket.close();
        } catch {
          // 关闭失败不影响订阅结果；浏览器会自行回收。
        }
      }
    },
  };

  return ok({
    opened: openedPromise,
    inbox,
    isOpen: () => socket.readyState === WebSocket.OPEN,
    closeCode: () => closeCode,
  });
}
