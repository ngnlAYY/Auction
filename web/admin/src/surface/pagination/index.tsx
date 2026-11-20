// Admin 分页 owner：数量 URL、独立本地偏好、共享控件与请求身份门禁。
// 回调属性保持必填；Solid 在事件发生时读取 props，不把回调句柄留存到本次渲染之外。

import { type QueryState, useQueryState, writeQueryPairs } from "@auction/web-shared/net/query";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource, type ResourceState } from "@auction/web-shared/reactive/resource";
import { ok, type Result } from "@auction/web-shared/result";
import { Pager } from "@auction/web-shared/ui/components/pager";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, createEffect } from "solid-js";

import {
  load as loadPreference,
  parse as parsePreference,
  save as savePreference,
} from "./preferences";

const PAGE_SIZE_OPTIONS: readonly number[] = [10, 20, 50, 100];

/** Rust `i64` 的可表示范围；偏移上限只在此范围内有意义。 */
const I64_MIN = -9_223_372_036_854_775_808n;
const I64_MAX = 9_223_372_036_854_775_807n;

/** 同页各表分别持有数量信号；数量不进入 API helper 的浏览器依赖。 */
export interface AdminPagination {
  readonly query: QueryState<number>;
  readonly scope: string;
  readonly keys: readonly string[];
  readonly maximum: number | null;
  /** 可接受的起始偏移上界；用 `bigint` 传 `i64::MAX` 时保持整除与加一的精度。 */
  readonly maxOffset: number | bigint;
  /** 手工多信号页面显式订阅数量，保留其既有 URL 同步协议。 */
  sizeSignal(): Cell<number>;
  /** 当前已应用的数量。 */
  size(): number;
  /** 与 Rust `PartialEq` 对应：数量信号与偏好域都相同的句柄才视为同一个分页 owner。 */
  equals(other: AdminPagination): boolean;
}

/** URL 明确值（含非法回退 10）优先；浏览链接不保存偏好。 */
export function useAdminPagination(
  scope: string,
  keys: readonly string[],
  maximum: number | null,
  maxOffset: number | bigint,
): AdminPagination {
  const sizeKey = firstKey(keys);
  const query = useQueryState<number>(
    keys,
    (pairs) => {
      const pair = pairs.find(([key]) => key === sizeKey);
      return pair === undefined
        ? loadPreference(scope, maximum)
        : (parsePreference(pair[1], maximum) ?? 10);
    },
    (size) => [[sizeKey, size.toString()] as const],
    // 数量切换的 push 与 replace 都可能在相同地址上发生；Rust 会照常写入历史项。
    { sameAddress: "write" },
  );
  const size = query.state;
  // 初次访问也固定有效数量，之后筛选/排序/翻页生成的每条历史都会继承它。
  // 仅规范 URL，不保存偏好；返回旧条目不会被后来保存的偏好污染。
  createEffect(
    () => size(),
    (value) => {
      writeQueryPairs(keys, [[sizeKey, value.toString()] as const], "replace");
    },
  );
  return {
    query,
    scope,
    keys,
    maximum,
    maxOffset,
    sizeSignal: () => query.state,
    size: () => query.state(),
    equals: (other) => query.state === other.query.state && scope === other.scope,
  };
}

/** 旧成功结果只在查询身份及数量都一致时可见；刷新同一请求不清除未完成命令。
 * 请求闭包可订阅 reload signal，但返回身份应仅含筛选、实体与页码。 */
export function useAdminPage<Q, T>(
  pagination: AdminPagination,
  request: () => Q,
  fetch: (query: Q, size: number, signal: AbortSignal) => Promise<Result<T, string>>,
): Accessor<Result<T, string> | null> {
  return useAdminPageWithState(pagination, request, fetch)[0];
}

