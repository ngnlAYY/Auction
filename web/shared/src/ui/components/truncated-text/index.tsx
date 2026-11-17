// 长文本截断展示与可操作浮层；悬停、聚焦和点击共享同一份完整文本。

import type { JSX } from "@solidjs/web";
import { createEffect, onCleanup, onSettled, Show } from "solid-js";

import { createCell } from "../../../reactive/cell";
import { createValueMemo } from "../../../reactive/memo";
import { createTaskScope } from "../../../reactive/tasks";
import {
  attachPopupListeners,
  createTooltipInteraction,
  type PopupListeners,
  type TooltipInteraction,
} from "./interaction";
import { type MeasureSubscription, observe as observeResize } from "./resize";
import { removeTooltip, showTooltip, toggleTooltip, updateTruncatedState } from "./tooltip";

let nextTooltipId = 1;

/** 浮层相对布局视口的定位与最大高度。 */
export interface TooltipPosition {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly maximumHeight: number;
  readonly placement: "above" | "below";
}

/** 浮层文本、唯一 id、class 与是否使用传送门的请求。 */
export interface TooltipRequest {
  readonly value: string;
  readonly tooltipId: string;
  readonly className: string;
  readonly portal: boolean;
}

interface TooltipDismissHandle {
  readonly window: Window;
  readonly viewport: VisualViewport | null;
  readonly resizeCallback: () => void;
  readonly scrollCallback: (event: Event) => void;
  popupListeners: PopupListeners | null;
  dispose(): void;
}

/** 截断文本的完整值、展示值、尺寸、可聚焦性与浮层模式。 */
export interface TruncatedTextProps {
  readonly value: string;
  readonly display?: string | null;
  readonly size?: string | null;
  readonly className?: string | null;
  readonly focusable?: boolean | null;
  readonly portal?: boolean | null;
  readonly legacyDom?: boolean | null;
}

/**
 * 展示可能很长的标识或 metadata，并在确实发生截断时提供可聚焦 tooltip。
 * `display` 用于调用方已提供缩写文本的场景，展示值与完整值不同也视为截断。浏览器端用
 * `ResizeObserver` 重测宽度，响应式变化后仍只把真正需要提示的文本加入 Tab 顺序；当
 * `focusable=false` 时内联浮层也不加入 Tab 顺序，避免在按钮或链接中产生嵌套焦点。
 * legacy DOM 模式省略 `title` 降级，只输出旧版显式 tooltip。
 */
