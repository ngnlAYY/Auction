// Public 只读公开 GET 的内存 SWR/TTL 缓存。
// 只有 auctions/current、拍卖目录、当前 lots、lot 详情、竞价日志、履约进度、价格历史与历史场次
// 可以进入缓存；会话、账号、watches、偏好、代理私有读取和命令响应永不缓存。条目保存的是经严格
// decoder 校验后的成功正文投影、响应 ETag（若有）与写入时间；缓存不参与金额、资格、幂等、锁、
// 会话或审计，命中只表示“同一完整 URL 的最近一次已验证成功投影”，清空或丢失缓存只会多读一次
// 权威接口。
//
// 语义：TTL 内且未被失效的条目直接返回；过期或被失效后携带 ETag 条件请求，304 刷新写入时间
// 并恢复新鲜。传输失败（网络/超时）与服务端暂时拒绝（429 读闸繁忙或来源超额、503 依赖暂不
// 可用、源站不可达时的网关 502/504 与 CDN 520–524）在已有条目时回退旧值
// （stale-while-revalidate）并标明这是回退及该快照最近一次被服务端确认的时刻，没有条目时照常
// 返回错误；其他 4xx/5xx 与正文校验失败失败关闭，既不写入也不回退。旧快照只用于读视图展示：
// 命令仍由服务端权威校验，过期的下一口价只会被服务端拒绝。失效只把全部条目标为过期（保留正文
// 与 ETag）并推进修订号。
// 同一 URL 的读取经在途登记串行：已有请求在途时，后到的读取等它结束再按上述规则查缓存，不并发
// 发出相同请求，因此成功响应不会比已有条目旧，总是替换其正文与 ETag。请求期间发生过失效时，
// 响应只作为过期条目保留，供条件请求与暂时性失败回退，不作为新鲜命中；在途请求失败、被取消或
// 期间失效时，等待方自行请求。
//
// 本层是这些读取唯一的客户端新鲜度来源：未命中（或旧条目没有 ETag）时的请求要求浏览器向服务端
// 再验证，不接受 HTTP 缓存按 max-age/stale-while-revalidate 给出的旧副本，否则旧正文会被当作
// 新快照保留到 TTL 结束；带 If-None-Match 的条件请求按 Fetch 规范同样绕过 HTTP 缓存。失效时机
// 是实时事件、开场迁移、外壳权威轮询（非 Live 每 60 秒，Live 约每 5 分钟兜底）、页面恢复可见、
// 看板对齐重读与错误重试、会话变化，以及竞价与代理命令得到服务端答复（成功或被拒绝）；只推进
// 页面刷新代号而不失效时，TTL 内的读取直接命中新鲜条目，不会回到服务端。命令答复后的失效另把
// 回退下限推进到新修订号：用户刚提交的命令之后，读视图只显示权威结果或错误，不退回命令之前的
// 价格与领先者；回退资格在失败返回时按当时最新的下限判定，命令答复发生在重读在途期间同样生效。
// 他人竞价等事件触发的普通失效仍允许繁忙时先显示最后已知快照。

import { err, ok, type Result, unreachable } from "@auction/web-shared/result";

/** 实时视图的缓存有效期：当前拍卖、当前 lots、lot 详情与竞价日志。 */
export const LIVE_TTL_MILLIS = 60_000;
/** 目录视图的缓存有效期：拍卖列表、价格历史、历史场次与履约进度。 */
export const CATALOG_TTL_MILLIS = 15_000;
/** 单线程内存缓存条目上限；失效只标记过期、不删除条目，超出上限时淘汰最早写入的条目。
 * 导出仅供测试使用。 */
export const MAX_ENTRIES = 128;

/** 单次缓存读取的请求结果分类；由调用方的 fetch 闭包在完整校验后构造。 */
export type FetchOutcome<T, E> =
  | {
      readonly kind: "changed";
      /** 2xx 且正文通过调用方严格校验；ETag 缺失时后续过期读取退回普通 GET。 */
      readonly value: T;
      readonly etag: string | null;
    }
  | {
      /** 条件请求返回 304；只有存在缓存条目时才会出现。 */
      readonly kind: "not_modified";
    }
  | {
      /** 网络/超时等传输失败：允许用旧值继续，不写入缓存。 */
      readonly kind: "transport";
      readonly error: E;
    }
  | {
      /** 服务端暂时拒绝：与传输失败相同，有可回退的条目时回退旧值。 */
      readonly kind: "busy";
      readonly error: E;
    }
  | {
      /** 其他非 2xx 或正文校验失败：失败关闭，既不写入缓存也不回退旧值。 */
      readonly kind: "rejected";
      readonly error: E;
    };