/** 给携带版本的写操作标记读取世代。等待新版本时，表格可保留上一份快照和交互状态。 */
export function useVersionedAdminPage<Q, T>(
  pagination: AdminPagination,
  reload: Accessor<number>,
  request: () => Q,
  fetch: (query: Q, size: number, signal: AbortSignal) => Promise<Result<T, string>>,
): VersionedAdminPage<T> {
  const page = useAdminPageRead(pagination, reload, request, fetch);
  return createValueMemo((): Result<readonly [number, T], string> | null => {
    const loaded = page.loaded();
    if (loaded === null) {
      return null;
    }
    return loaded.result.ok ? ok([loaded.generation, loaded.result.value] as const) : loaded.result;
  });
}

type VersionedAdminPage<T> = Accessor<Result<readonly [number, T], string> | null>;

/** 额外返回读取状态，供自动轮询跳过仍在进行的请求；展示身份与快照复用规则保持一致。 */
export function useAdminPageWithState<Q, T>(
  pagination: AdminPagination,
  request: () => Q,
  fetch: (query: Q, size: number, signal: AbortSignal) => Promise<Result<T, string>>,
): readonly [Accessor<Result<T, string> | null>, Accessor<ResourceState>] {
  const page = useAdminPageRead(pagination, () => 0, request, fetch);
  const result = createValueMemo(() => page.loaded()?.result ?? null);
  return [result, page.state];
}

interface AdminPageDependencies<Q> {
  readonly identity: Q;
  readonly size: number;
  readonly generation: number;
}

interface LoadedAdminPage<Q, T> {
  readonly identity: Q;
  readonly size: number;
  readonly generation: number;
  readonly result: Result<T, string>;
}

interface AdminPageRead<Q, T> {
  readonly loaded: Accessor<LoadedAdminPage<Q, T> | null>;
  readonly state: Accessor<ResourceState>;
}

function useAdminPageRead<Q, T>(
  pagination: AdminPagination,
  generation: () => number,
  request: () => Q,
  fetch: (query: Q, size: number, signal: AbortSignal) => Promise<Result<T, string>>,
): AdminPageRead<Q, T> {
  const resource = createTaskResource(
    (): AdminPageDependencies<Q> => ({
      identity: request(),
      size: pagination.size(),
      generation: generation(),
    }),
    async (dependencies, context) => {
      const result = await fetch(dependencies.identity, dependencies.size, context.signal);
      // 被取代或被取消的任务不再交付；旧结果保留到新请求完成。
      if (!context.alive()) {
        return null;
      }
      return {
        identity: dependencies.identity,
        size: dependencies.size,
        generation: dependencies.generation,
        result,
      };
    },
  );
  // reload 只驱动请求；同一身份开始重读时不再复制并深比较整页旧结果。
  const requested = createValueMemo(() => ({
    identity: request(),
    size: pagination.size(),
  }));
  const loaded = createValueMemo((): LoadedAdminPage<Q, T> | null => {
    const page = resource();
    if (page === null || page === undefined) {
      return null;
    }
    const current = requested();
    return structuralEquals(page.identity, current.identity) && page.size === current.size
      ? page
      : null;
  });
  return { loaded, state: resource.state };
}

/** 保留表格原有标签，统一注入翻页/跳页文案、数字页码、数量文案、上限与偏好写入。 */
export interface AdminPagerProps {
  readonly pagination: AdminPagination;
  readonly page: number;
  readonly pageSize: number;
  readonly total?: number | null;
  readonly hasMore?: boolean;
  readonly label: string;
  readonly onPageChange: (page: number) => void;
  /** 本地报告页码不写历史；数量选择自行创建历史项。 */
  readonly local?: boolean;
  /** 目录数量变化只重置目录页，不清除其他独立报告分页或命令上下文。
   * 返回 true 表示已重置，缺省回调返回 false，由常规翻页动作回到首页。 */
  readonly onSizeReset?: () => boolean;
}

