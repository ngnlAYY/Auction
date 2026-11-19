// 挂载截断组件，由 DOM 替身驱动真实悬停、传送门、文档按键和焦点恢复。

import { expect, test } from "bun:test";
import { createRoot, flush } from "solid-js";
import { createCell } from "../../../reactive/cell";
import { createTaskScope } from "../../../reactive/tasks";
import type { TooltipPosition } from ".";
import { TruncatedText } from ".";
import { createTooltipInteraction } from "./interaction";
import { DocumentFixture, ElementFixture } from "./test-document";
import { showTooltip } from "./tooltip";

function mountPage() {
  const dom = new DocumentFixture();
  const frames = new Map<number, FrameRequestCallback>();
  let nextFrame = 0;
  const window = Object.assign(new EventTarget(), {
    document: dom,
    innerWidth: 800,
    innerHeight: 600,
    requestAnimationFrame(callback: FrameRequestCallback): number {
      const id = nextFrame;
      nextFrame += 1;
      frames.set(id, callback);
      return id;
    },
    cancelAnimationFrame(id: number): void {
      frames.delete(id);
    },
    getComputedStyle(element: ElementFixture) {
      element.styleReads += 1;
      return element.overflow;
    },
  });
  const globals = {
    document: dom,
    window,
    Node: ElementFixture,
    Element: ElementFixture,
    HTMLElement: ElementFixture,
  };
  const previous = Object.keys(globals).map(
    (key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)] as const,
  );
  for (const [key, value] of Object.entries(globals)) {
    Object.defineProperty(globalThis, key, { configurable: true, value });
  }
  const disposers: (() => void)[] = [];
  const element = (tag: string, parent = dom.body) => {
    const child = dom.createElement(tag);
    parent.appendChild(child);
    return child;
  };
  return {
    dom,
    element,
    pendingFrames: () => frames.size,
    advanceFrame() {
      for (const id of [...frames.keys()]) {
        const callback = frames.get(id);
        frames.delete(id);
        callback?.(0);
      }
      flush();
    },
    tooltip(parent = dom.body) {
      const link = element("a", parent);
      link.setAttribute("href", "/equipment");
      const trigger = element("span", link);
      const jsx = Object.getOwnPropertyDescriptor(globalThis, "jsx");
      Object.defineProperty(globalThis, "jsx", {
        configurable: true,
        value(tag: unknown, attributes: Record<string, unknown> | null) {
          if (tag === "span" && typeof attributes?.ref === "function") {
            trigger.setAttribute("id", String(attributes.id));
            attributes.ref(trigger);
            const hover = attributes.onMouseEnter;
            if (typeof hover === "function") trigger.onHover = () => hover();
          }
          return null;
        },
      });
      try {
        disposers.push(
          createRoot((dispose) => {
            TruncatedText({
              value: "Legendary Cotton Robe",
              display: "Robe",
              portal: true,
              focusable: false,
              anchorOrder: 3,
            });
            return dispose;
          }),
        );
        flush();
      } finally {
        if (jsx === undefined) Reflect.deleteProperty(globalThis, "jsx");
        else Object.defineProperty(globalThis, "jsx", jsx);
      }
      const popupId = trigger.attributes.get("id")?.replace(/-trigger$/u, "");
      if (popupId === undefined || trigger.onHover === null) throw new Error("缺少提示触发组件");
      return {
        link,
        trigger,
        popup: () => dom.getElementById(popupId),
        hover() {
          trigger.onHover?.();
          flush();
          const popup = dom.getElementById(popupId);
          if (popup === null) throw new Error("提示浮层未挂载");
          return popup;
        },
      };
    },
    dispose() {
      for (const dispose of disposers) dispose();
      flush();
      for (const [key, descriptor] of previous) {
        if (descriptor === undefined) Reflect.deleteProperty(globalThis, key);
        else Object.defineProperty(globalThis, key, descriptor);
      }
    },
  };
}

