// 页面提交后的片段定位与历史滚动恢复。

/** 浏览器视口的滚动位置。 */
export interface ScrollPosition {
  readonly x: number;
  readonly y: number;
}

/** 每个历史条目的落点。 */
export type ScrollTarget =
  | { readonly kind: "position"; readonly position: ScrollPosition }
  | { readonly kind: "fragment"; readonly id: string };

/** 用 HTML 的 ID 查找片段，避免把地址里的任意字符当成 CSS 选择器。 */
export function fragmentTarget(hash: string): ScrollTarget | null {
  const fragment = hash.slice(1);
  if (fragment === "") {
    return null;
  }
  try {
    return { kind: "fragment", id: decodeURIComponent(fragment) };
  } catch {
    return { kind: "fragment", id: fragment };
  }
}

/** 已挂载的片段交给浏览器；只补原生导航时还找不到的目标。 */
export function hasFragmentTarget(id: string): boolean {
  return typeof document !== "undefined" && document.getElementById(id) !== null;
}

/** 视口是否仍在同一物理坐标。 */
export function sameScrollPosition(left: ScrollPosition, right: ScrollPosition): boolean {
  return left.x === right.x && left.y === right.y;
}

function scrollLimit(): ScrollPosition | null {
  const root = typeof document === "undefined" ? null : document.scrollingElement;
  return root === null
    ? null
    : {
        x: Math.max(root.scrollWidth - root.clientWidth, 0),
        y: Math.max(root.scrollHeight - root.clientHeight, 0),
      };
}

function applyScroll(target: ScrollTarget, performScroll: (scroll: () => void) => void): boolean {
  if (target.kind === "fragment") {
    if (typeof document === "undefined") {
      return false;
    }
    const element = document.getElementById(target.id);
    if (element === null || element.getClientRects().length === 0) {
      return false;
    }
    performScroll(() =>
      element.scrollIntoView({ block: "start", inline: "nearest", behavior: "instant" }),
    );
    return true;
  }
  const { x, y } = target.position;
  const limit = scrollLimit();
  if (limit !== null && (x > limit.x + 1 || y > limit.y + 1)) {
    // 异步列表可能还没有撑开文档；不能把被浏览器截短的位置记成目标。
    return false;
  }
  performScroll(() => window.scrollTo({ left: x, top: y, behavior: "instant" }));
  return true;
}

/** 在提交后的帧等待落点可达；取消时释放等待中的帧和观察器。 */
export function restoreScroll(
  target: ScrollTarget,
  performScroll: (scroll: () => void) => void,
  onRestored: () => void,
): () => void {
  const root = typeof document === "undefined" ? null : document.body;
  let frame: number | null = null;
  let mutations: MutationObserver | null = null;
  let resize: ResizeObserver | null = null;
  let stopped = false;
  const stop = (): void => {
    stopped = true;
    if (frame !== null) {
      window.cancelAnimationFrame(frame);
      frame = null;
    }
    mutations?.disconnect();
    resize?.disconnect();
  };
  const queue = (): void => {
    if (!stopped && frame === null) {
      frame = window.requestAnimationFrame(attempt);
    }
  };
  const attempt = (): void => {
    frame = null;
    if (stopped) {
      return;
    }
    if (applyScroll(target, performScroll)) {
      stop();
      onRestored();
      return;
    }
    if (root === null || mutations !== null) {
      return;
    }
    mutations = new MutationObserver(queue);
    mutations.observe(root, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["id", "hidden"],
    });
    if (target.kind === "position" && typeof ResizeObserver === "function") {
      resize = new ResizeObserver(queue);
      resize.observe(root);
    }
  };
  queue();
  return stop;
}
