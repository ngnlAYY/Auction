// 截断提示测试用的文档与节点替身。

import { flush } from "solid-js";

const NATIVE = "dialog:modal";
const OPEN = "dialog[open]";
const ARIA = '[role="dialog"][aria-modal="true"]';

/** 提供截断提示所需的元素几何、焦点、样式与节点身份。 */
export class ElementFixture extends EventTarget {
  readonly attributes = new Map<string, string>();
  readonly children: ElementFixture[] = [];
  readonly styleValues = new Map<string, string>();
  styleWrites = 0;
  readonly style = {
    setProperty: (key: string, value: string) => {
      this.styleWrites += 1;
      this.styleValues.set(key, value);
    },
  };
  readonly scrollWidth = 100;
  readonly clientWidth = 50;
  parentNode: ElementFixture | null = null;
  onHover: (() => void) | null = null;
  rectangle = { top: 80, bottom: 100, left: 10, right: 60, width: 50, height: 20 };
  rectangleReads = 0;
  overflow = { overflowX: "visible", overflowY: "visible" };
  styleReads = 0;

  constructor(
    readonly tag: string,
    private readonly fixture: DocumentFixture,
  ) {
    super();
  }

  get isConnected(): boolean {
    return this.fixture.body.contains(this);
  }

  get parentElement(): ElementFixture | null {
    return this.parentNode;
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
    if (this.fixture.activeElement !== null && child.contains(this.fixture.activeElement)) {
      this.fixture.activeElement = this.fixture.body;
    }
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
    this.rectangleReads += 1;
    return this.rectangle;
  }
}

/** 提供截断提示所需的文档查找、键盘事件与页面可见性。 */
export class DocumentFixture extends EventTarget {
  readonly body = new ElementFixture("body", this);
  activeElement: ElementFixture | null = null;
  modalSelector = true;
  hidden = false;

  setHidden(hidden: boolean): void {
    this.hidden = hidden;
    this.dispatchEvent(new Event("visibilitychange"));
    flush();
  }

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