test("悬停提示响应外部焦点的 Escape，浮层有焦点时还给链接", () => {
  const page = mountPage();
  try {
    const outside = page.element("button");
    const hint = page.tooltip();
    outside.focus();
    hint.hover();
    expect(page.dom.escape(outside, true)).toBe(false);
    expect(hint.popup()).not.toBeNull();
    expect(page.dom.escape(outside)).toBe(true);
    expect(hint.popup()).toBeNull();
    expect(page.dom.activeElement).toBe(outside);
    const popup = hint.hover();
    popup.focus();
    expect(page.dom.escape(popup)).toBe(true);
    expect(hint.popup()).toBeNull();
    expect(page.dom.activeElement).toBe(hint.link);
    expect(page.dom.escape(hint.link)).toBe(false);
  } finally {
    page.dispose();
  }
});

test("展开的提示跟随同序号锚点，静止和亚像素抖动不写位置", () => {
  const page = mountPage();
  try {
    const hint = page.tooltip();
    expect(page.pendingFrames()).toBe(0);
    const popup = hint.hover();
    const top = "--truncated-text-tooltip-top";
    expect(popup.styleValues.get(top)).toBe("108px");
    expect(page.pendingFrames()).toBe(1);
    const writes = popup.styleWrites;
    const reads = hint.trigger.rectangleReads;
    page.advanceFrame();
    hint.trigger.rectangle = { ...hint.trigger.rectangle, top: 80.1, bottom: 100.1 };
    page.advanceFrame();
    expect(hint.trigger.rectangleReads).toBe(reads + 2);
    expect(popup.styleWrites).toBe(writes);
    expect(page.pendingFrames()).toBe(1);

    popup.focus();
    hint.trigger.rectangle = { ...hint.trigger.rectangle, top: 140, bottom: 160 };
    page.advanceFrame();
    expect(hint.popup()).toBe(popup);
    expect(popup.styleValues.get(top)).toBe("168px");
    expect(page.dom.activeElement).toBe(popup);

    page.dom.setHidden(true);
    hint.trigger.rectangle = { ...hint.trigger.rectangle, top: 170, bottom: 190 };
    expect(page.pendingFrames()).toBe(0);
    page.advanceFrame();
    expect(popup.styleValues.get(top)).toBe("168px");
    page.dom.setHidden(false);
    expect(page.pendingFrames()).toBe(1);
    page.advanceFrame();
    expect(popup.styleValues.get(top)).toBe("198px");

    expect(page.dom.escape(popup)).toBe(true);
    expect(page.pendingFrames()).toBe(0);
    page.dom.setHidden(true);
    page.dom.setHidden(false);
    expect(page.pendingFrames()).toBe(0);
    hint.hover();
    expect(page.pendingFrames()).toBe(1);
  } finally {
    page.dispose();
  }
  expect(page.pendingFrames()).toBe(0);
  page.dom.setHidden(true);
  page.dom.setHidden(false);
  expect(page.pendingFrames()).toBe(0);
});