/** 一次缓存读取得到的值及其来源。 */
export interface CacheRead<T> {
  readonly value: T;
  /** 重读遇到暂时性失败时返回旧快照，给出该快照最近一次被服务端确认的毫秒时刻。 */
  readonly staleFallback: number | null;
}

interface CacheEntry<T> {
  /** 写入该投影的查询类型标识；与读取方不一致时按未命中处理。 */
  typeId: string;
  value: T;
  etag: string | null;
  storedAt: number;
  /** 条目在当前修订号下写入或确认；失效后为 false。 */
  current: boolean;
  /** 条目写入或最近一次确认时的修订号。 */
  revision: number;
}

interface Store {
  entries: Map<string, CacheEntry<unknown>>;
  /** 每次失效递增，已有条目随之全部过期。 */
  revision: number;
  /** 同一 URL 正在进行的请求。 */
  pending: Map<string, Pending>;
  /** 暂时性失败回退的修订号下限。 */
  fallbackFloor: number;
}

const STORE: Store = {
  entries: new Map(),
  revision: 0,
  pending: new Map(),
  fallbackFloor: 0,
};

/** 仅供测试使用：把缓存恢复为初始状态，使每个用例从干净缓存开始。 */
export function resetStoreForTest(): void {
  STORE.entries.clear();
  STORE.revision = 0;
  STORE.pending.clear();
  STORE.fallbackFloor = 0;
}

/** 仅供测试使用：当前缓存条目数。 */
export function entryCountForTest(): number {
  return STORE.entries.size;
}

/** 仅供测试使用：判断完整 URL 是否有缓存条目。 */
export function hasEntryForTest(url: string): boolean {
  return STORE.entries.has(url);
}

/** 在途请求的完成通知；只表示“该请求已结束”，不携带结果。 */
class Pending {
  done = false;
  readonly promise: Promise<void>;
  #resolve: (() => void) | null = null;

  constructor() {
    this.promise = new Promise<void>((resolve) => {
      this.#resolve = resolve;
    });
  }

  finish(): void {
    this.done = true;
    const resolve = this.#resolve;
    if (resolve !== null) {
      this.#resolve = null;
      resolve();
    }
  }
}

/** 等待在途请求结束；调用方被取消时立即返回 `false`，不再关心该请求的结果。 */
function waitForPending(pending: Pending, signal: AbortSignal): Promise<boolean> {
  if (pending.done) {
    return Promise.resolve(true);
  }
  if (signal.aborted) {
    return Promise.resolve(false);
  }
  return new Promise<boolean>((resolve) => {
    let settled = false;
    const onAbort = () => finish(false);
    const finish = (completed: boolean): void => {
      if (settled) {
        return;
      }
      settled = true;
      signal.removeEventListener("abort", onAbort);
      resolve(completed);
    };
    signal.addEventListener("abort", onAbort, { once: true });
    if (signal.aborted) {
      onAbort();
      return;
    }
    void pending.promise.then(() => finish(true));
  });
}

/** 登记在途请求；正常结束、出错或调用方被丢弃时都会撤销登记并唤醒等待方。 */
class PendingGuard {
  readonly url: string;
  readonly pending: Pending;

  constructor(url: string, pending: Pending) {
    this.url = url;
    this.pending = pending;
  }

  static register(url: string): PendingGuard {
    const pending = new Pending();
    STORE.pending.set(url, pending);
    return new PendingGuard(url, pending);
  }

  release(): void {
    if (STORE.pending.get(this.url) === this.pending) {
      STORE.pending.delete(this.url);
    }
    this.pending.finish();
  }
}

/** 判断完整 URL 是否属于允许缓存的公开只读 GET；路径按段精确匹配，查询参数不参与。 */
export function isCacheableUrl(url: string): boolean {
  const questionMark = url.indexOf("?");
  const path = questionMark < 0 ? url : url.slice(0, questionMark);
  const segments = path.split("/");
  return (
    segmentsEqual(segments, ["", "api", "v1", "auctions", "current"]) ||
    segmentsEqual(segments, ["", "api", "v1", "auctions"]) ||
    segmentsEqual(segments, ["", "api", "v2", "auctions"]) ||
    segmentsEqual(segments, ["", "api", "v1", "price-history"]) ||
    segmentsEqual(segments, ["", "api", "v1", "auctions", "*", "lots"]) ||
    segmentsEqual(segments, ["", "api", "v1", "auctions", "*", "bid-log"]) ||
    segmentsEqual(segments, ["", "api", "v1", "auctions", "*", "fulfillment"]) ||
    segmentsEqual(segments, ["", "api", "v1", "lots", "*"])
  );
}

