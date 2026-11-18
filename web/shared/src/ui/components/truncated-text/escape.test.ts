// 挂载截断组件，由 DOM 替身驱动真实悬停、传送门、文档按键和焦点恢复。

import { expect, test } from "bun:test";
import { createRoot, flush } from "solid-js";
import { TruncatedText } from ".";

const NATIVE = "dialog:modal";
const OPEN = "dialog[open]";
const ARIA = '[role="dialog"][aria-modal="true"]';

class ElementFixture extends EventTarget {
  readonly attributes = new Map<string, string>();
  readonly children: ElementFixture[] = [];
  readonly style = { setProperty() {} };
  readonly scrollWidth = 100;
  readonly clientWidth = 50;
  parentNode: ElementFixture | null = null;
  onHover: (() => void) | null = null;

  constructor(
    readonly tag: string,
    private readonly fixture: DocumentFixture,
  ) {
    super();
  }

  get isConnected(): boolean {
    return this.fixture.body.contains(this);
  }

  matches(selector: string): boolean {
    if (selector === NATIVE) {
      if (!this.fixture.modalSelector) throw new SyntaxError("测试浏览器不支持 :modal");
      return this.tag === "dialog" && this.attributes.has("modal");
    }
    if (selector === OPEN) return this.tag === "dialog" && this.attributes.has("open");
    if (selector === ARIA) return this.attributes.get("aria-modal") === "true";
    if (selector === "a[href]") return this.tag === "a" && this.attributes.has("href");
    return selector === "button:not([disabled])" && this.tag === "button";
  }

  closest(selector: string): ElementFixture | null {
    return selector.split(",").some((part) => this.matches(part))
      ? this
      : (this.parentNode?.closest(selector) ?? null);
  }

  contains(other: ElementFixture): boolean {
    return this === other || this.children.some((child) => child.contains(other));
  }

  *descendants(): Iterable<ElementFixture> {
    for (const child of this.children) {
      yield child;
      yield* child.descendants();
    }
  }

  querySelectorAll(selector: string) {
    if (selector === NATIVE && !this.fixture.modalSelector) throw new SyntaxError("不支持 :modal");
    const nodes = [...this.descendants()].filter((node) => node.matches(selector));
    return Object.assign(nodes, { item: (index: number) => nodes[index] ?? null });
  }

  setAttribute(key: string, value: string): void {
    this.attributes.set(key, value);
  }

  appendChild(child: ElementFixture): void {
    child.parentNode?.removeChild(child);
    this.children.push(child);
    child.parentNode = this;
  }

  removeChild(child: ElementFixture): void {
    const index = this.children.indexOf(child);
    if (index >= 0) this.children.splice(index, 1);
    child.parentNode = null;
  }

  isSameNode(other: ElementFixture): boolean {
    return this === other;
  }

  focus(): void {
    this.fixture.activeElement = this;
  }

  getBoundingClientRect() {
    return { top: 80, bottom: 100, left: 10 };
  }
}

class DocumentFixture extends EventTarget {
  readonly body = new ElementFixture("body", this);
  activeElement: ElementFixture | null = null;
  modalSelector = true;

  createElement(tag: string): ElementFixture {
    return new ElementFixture(tag, this);
  }

  querySelectorAll(selector: string) {
    return this.body.querySelectorAll(selector);
  }

  getElementById(id: string): ElementFixture | null {
    return [...this.body.descendants()].find((node) => node.attributes.get("id") === id) ?? null;
  }

  escape(target: ElementFixture, isComposing = false): boolean {
    const event = new Event("keydown", { cancelable: true });
    Object.defineProperties(event, {
      key: { value: "Escape" },
      isComposing: { value: isComposing },
      target: { value: target },
    });
    this.dispatchEvent(event);
    flush();
    return event.defaultPrevented;
  }
}

function mountPage() {
  const dom = new DocumentFixture();
  const window = Object.assign(new EventTarget(), {
    document: dom,
    innerWidth: 800,
    innerHeight: 600,
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