test("静止与跨半像素边界的抖动都不写状态，放置方向保持不变", () => {
  const page = mountPage();
  try {
    createRoot((dispose) => {
      const trigger = page.element("span");
      if (!(trigger instanceof Element)) throw new Error("缺少 DOM 替身");
      const move = (top: number): void => {
        trigger.rectangle = { ...trigger.rectangle, top, bottom: top + 41, height: 41 };
      };
      move(279.49);
      let writes = 0;
      const countedCell = <T>(value: T) => {
        const cell = createCell(value);
        const set = cell.set;
        cell.set = (next) => {
          writes += 1;
          set(next);
        };
        return cell;
      };
      const interaction = createTooltipInteraction(
        {
          position: countedCell<TooltipPosition | null>(null),
          focused: countedCell(false),
          hovered: countedCell(false),
          popupHovered: countedCell(false),
          popupFocused: countedCell(false),
          pinned: countedCell(false),
          dismissed: countedCell(false),
          closeRevision: countedCell(0),
          portal: countedCell<HTMLElement | null>(null),
        },
        createTaskScope(),
        "stationary-tooltip",
      );
      try {
        showTooltip(trigger, true, countedCell(false), interaction, {
          value: "Full text",
          tooltipId: "stationary-tooltip",
          className: "shared-truncated-text__tooltip",
          portal: false,
        });
        const opened = writes;
        const position = interaction.position.peek();
        expect(position).toMatchObject({ top: 328.49, placement: "below" });
        expect(opened).toBeGreaterThan(0);
        expect(page.pendingFrames()).toBe(1);
        for (const top of [279.49, 279.51, 279.49, 279.51, 279.79, 279.98]) {
          move(top);
          page.advanceFrame();
          expect(interaction.position.peek()).toBe(position);
          expect(writes).toBe(opened);
        }
        move(280);
        page.advanceFrame();
        expect(interaction.position.peek()).toMatchObject({ top: 272, placement: "above" });
        expect(writes).toBe(opened + 1);
      } finally {
        interaction.setAnchorTracking(null);
        dispose();
      }
    });
  } finally {
    page.dispose();
  }
});

test("锚点仍在视口内但被单轴裁剪祖先完全挡住时收起并还焦", () => {
  const page = mountPage();
  try {
    const clip = page.element("section");
    clip.overflow.overflowX = "clip";
    clip.rectangle = { left: 0, right: 100, top: 150, bottom: 200, width: 100, height: 50 };
    const hint = page.tooltip(clip);
    const move = (left: number): void => {
      hint.trigger.rectangle = { ...hint.trigger.rectangle, left, right: left + 50 };
    };
    move(1);
    const popup = hint.hover();
    popup.focus();

    page.advanceFrame();
    expect(clip.styleReads).toBe(0);
    move(2);
    page.advanceFrame();
    expect(hint.popup()).toBe(popup);
    expect(page.dom.activeElement).toBe(popup);
    const reads = clip.styleReads;
    page.advanceFrame();
    expect(clip.styleReads).toBe(reads);

    move(100);
    page.advanceFrame();
    expect(hint.popup()).toBeNull();
    expect(reads).toBe(1);
    expect(page.dom.activeElement).toBe(hint.link);
    expect(page.pendingFrames()).toBe(0);
  } finally {
    page.dispose();
  }
});

test.each([
  { loss: "detached", popupFocused: true, expectedFocus: "link" },
  { loss: "detached row", popupFocused: true, expectedFocus: "body" },
  { loss: "detached", popupFocused: false, expectedFocus: "outside" },
  { loss: "empty", popupFocused: true, expectedFocus: "link" },
  { loss: "offscreen", popupFocused: false, expectedFocus: "outside" },
] as const)("锚点失效时关闭提示并按焦点归属还焦 $loss $popupFocused", (scenario) => {
  const page = mountPage();
  try {
    const outside = page.element("button");
    const hint = page.tooltip();
    const popup = hint.hover();
    (scenario.popupFocused ? popup : outside).focus();
    expect(page.pendingFrames()).toBe(1);

    if (scenario.loss === "detached") hint.link.removeChild(hint.trigger);
    if (scenario.loss === "detached row") page.dom.body.removeChild(hint.link);
    if (scenario.loss === "empty") {
      hint.trigger.rectangle = { ...hint.trigger.rectangle, right: 10, width: 0 };
    }
    if (scenario.loss === "offscreen") {
      hint.trigger.rectangle = { ...hint.trigger.rectangle, top: 600, bottom: 620 };
    }
    page.advanceFrame();
    expect(hint.popup()).toBeNull();
    const focusTargets = { link: hint.link, outside, body: page.dom.body };
    expect(page.dom.activeElement).toBe(focusTargets[scenario.expectedFocus]);
    expect(page.pendingFrames()).toBe(0);
  } finally {
    page.dispose();
  }
});