function segmentsEqual(segments: readonly string[], pattern: readonly string[]): boolean {
  if (segments.length !== pattern.length) {
    return false;
  }
  for (let index = 0; index < pattern.length; index += 1) {
    const expected = pattern[index];
    const actual = segments[index];
    if (expected === undefined || actual === undefined) {
      return false;
    }
    if (expected !== "*" && expected !== actual) {
      return false;
    }
  }
  return true;
}

/** 返回浏览器墙钟毫秒；WASM 与浏览器使用 `Date.now()`，宿主测试可注入。 */
export function nowMillis(): number {
  const now = Date.now();
  // 与源实现的 `now as u64` 一致：有限正数向零截断，非法值归零。
  return Number.isFinite(now) && now > 0 ? Math.trunc(now) : 0;
}

/** 与 readOrFetch 相同，只取值，不区分是否为暂时性失败后的旧快照回退。 */
export async function getOrFetch<T, E>(
  url: string,
  ttlMillis: number,
  typeId: string,
  fetch: (etag: string | null) => Promise<FetchOutcome<T, E>>,
  abortError: E,
  signal: AbortSignal,
  now: () => number = nowMillis,
): Promise<Result<T, E>> {
  const read = await readOrFetch(url, ttlMillis, typeId, fetch, abortError, signal, now);
  return read.ok ? ok(read.value.value) : read;
}

/** 读取公开 URL：命中未过期且未被失效的条目时不调用 fetch；否则把旧条目的 ETag 交给 fetch
 * 做条件请求。`typeId` 区分同一 URL 的查询投影；`now` 与 `fetch` 都是注入点，测试无需网络或
 * 浏览器即可覆盖全部缓存语义。`signal` 中止后不再等待他人、请求或写入缓存，并在当前持有者
 * 退出时唤醒其他等待方；`abortError` 是该取消范围内唯一可观察的失败值。 */
export async function readOrFetch<T, E>(
  url: string,
  ttlMillis: number,
  typeId: string,
  fetch: (etag: string | null) => Promise<FetchOutcome<T, E>>,
  abortError: E,
  signal: AbortSignal,
  now: () => number = nowMillis,
): Promise<Result<CacheRead<T>, E>> {
  const current = (value: T): CacheRead<T> => ({ value, staleFallback: null });
  if (signal.aborted) {
    return err(abortError);
  }
  // 非白名单 URL 一律直连：不读取、不写入，也不改变失效修订号。
  if (!isCacheableUrl(url)) {
    const outcome = await fetch(null);
    if (signal.aborted) {
      return err(abortError);
    }
    switch (outcome.kind) {
      case "changed":
        return ok(current(outcome.value));
      case "transport":
      case "busy":
      case "rejected":
        return err(outcome.error);
      case "not_modified":
        // 调用方在未收到 ETag 时必须把 304 归为 rejected；这里没有可回退的值。
        throw new Error("未提供 ETag 的读取不应返回 304");
      default:
        return unreachable(outcome);
    }
  }
  const fresh = (entry: CacheEntry<T>): boolean =>
    entry.current && Math.max(0, now() - entry.storedAt) < ttlMillis;
  // 同一 URL 已有请求在途（例如启动预取）：等它结束再按同一规则读缓存。单线程内“检查—登记”
  // 之间没有 await，因此不会有两个请求同时登记同一 URL。
  let pending = STORE.pending.get(url);
  while (pending !== undefined) {
    if (!(await waitForPending(pending, signal))) {
      return err(abortError);
    }
    if (signal.aborted) {
      return err(abortError);
    }
    const freshEntry = lookup<T>(url, typeId);
    if (freshEntry !== undefined && fresh(freshEntry)) {
      return ok(current(freshEntry.value));
    }
    pending = STORE.pending.get(url);
  }
  if (signal.aborted) {
    return err(abortError);
  }
  const guard = PendingGuard.register(url);
  // 取消时立即释放在途登记，让仍存活的等待方重新发起读取，而不是继续等一个已经没有人需要的
  // 响应。旧的 fetch 仍须在自己的 finally 中收口；若等待方已登记新请求，它不会误删新登记。
  const releaseOnAbort = () => guard.release();
  signal.addEventListener("abort", releaseOnAbort, { once: true });
  try {
    if (signal.aborted) {
      return err(abortError);
    }
    const revision = currentRevision();
    const cached = lookup<T>(url, typeId);
    if (cached !== undefined) {
      if (fresh(cached)) {
        if (signal.aborted) {
          return err(abortError);
        }
        return ok(current(cached.value));
      }
      // 过期与已被失效的条目同样带旧 ETag 再验证：正文未变时 304 直接复用，不重传整页。
      const outcome = await fetch(cached.etag);
      if (signal.aborted) {
        return err(abortError);
      }
      switch (outcome.kind) {
        case "changed":
          store(url, typeId, outcome.value, outcome.etag, now(), revision);
          return ok(current(outcome.value));
        case "not_modified":
          // 304 表示正文在本次请求时仍未变：期间没有再发生失效时，前移写入时间并标回当前修订号。
          touch(url, now(), revision);
          return ok(current(cached.value));
        case "transport":
        case "busy":
          // 条目早于最近一次命令答复时不回退，按失败返回；下限可能在本次请求在途期间被推进，
          // 必须在此刻重新判定，不能沿用请求前结论。
          if (fallbackAllowed(cached.revision)) {
            return ok({ value: cached.value, staleFallback: cached.storedAt });
          }
          return err(outcome.error);
        case "rejected":
          return err(outcome.error);
        default:
          return unreachable(outcome);
      }
    }
    const outcome = await fetch(null);
    if (signal.aborted) {
      return err(abortError);
    }
    switch (outcome.kind) {
      case "changed":
        store(url, typeId, outcome.value, outcome.etag, now(), revision);
        return ok(current(outcome.value));
      case "transport":
      case "busy":
      case "rejected":
        return err(outcome.error);
      case "not_modified":
        throw new Error("未提供 ETag 的读取不应返回 304");
      default:
        return unreachable(outcome);
    }
  } finally {
    signal.removeEventListener("abort", releaseOnAbort);
    guard.release();
  }
}

