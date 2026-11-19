// 截断文本浮层的定位、显示与关闭：视口边界内放置，传送门渲染到 body，滚动或缩放时收起。

import { containingModal } from "../../../platform/a11y/escape";
import type { Cell } from "../../../reactive/cell";
import { structuralEquals } from "../../../reactive/equality";
import type { TooltipPosition, TooltipRequest } from ".";
import type { TooltipInteraction } from "./interaction";

/** 点击触发器时固定或取消固定浮层，不改变本次测量结果。 */
export function toggleTooltip(
  interaction: TooltipInteraction,
  element: Element | null,
  displayDiffers: boolean,
  truncated: Cell<boolean>,
  request: TooltipRequest,
): void {
  if (interaction.pinned.peek()) {
    interaction.close();
  } else {
    interaction.dismissed.set(false);
    showTooltip(element, displayDiffers, truncated, interaction, request);
    interaction.pinned.set(interaction.position.peek() !== null);
  }
}

/** 重测并更新截断状态；不再截断时立即收起浮层。 */
export function updateTruncatedState(
  element: Element,
  displayDiffers: boolean,
  truncated: Cell<boolean>,
  interaction: TooltipInteraction,
): boolean {
  const next = elementIsTruncated(element, displayDiffers);
  // 先归还焦点，随后才能移除入口的 tabindex。
  if (!next) interaction.dismiss();
  // 相同测量值不写信号，避免表格中每个文本在窗口与观察器重复通知时重渲染。
  const changed = truncated.peek() !== next;
  if (changed) {
    truncated.set(next);
  }
  return next;
}

/** 展示值不同或内容实际溢出都算截断。 */
export function elementIsTruncated(element: Element, displayDiffers: boolean): boolean {
  return displayDiffers || element.scrollWidth > element.clientWidth;
}

/** 在视口内计算位置并显示浮层；传送门失败时不留下可见位置。 */
export function showTooltip(
  element: Element | null,
  displayDiffers: boolean,
  truncated: Cell<boolean>,
  interaction: TooltipInteraction,
  request: TooltipRequest,
): void {
  if (element === null || !updateTruncatedState(element, displayDiffers, truncated, interaction)) {
    return;
  }

  const rectangle = element.getBoundingClientRect();
  const position = positionTooltip(rectangle, tooltipViewportBounds());
  if (
    request.portal &&
    !renderPortalTooltip(
      interaction.portal,
      element,
      request.value,
      request.tooltipId,
      request.className,
      position,
    )
  ) {
    return;
  }
  interaction.position.set(position);
  if (typeof window !== "undefined" && typeof window.requestAnimationFrame === "function") {
    const previousParent = element.parentElement;
    let applied = rectangle;
    let checked = rectangle;
    interaction.setAnchorTracking(
      trackAnchorFrames(window, () => {
        if (!element.isConnected) {
          interaction.dismiss(previousParent);
          return;
        }
        const current = element.getBoundingClientRect();
        const viewport = tooltipViewportBounds();
        const visible = intersectViewport(current, viewport);
        if (!hasArea(visible)) {
          interaction.dismiss();
          return;
        }
        // 相对实际采用的矩形比较，逐帧的小位移也能累积，半像素边界的抖动不会翻边。
        if (edgeDistance(current, applied) < 0.5 || edgeDistance(current, checked) === 0) return;
        if (!visibleThroughAncestors(element, visible)) {
          interaction.dismiss();
          return;
        }
        // 定位结果相同时保持应用基准，同时避免静止帧重复读取祖先样式。
        checked = current;
        const next = positionTooltip(current, viewport);
        if (!structuralEquals(next, interaction.position.peek())) {
          const popup = interaction.portal.peek();
          if (popup !== null) positionPortal(popup, next);
          interaction.position.set(next);
          applied = current;
        }
      }),
    );
  }
}

type Viewport = readonly [left: number, top: number, width: number, height: number];
type RectEdges = Pick<DOMRect, "left" | "top" | "right" | "bottom">;

function edgeDistance(current: RectEdges, previous: RectEdges): number {
  return Math.max(
    Math.abs(current.left - previous.left),
    Math.abs(current.top - previous.top),
    Math.abs(current.right - previous.right),
    Math.abs(current.bottom - previous.bottom),
  );
}

function intersectViewport(rectangle: RectEdges, [left, top, width, height]: Viewport): RectEdges {
  return {
    left: Math.max(rectangle.left, left),
    top: Math.max(rectangle.top, top),
    right: Math.min(rectangle.right, left + width),
    bottom: Math.min(rectangle.bottom, top + height),
  };
}

function hasArea({ left, top, right, bottom }: RectEdges): boolean {
  return right > left && bottom > top;
}

function visibleThroughAncestors(element: Element, visible: RectEdges): boolean {
  let { left, top, right, bottom } = visible;
  for (let ancestor = element.parentElement; ancestor !== null; ancestor = ancestor.parentElement) {
    const { overflowX, overflowY } = window.getComputedStyle(ancestor);
    const clipX = overflowX !== "visible";
    const clipY = overflowY !== "visible";
    if (!clipX && !clipY) continue;
    const clip = ancestor.getBoundingClientRect();
    if (clipX) {
      left = Math.max(left, clip.left);
      right = Math.min(right, clip.right);
    }
    if (clipY) {
      top = Math.max(top, clip.top);
      bottom = Math.min(bottom, clip.bottom);
    }
    if (right <= left || bottom <= top) return false;
  }
  return true;
}