test.each(["aria", "native", "fallback"] as const)(
  "仅关闭前景模态 %s 中的提示并把下一次 Escape 留给模态",
  (kind) => {
    const page = mountPage();
    try {
      const background = page.tooltip();
      background.hover();
      const foreground = page.element(kind === "aria" ? "section" : "dialog");
      if (kind === "aria") foreground.setAttribute("aria-modal", "true");
      else {
        foreground.setAttribute("open", "");
        foreground.setAttribute("modal", "");
      }
      page.dom.modalSelector = kind !== "fallback";
      // 原生顶层优先于文档中更晚出现的手工对话框。
      if (kind !== "aria") {
        const aria = page.element("section");
        aria.setAttribute("aria-modal", "true");
        page.tooltip(aria).hover();
      }
      const other = page.element("button", foreground);
      other.focus();
      expect(page.dom.escape(other)).toBe(false);
      expect(background.popup()).not.toBeNull();
      const frontHint = page.tooltip(foreground);
      const popup = frontHint.hover();
      expect(popup.parentNode).toBe(foreground);
      expect(page.dom.escape(background.link)).toBe(false);
      expect(frontHint.popup()).toBe(popup);
      popup.focus();
      expect(page.dom.escape(popup)).toBe(true);
      expect(frontHint.popup()).toBeNull();
      expect(page.dom.activeElement).toBe(frontHint.link);
      expect(page.dom.escape(frontHint.link)).toBe(false);
      expect(background.popup()).not.toBeNull();
    } finally {
      page.dispose();
    }
  },
);

test("同一范围依次关闭最近显示的长文本提示", () => {
  const page = mountPage();
  try {
    const first = page.tooltip();
    const last = page.tooltip();
    first.hover();
    last.hover();
    const outside = page.element("button");
    expect(page.dom.escape(outside)).toBe(true);
    expect(last.popup()).toBeNull();
    expect(first.popup()).not.toBeNull();
    expect(page.dom.escape(outside)).toBe(true);
    expect(first.popup()).toBeNull();
  } finally {
    page.dispose();
  }
});

test.each([true, false])(
  "原生模态内的应用对话框独占提示的 Escape（支持 :modal %s）",
  (supported) => {
    const page = mountPage();
    try {
      page.dom.modalSelector = supported;
      const native = page.element("dialog");
      native.setAttribute("open", "");
      native.setAttribute("modal", "");
      const outer = page.tooltip(native);
      outer.hover();
      const inner = page.element("section", native);
      inner.setAttribute("role", "dialog");
      inner.setAttribute("aria-modal", "true");
      const button = page.element("button", inner);
      button.focus();
      expect(page.dom.escape(button)).toBe(false);
      expect(outer.popup()).not.toBeNull();

      const hint = page.tooltip(inner);
      const popup = hint.hover();
      expect(popup.parentNode === inner).toBe(true);
      popup.focus();
      expect(page.dom.escape(popup)).toBe(true);
      expect(hint.popup()).toBeNull();
      expect(page.dom.activeElement).toBe(hint.link);
      expect(page.dom.escape(hint.link)).toBe(false);
      expect(outer.popup()).not.toBeNull();

      native.removeChild(inner);
      outer.link.focus();
      expect(page.dom.escape(outer.link)).toBe(true);
      expect(outer.popup()).toBeNull();
    } finally {
      page.dispose();
    }
  },
);

test("支持 :modal 时，前面的非模态 dialog 不会遮住页面提示的 Escape", () => {
  const page = mountPage();
  try {
    const hint = page.tooltip();
    hint.hover();
    const dialog = page.element("dialog");
    dialog.setAttribute("open", "");
    const outside = page.element("button", dialog);
    outside.focus();
    expect(page.dom.escape(outside)).toBe(true);
    expect(hint.popup()).toBeNull();
    expect(page.dom.activeElement).toBe(outside);
  } finally {
    page.dispose();
  }
});