export function AdminPager(props: AdminPagerProps): JSX.Element {
  const maxPage = createValueMemo(() =>
    maxPageFromOffset(props.pagination.maxOffset, props.pageSize),
  );
  const maximum = createValueMemo(() => props.pagination.maximum);
  const totalSummary = createValueMemo(() => {
    const total = props.total ?? null;
    return total === null ? "" : t("admin-pagination-total", { count: String(total) });
  });
  const sizeLimitLabel = createValueMemo(() => {
    const limit = maximum();
    return limit === null
      ? t("admin-pagination-positive-integer")
      : t("admin-pagination-size-limit", { maximum: String(limit) });
  });

  return (
    <Pager
      page={props.page}
      pageSize={props.pageSize}
      total={props.total ?? null}
      hasMore={props.hasMore === true}
      maxPage={maxPage()}
      maxPageSize={maximum()}
      label={props.label}
      previousLabel={t("admin-pagination-previous")}
      status={(value) => t("admin-pagination-status", { page: value[0], pages: value[1] })}
      nextLabel={t("admin-pagination-next")}
      jumpLabel={t("admin-pagination-input")}
      jumpButtonLabel={t("admin-pagination-go")}
      totalSummary={totalSummary()}
      rangeSummary={(value) =>
        t("admin-pagination-range", {
          start: String(value[0]),
          end: String(value[1]),
          total: String(value[2]),
        })
      }
      pageSizeOptions={PAGE_SIZE_OPTIONS}
      pageSizeLabel={t("admin-pagination-size")}
      pageSizeOptionLabel={(count) => t("admin-pagination-size-option", { count: String(count) })}
      allowCustomPageSize={true}
      pageSizeCustomLabel={t("admin-pagination-custom")}
      pageSizeApplyLabel={t("admin-pagination-apply")}
      pageSizeInvalidLabel={sizeLimitLabel()}
      onPageChange={(next) => {
        // 数量已切换时，旧控件尚未卸载的纠偏不能覆盖新请求页码。
        if (props.pagination.size() === props.pageSize) {
          props.onPageChange(next);
        }
      }}
      onPageSizeChange={(size) => {
        const pagination = props.pagination;
        if (
          pagination.size() !== props.pageSize ||
          size <= 0 ||
          (pagination.maximum !== null && size > pagination.maximum)
        ) {
          return;
        }
        // 先固定离开历史项的旧数量，随后由原页面 push 首页，再 replace 新数量。
        // 各 hook 只写自己拥有的键，保留筛选、排序及未知参数。
        writeQueryPairs(
          pagination.keys,
          [[firstKey(pagination.keys), pagination.size().toString()] as const],
          "replace",
        );
        if (!(props.onSizeReset?.() ?? false)) {
          props.onPageChange(1);
        }
        if (props.local === true) {
          pagination.query.push(size);
        } else {
          pagination.query.replace(size);
        }
        savePreference(pagination.scope, size);
      }}
    />
  );
}

function firstKey(keys: readonly string[]): string {
  const key = keys[0];
  if (key === undefined) {
    // Rust 对空 keys 的索引会 panic；这里显式失败，避免静默写出错误键。
    throw new Error("Admin 分页键不能为空");
  }
  return key;
}

/**
 * Rust `checked_div(...).unwrap_or(0).saturating_add(1)` 的整数语义。
 * 偏移上限是 `i64`，先按精确整数相除再加一，避免 `i64::MAX / 2048` 这类结果被
 * `number` 舍入到相邻值；页面只以 `number` 消费最大页码，超出安全整数时取共享
 * `Pager` 的缺省上界（权威 `total` 本身受 JSON 安全整数约束，不会落到这一段）。
 */
function maxPageFromOffset(maxOffset: number | bigint, pageSize: number): number {
  // 调用方按 `i64` 传值；字面量接近 `i64::MAX` 的 `number` 可能已被舍入到值域外，
  // 夹回 `i64` 边界才与 Rust 收到同一个上限。
  const exact = typeof maxOffset === "bigint" ? maxOffset : BigInt(Math.trunc(maxOffset));
  const offset = exact > I64_MAX ? I64_MAX : exact < I64_MIN ? I64_MIN : exact;
  const size = BigInt(Math.max(Math.trunc(pageSize), 1));
  // 除数恒大于零，`checked_div` 不会溢出；商为 `i64::MAX` 时加一饱和。
  const quotient = offset / size;
  const pages = quotient >= I64_MAX ? I64_MAX : quotient + 1n;
  return pages > BigInt(Number.MAX_SAFE_INTEGER) ? Number.MAX_SAFE_INTEGER : Number(pages);
}
