// 路由页面的代码加载失败后的恢复：整页重新加载当前地址一次。
// 部署新版本后，仍开着的旧标签页引用的页面分块已经不存在，点到尚未加载过的页面时动态导入会失败。
// 这时地址栏已经是目标地址，重新加载就能拿到新的入口与分块；同一地址短时间内只自动重载一次，
// 资源确实缺失时不会无限刷新，由调用方改为显示错误横幅。

import { getItem, setItem } from "../../platform/storage";

const RELOAD_MARK_KEY = "hv.page-load-reload";
/** 同一地址在这段时间内只自动重载一次。 */
const RELOAD_GUARD_MILLIS = 30_000;

/** 是否应当重载，以及随之写入的标记。 */
export interface PageLoadReloadDecision {
  readonly reload: boolean;
  readonly mark: string;
}

/**
 * 纯判定：`mark` 是上一次重载留下的标记（`<毫秒时刻>|<地址>`，没有或已损坏时为 null 或任意文本）。
 * 同一地址在保护时间内已经重载过则不再重载；时钟回拨（标记时刻晚于现在）按“刚重载过”处理。
 */
export function pageLoadReloadDecision(
  mark: string | null,
  href: string,
  now: number,
): PageLoadReloadDecision {
  const next = `${now}|${href}`;
  if (mark === null) {
    return { reload: true, mark: next };
  }
  const separator = mark.indexOf("|");
  if (separator <= 0) {
    return { reload: true, mark: next };
  }
  const markedAt = Number(mark.slice(0, separator));
  const markedHref = mark.slice(separator + 1);
  if (!Number.isFinite(markedAt) || markedHref !== href) {
    return { reload: true, mark: next };
  }
  const elapsed = now - markedAt;
  return { reload: elapsed >= RELOAD_GUARD_MILLIS, mark: next };
}

/**
 * 路由页面的代码加载失败时调用。返回 true 表示已发起整页重载；返回 false 时调用方应显示错误横幅
 * （不在浏览器里、刚为同一地址重载过，或标记无法写入——没有标记就无法防止循环刷新）。
 */
export function reloadAfterPageLoadError(): boolean {
  if (typeof window === "undefined") {
    return false;
  }
  let href: string;
  try {
    href = window.location.href;
  } catch {
    return false;
  }
  const decision = pageLoadReloadDecision(getItem(RELOAD_MARK_KEY), href, Date.now());
  if (!decision.reload || !setItem(RELOAD_MARK_KEY, decision.mark)) {
    return false;
  }
  try {
    window.location.reload();
  } catch {
    return false;
  }
  return true;
}