export function TruncatedText(props: TruncatedTextProps): JSX.Element {
  const displayDiffers = createValueMemo(
    () => props.display != null && props.display !== props.value,
  );
  const display = createValueMemo(() => props.display ?? props.value);
  const size = createValueMemo(() =>
    props.size === "compact" ? "compact" : props.size === "wide" ? "wide" : "medium",
  );
  const className = createValueMemo(() => props.className ?? "");
  const focusable = createValueMemo(() => props.focusable ?? true);
  const portal = createValueMemo(() => props.portal ?? false);
  const legacyDom = createValueMemo(() => props.legacyDom ?? false);
  const baseTriggerClass = createValueMemo(() =>
    legacyDom()
      ? ["ui-truncated-text", `ui-truncated-text--${size()}`]
      : ["shared-truncated-text", `shared-truncated-text--${size()}`],
  );
  const tooltipClassName = createValueMemo(() =>
    legacyDom() ? "ui-truncated-text__tooltip" : "shared-truncated-text__tooltip",
  );
  const title = createValueMemo(() => (legacyDom() ? undefined : props.value));
  const dataFullValue = createValueMemo(() => (legacyDom() ? undefined : props.value));

  const truncated = createCell(false);
  const focused = createCell(false);
  const hovered = createCell(false);
  const popupHovered = createCell(false);
  const popupFocused = createCell(false);
  const pinned = createCell(false);
  const dismissed = createCell(false);
  const closeRevision = createCell(0);
  const tooltipPosition = createCell<TooltipPosition | null>(null);
  const tooltipId = `shared-truncated-tooltip-${nextTooltipId}`;
  nextTooltipId += 1;

  const mountedElement = createCell<Element | null>(null);
  let triggerElement: Element | null = null;
  const measureSubscription = createCell<MeasureSubscription | null>(null);
  const portalElement = createCell<HTMLElement | null>(null);
  onCleanup(() => removeTooltip(portalElement));
  const tasks = createTaskScope();
  let dismissHandle: TooltipDismissHandle | null = null;
  onCleanup(() => {
    dismissHandle?.dispose();
    dismissHandle = null;
  });
  const interaction: TooltipInteraction = createTooltipInteraction(
    {
      position: tooltipPosition,
      focused,
      hovered,
      popupHovered,
      popupFocused,
      pinned,
      dismissed,
      closeRevision,
      portal: portalElement,
    },
    tasks,
  );

  // 固定宽度文本变化不触发 ResizeObserver；内容提交到 DOM 后重测，且不保留旧 portal 正文。
  const contentDependency = createValueMemo(() => [props.value, display()] as const);
  createEffect(contentDependency, () => {
    const element = mountedElement.peek();
    if (element === null) {
      return;
    }
    if (interaction.position.peek() !== null) {
      interaction.close();
    }
    updateTruncatedState(element, displayDiffers(), truncated, tooltipPosition, portalElement);
  });
  // ref 回调发生在元素插入文档之前；挂载后的首次测量与尺寸观察统一推迟到布局完成。
  onSettled(() => {
    const element = triggerElement;
    if (element === null) {
      return;
    }
    mountedElement.set(element);
    updateTruncatedState(element, displayDiffers(), truncated, tooltipPosition, portalElement);

    // 元素尺寸与窗口尺寸变化共用全页唯一的监听，只登记本实例的重测回调。
    const subscription = observeResize(element, () => {
      updateTruncatedState(element, displayDiffers(), truncated, tooltipPosition, portalElement);
    });
    measureSubscription.set(subscription);
    return () => {
      const current = measureSubscription.peek();
      measureSubscription.set(null);
      current?.dispose();
    };
  });
  // 原生 DOM 回调没有 Solid 上下文；组件创建的任务作用域让延迟任务随组件卸载取消。
  const deferClose = (): void => interaction.deferClose();
  const tooltipVisibility = createValueMemo(() => tooltipPosition());
  createEffect(tooltipVisibility, (position) => {
    if (position === null) {
      dismissHandle?.dispose();
      dismissHandle = null;
      interaction.pinned.set(false);
      return;
    }
    if (dismissHandle !== null || typeof window === "undefined") {
      return;
    }

    const resizeCallback = (): void => interaction.close();
    const popupId = tooltipId;
    const scrollCallback = (event: Event): void => {
      // 浮层自身的滚动必须保留内容；页面或外层容器滚动后关闭以免定位过期。
      const target = event.target instanceof Node ? event.target : null;
      const popup = window.document.getElementById(popupId);
      const insidePopup = target !== null && (popup?.contains(target) ?? false);
      if (!insidePopup) {
        interaction.close();
      }
    };

    let resizeAdded = false;
    let scrollAdded = false;
    try {
      window.addEventListener("resize", resizeCallback);
      resizeAdded = true;
    } catch {
      resizeAdded = false;
    }
    try {
      window.addEventListener("scroll", scrollCallback, true);
      scrollAdded = true;
    } catch {
      scrollAdded = false;
    }
    if (!resizeAdded || !scrollAdded) {
      if (resizeAdded) {
        window.removeEventListener("resize", resizeCallback);
      }
      if (scrollAdded) {
        window.removeEventListener("scroll", scrollCallback, true);
      }
      return;
    }

    // 手机软键盘与页面缩放只改变可见视口时，window 不一定派发 resize/scroll。
    // 沿用窗口变动时关闭浮层的策略，避免保留已经偏离触发文本的旧坐标。
    const viewport = window.visualViewport ?? null;
    const handle: TooltipDismissHandle = {
      window,
      viewport,
      resizeCallback,
      scrollCallback,
      popupListeners: null,
      dispose() {
        removeTooltipDismissHandle(this);
      },
    };
    if (viewport !== null) {
      let viewportResizeAdded = false;
      let viewportScrollAdded = false;
      try {
        viewport.addEventListener("resize", resizeCallback);
        viewportResizeAdded = true;
      } catch {
        viewportResizeAdded = false;
      }
      try {
        viewport.addEventListener("scroll", scrollCallback);
        viewportScrollAdded = true;
      } catch {
        viewportScrollAdded = false;
      }
      if (!viewportResizeAdded || !viewportScrollAdded) {
        removeTooltipDismissHandle(handle);
        return;
      }
    }
    const popup = window.document.getElementById(popupId);
    handle.popupListeners =
      popup === null
        ? null
        : attachPopupListeners(popup, `${popupId}-trigger`, interaction, deferClose);
    dismissHandle = handle;
  });

  const describedBy = (): string | undefined =>
    tooltipPosition() !== null ? tooltipId : undefined;
  const tabindex = (): string | undefined => (focusable() && truncated() ? "0" : undefined);
  const role = (): "button" | undefined => (focusable() && truncated() ? "button" : undefined);
  const ariaExpanded = (): "true" | "false" | undefined => {
    if (!(focusable() && truncated())) {
      return undefined;
    }
    return tooltipPosition() !== null ? "true" : "false";
  };
  const dataTruncated = (): string | undefined => (truncated() ? "true" : undefined);
  const request = (): TooltipRequest => ({
    value: props.value,
    tooltipId,
    className: tooltipClassName(),
    portal: portal(),
  });

  return (
    <>
      {/* biome-ignore lint/a11y/noStaticElementInteractions: 截断时角色与键盘处理随状态启用，未截断时保持静态文本。 */}
      {/* biome-ignore lint/a11y/useAriaPropsSupportedByRole: aria-expanded 只在可聚焦的 button 角色下输出。 */}
      <span
        id={`${tooltipId}-trigger`}
        class={[baseTriggerClass(), className()]}
        title={title()}
        tabindex={tabindex()}
        aria-describedby={describedBy()}
        role={role()}
        aria-expanded={ariaExpanded()}
        data-truncated={dataTruncated()}
        data-full-value={dataFullValue()}
        ref={(element) => {
          triggerElement = element;
        }}
        onMouseEnter={() => {
          interaction.hovered.set(true);
          interaction.dismissed.set(false);
          showTooltip(
            mountedElement.peek(),
            displayDiffers(),
            truncated,
            tooltipPosition,
            portalElement,
            request(),
          );
        }}
        onMouseLeave={() => {
          interaction.hovered.set(false);
          interaction.deferClose();
        }}
        onFocus={() => {
          focused.set(true);
          if (!interaction.dismissed.peek()) {
            showTooltip(
              mountedElement.peek(),
              displayDiffers(),
              truncated,
              tooltipPosition,
              portalElement,
              request(),
            );
          }
        }}
        onBlur={() => {
          focused.set(false);
          interaction.dismissed.set(false);
          interaction.deferClose();
        }}
        onClick={(event) => {
          if (focusable() && truncated()) {
            event.stopPropagation();
            toggleTooltip(
              interaction,
              mountedElement.peek(),
              displayDiffers(),
              truncated,
              request(),
            );
          }
        }}
        onKeyDown={(event) => {
          if (event.key === "Escape" && tooltipPosition() !== null) {
            event.stopPropagation();
            event.preventDefault();
            interaction.close();
          } else if (focusable() && truncated() && (event.key === "Enter" || event.key === " ")) {
            event.preventDefault();
            event.stopPropagation();
            toggleTooltip(
              interaction,
              mountedElement.peek(),
              displayDiffers(),
              truncated,
              request(),
            );
          }
        }}
      >
        {display()}
      </span>
      <Show when={!portal() ? tooltipPosition() : null}>
        {(position) => (
          <span
            id={tooltipId}
            class={tooltipClassName()}
            role="tooltip"
            tabindex={focusable() ? "0" : undefined}
            data-placement={position().placement}
            style={{
              "--truncated-text-tooltip-left": `${position().left}px`,
              "--truncated-text-tooltip-top": `${position().top}px`,
              "--truncated-text-tooltip-width": `${position().width}px`,
              "--truncated-text-tooltip-max-height": `${position().maximumHeight}px`,
            }}
          >
            {props.value}
          </span>
        )}
      </Show>
    </>
  );
}

function removeTooltipDismissHandle(handle: TooltipDismissHandle): void {
  try {
    handle.window.removeEventListener("resize", handle.resizeCallback);
  } catch {
    // 窗口已不可用时没有可释放的监听。
  }
  try {
    handle.window.removeEventListener("scroll", handle.scrollCallback, true);
  } catch {
    // 窗口已不可用时没有可释放的监听。
  }
  if (handle.viewport !== null) {
    handle.viewport.removeEventListener("resize", handle.resizeCallback);
    handle.viewport.removeEventListener("scroll", handle.scrollCallback);
  }
  handle.popupListeners?.dispose();
  handle.popupListeners = null;
}