/** 使全部公开读缓存过期；条目保留正文与 ETag，只供随后的条件请求与暂时性失败回退。 */
export function invalidateAll(): void {
  STORE.revision += 1;
}

/** 用户命令（竞价、代理创建与降额）得到服务端答复后调用：与 invalidateAll 一样使全部条目
 * 过期，另把回退下限推进到新修订号。 */
export function invalidateAfterCommand(): void {
  STORE.revision += 1;
  STORE.fallbackFloor = STORE.revision;
}

function currentRevision(): number {
  return STORE.revision;
}

/** 修订号为 revision 的条目此刻能否作为暂时性失败的回退值：不早于最近一次命令答复推进的下限。 */
function fallbackAllowed(revision: number): boolean {
  return revision >= STORE.fallbackFloor;
}

function lookup<T>(url: string, typeId: string): (CacheEntry<T> & { value: T }) | undefined {
  const entry = STORE.entries.get(url);
  if (entry === undefined || entry.typeId !== typeId) {
    return undefined;
  }
  // Rust 用 downcast_ref 区分同一 URL 的投影类型；这里以各查询模块定义的字符串标识作运行期
  // 证明，标识一致才允许把保存值收窄为该查询的类型。
  return {
    typeId: entry.typeId,
    // 与 Rust 的 `CachedEntry::value.clone()` 一致：缓存命中的值不能与缓存条目或另一调用方
    // 共享可变引用，保存方修改返回值时不能污染后续命中。
    value: structuredClone(entry.value as T),
    etag: entry.etag,
    storedAt: entry.storedAt,
    current: entry.revision === STORE.revision,
    revision: entry.revision,
  };
}

function store<T>(
  url: string,
  typeId: string,
  value: T,
  etag: string | null,
  storedAt: number,
  revision: number,
): void {
  // 同一 URL 的请求经在途登记串行，响应不会比已有条目旧，因此总是替换正文与 ETag：页面已展示
  // 这份响应，之后的条件请求与暂时性失败回退都不能退回更早的快照。revision 是请求开始时的
  // 修订号，期间发生过失效时条目随即落后于当前修订号，只作过期条目，不成为新鲜命中。
  if (!STORE.entries.has(url) && STORE.entries.size >= MAX_ENTRIES) {
    let oldestKey: string | null = null;
    let oldestStoredAt = Number.POSITIVE_INFINITY;
    for (const [key, entry] of STORE.entries) {
      if (entry.storedAt < oldestStoredAt) {
        oldestStoredAt = entry.storedAt;
        oldestKey = key;
      }
    }
    if (oldestKey !== null) {
      STORE.entries.delete(oldestKey);
    }
  }
  STORE.entries.set(url, {
    typeId,
    value: structuredClone(value),
    etag,
    storedAt,
    revision,
    current: false,
  });
}

function touch(url: string, storedAt: number, revision: number): void {
  if (STORE.revision !== revision) {
    return;
  }
  const entry = STORE.entries.get(url);
  if (entry !== undefined) {
    entry.storedAt = storedAt;
    entry.revision = revision;
  }
}
