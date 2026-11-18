// 路由页面的代码加载失败后的恢复：整页重新加载当前地址一次。
// 部署新版本后，仍开着的旧标签页引用的页面分块已经不存在，点到尚未加载过的页面时动态导入会失败。
// 这时地址栏已经是目标地址，重新加载就能拿到新的入口与分块；同一标签页的同一路径短时间内只自动重载一次，
// 资源确实缺失时不会无限刷新，由调用方改为显示错误横幅。

import { jsonExactObject, parseJson } from "../../net/wire";
import { getSessionItem, setSessionItem } from "../../platform/storage";

const RELOAD_MARK_KEY = "hv.page-load-reload";
/** 同一页面在这段时间内只自动重载一次。 */
const RELOAD_GUARD_MILLIS = 30_000;
const MAX_RELOAD_MARKS = 8;

interface ReloadMark {
  readonly path: string;
  readonly at: number;
}

/** 是否应当重载，以及随之写入的标记。 */
export interface PageLoadReloadDecision {
  readonly reload: boolean;
  readonly mark: string;
}

/**
 * 纯判定：`mark` 记录最近重载过的路径与时间，旧格式和损坏的标记按没有记录处理。
 * 同一路径在保护时间内已经重载过则不再重载；时钟回拨（标记时刻晚于现在）按“刚重载过”处理。
 */
export function pageLoadReloadDecision(
  mark: string | null,
  routePath: string,
  now: number,
): PageLoadReloadDecision {
  const recent = readReloadMarks(mark).filter(({ at }) => now - at < RELOAD_GUARD_MILLIS);
  const reload = !recent.some(({ path }) => path === routePath);
  if (reload) {
    // 按写入顺序保留最近路径，切到其他页面不能覆盖仍受保护的路径。
    recent.push({ path: routePath, at: now });
  }
  return { reload, mark: JSON.stringify(recent.slice(-MAX_RELOAD_MARKS)) };
}

function readReloadMarks(mark: string | null): ReloadMark[] {
  if (mark === null) {
    return [];
  }
  const parsed = parseJson(mark);
  if (!parsed.ok || !Array.isArray(parsed.value) || parsed.value.length > MAX_RELOAD_MARKS) {
    return [];
  }
  const marks: ReloadMark[] = [];
  for (const entry of parsed.value) {
    const record = jsonExactObject(entry, ["path", "at"]);
    const path = record?.path;
    const at = record?.at;
    if (
      typeof path !== "string" ||
      !path.startsWith("/") ||
      /[?#]/.test(path) ||
      typeof at !== "number" ||
      !Number.isSafeInteger(at) ||
      at < 0 ||
      marks.some((mark) => mark.path === path)
    ) {
      return [];
    }
    marks.push({ path, at });
  }
  return marks;
}

/**
 * 路由页面的代码加载失败时调用。返回 true 表示已发起整页重载；返回 false 时调用方应显示错误横幅
 * （不在浏览器里、刚为同一路径重载过，或标记无法写入——没有标记就无法防止循环刷新）。
 */
export function reloadAfterPageLoadError(): boolean {
  if (typeof window === "undefined") {
    return false;
  }
  let routePath: string;
  try {
    routePath = window.location.pathname;
  } catch {
    return false;
  }
  // 防循环标记只跟随当前标签页，查询与片段不参与页面身份，也不进入存储。
  const decision = pageLoadReloadDecision(getSessionItem(RELOAD_MARK_KEY), routePath, Date.now());
  if (!decision.reload || !setSessionItem(RELOAD_MARK_KEY, decision.mark)) {
    return false;
  }
  try {
    window.location.reload();
  } catch {
    return false;
  }
  return true;
}
