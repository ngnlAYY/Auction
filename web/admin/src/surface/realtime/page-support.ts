// Realtime 页面的分区视图、分页边界与刷新计数辅助。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { Result } from "@auction/web-shared/result";
import type { Accessor } from "solid-js";

/** Rust `u64::MAX`；刷新计数与源码一样在边界饱和。 */
const U64_MAX = 18_446_744_073_709_551_615n;

/** 分区内容视图；首次完成前是 loading，读取失败时保留稳定错误码。 */
export type SectionView<T> =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "ready"; readonly page: T };

/** 读取完成前是加载态；失败与成功都保留稳定错误码/整页快照。 */
export function sectionView<T>(result: Result<T, string> | null): SectionView<T> {
  if (result === null) {
    return { kind: "loading" };
  }
  return result.ok ? { kind: "ready", page: result.value } : { kind: "error", code: result.error };
}

/** 分页页码在浏览器接口层是安全整数；超出范围按分页边界错误处理。 */
export function paginationPage(page: bigint): number | null {
  return page > BigInt(Number.MAX_SAFE_INTEGER) ? null : Number(page);
}

/** Rust `u64::saturating_add(1)`；刷新计数只用于触发重读。 */
export function bumpRefresh(refresh: Cell<bigint>): void {
  const current = refresh.peek();
  refresh.set(current === U64_MAX ? current : current + 1n);
}

/** 一个分区的展示视图、整页快照与错误码；三者都只在对应读取完成时才有值。 */
export interface SectionReads<T> {
  readonly view: Accessor<SectionView<T>>;
  readonly page: Accessor<T | null>;
  readonly error: Accessor<{ readonly code: string } | null>;
}

/** 把页面读取结果拆成 loading/error/ready；分页资源已按请求身份隐藏旧快照。 */
export function useSectionReads<T>(result: Accessor<Result<T, string> | null>): SectionReads<T> {
  const view = createValueMemo((): SectionView<T> => sectionView(result()));
  const page = createValueMemo((): T | null => {
    const current = view();
    return current.kind === "ready" ? current.page : null;
  });
  const error = createValueMemo((): { readonly code: string } | null => {
    const current = view();
    return current.kind === "error" ? current : null;
  });
  return { view, page, error };
}