function trackAnchorFrames(window: Window, update: () => void): () => void {
  const { document } = window;
  let frame: number | null = null;
  let active = true;
  const cancel = (): void => {
    if (frame !== null) window.cancelAnimationFrame(frame);
    frame = null;
  };
  const step = (): void => {
    frame = null;
    if (active && !document.hidden) {
      update();
      sync();
    }
  };
  const sync = (): void => {
    if (!active || document.hidden) {
      cancel();
    } else if (frame === null) {
      frame = window.requestAnimationFrame(step);
    }
  };
  document.addEventListener("visibilitychange", sync);
  sync();
  return () => {
    // 收起与卸载都先停止循环，隐藏期间也只保留可见性监听。
    active = false;
    cancel();
    document.removeEventListener("visibilitychange", sync);
  };
}

function positionTooltip(
  rectangle: DOMRect,
  [viewportLeft, viewportTop, viewportWidth, viewportHeight]: Viewport,
): TooltipPosition {
  const width = Math.min(360, Math.max(viewportWidth - 16, 1));
  const gap = 8;
  const margin = 8;
  const minimumTop = viewportTop + margin;
  const maximumTop = Math.max(viewportTop + viewportHeight - margin, minimumTop);
  const aboveTop = clamp(rectangle.top - gap, minimumTop, maximumTop);
  const belowTop = clamp(rectangle.bottom + gap, minimumTop, maximumTop);
  const availableAbove = Math.max(aboveTop - minimumTop, 1);
  const availableBelow = Math.max(maximumTop - belowTop, 1);
  const above = availableAbove > availableBelow;
  const minimumLeft = viewportLeft + margin;
  const maximumLeft = Math.max(viewportLeft + viewportWidth - width - margin, minimumLeft);
  return {
    left: clamp(rectangle.left, minimumLeft, maximumLeft),
    top: above ? aboveTop : belowTop,
    width,
    maximumHeight: above ? availableAbove : availableBelow,
    placement: above ? "above" : "below",
  };
}

/** 返回相对布局视口的可见区域；旧浏览器回退到完整窗口，不限制用户缩放。 */
export function tooltipViewportBounds(): Viewport {
  if (typeof window === "undefined") {
    return [0, 0, 1024, 768] as const;
  }
  const viewport = window.visualViewport;
  if (viewport != null) {
    const left = viewport.offsetLeft;
    const top = viewport.offsetTop;
    const width = viewport.width;
    const height = viewport.height;
    if ([left, top, width, height].every(Number.isFinite) && width > 0 && height > 0) {
      return [left, top, width, height] as const;
    }
  }
  const width = window.innerWidth;
  const height = window.innerHeight;
  return [
    0,
    0,
    Number.isFinite(width) && width > 0 ? width : 1024,
    Number.isFinite(height) && height > 0 ? height : 768,
  ] as const;
}

/** 清空可见位置并移除传送门节点，二者成对处理。 */
export function hideTooltip(
  tooltipPosition: Cell<TooltipPosition | null>,
  portalElement: Cell<HTMLElement | null>,
): void {
  const visible = tooltipPosition.peek() !== null;
  if (visible) {
    tooltipPosition.set(null);
  }
  const element = portalElement.peek();
  if (element !== null) {
    if (element.parentNode !== null) {
      try {
        element.parentNode.removeChild(element);
      } catch {
        // 节点已被其他清理路径移除时无需再次移除。
      }
    }
    portalElement.set(null);
  }
}

/** 构件卸载时无条件移除传送门节点。 */
export function removeTooltip(portalElement: Cell<HTMLElement | null>): void {
  const element = portalElement.peek();
  if (element !== null && element.parentNode !== null) {
    try {
      element.parentNode.removeChild(element);
    } catch {
      // 节点已被其他清理路径移除时无需再次移除。
    }
  }
  portalElement.set(null);
}

/** 在模态对话框或 body 中创建/复用 portal 浮层；失败返回 false。 */
export function renderPortalTooltip(
  portalElement: Cell<HTMLElement | null>,
  trigger: Element,
  value: string,
  tooltipId: string,
  tooltipClassName: string,
  position: TooltipPosition,
): boolean {
  if (typeof document === "undefined") {
    return false;
  }
  // 弹窗里的浮层留在所属焦点范围与层叠上下文内，避免 body portal 落到遮罩后面。
  const parent = containingModal(trigger) ?? document.body ?? null;
  if (parent === null) {
    return false;
  }
  const existing = portalElement.peek();
  let tooltip: HTMLElement;
  if (existing === null) {
    try {
      const element = document.createElement("span");
      if (!(element instanceof HTMLElement)) {
        return false;
      }
      tooltip = element;
    } catch {
      // 无法创建浮层时不改变当前可见状态。
      return false;
    }
  } else {
    tooltip = existing;
  }
  if (!(tooltip.parentNode?.isSameNode(parent) ?? false)) {
    try {
      parent.appendChild(tooltip);
    } catch {
      return false;
    }
  }
  tooltip.setAttribute("id", tooltipId);
  tooltip.setAttribute("class", tooltipClassName);
  tooltip.setAttribute("role", "tooltip");
  tooltip.setAttribute("tabindex", "0");
  tooltip.textContent = value;
  positionPortal(tooltip, position);
  portalElement.set(tooltip);
  return true;
}

function positionPortal(tooltip: HTMLElement, position: TooltipPosition): void {
  tooltip.setAttribute("data-placement", position.placement);
  const style = tooltip.style;
  style.setProperty("--truncated-text-tooltip-left", `${position.left}px`);
  style.setProperty("--truncated-text-tooltip-top", `${position.top}px`);
  style.setProperty("--truncated-text-tooltip-width", `${position.width}px`);
  style.setProperty("--truncated-text-tooltip-max-height", `${position.maximumHeight}px`);
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), maximum);
}
