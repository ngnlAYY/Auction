// Admin 渲染测试用的节点、文档与控件事件，查询时保留响应式槽位中的真实节点。

import { flush } from "solid-js";
import { cleanupScope, preserveProperty } from "../test-scope-fixture";

type NodeMutation = "insert" | "remove";

/** 内存节点记录渲染写入、父子关系及焦点；控件直接赋值与渲染写入可分别观察。 */
export class TestElement {
  readonly nodes: (() => unknown)[] = [];
  readonly attributes: Record<string, unknown>;
  readonly propWrites: { readonly key: string; readonly value: unknown }[];
  parentElement: TestElement | null = null;
  focusCount = 0;
  text = "";
  private optionSelected = false;
  private optionSelectionDirty = false;
  private pendingSelectValue: string | null = null;

  constructor(
    readonly tag: string,
    attributes: Record<string, unknown> = {},
  ) {
    this.attributes = { ...attributes };
    this.propWrites = Object.entries(attributes).map(([key, value]) => ({ key, value }));
    this.optionSelected = tag === "option" && this.hasAttribute("selected");
  }

  get tagName(): string {
    return this.tag.toUpperCase();
  }

  get id(): string {
    return this.getAttribute("id") ?? "";
  }

  get value(): string {
    if (this.tag === "select") {
      return this.selectOptions().find((option) => option.selected)?.value ?? "";
    }
    const value = this.attributes.value;
    return typeof value === "string" ? value : this.tag === "option" ? this.textContent : "";
  }

  set value(value: string) {
    if (this.tag === "select") {
      this.pendingSelectValue = null;
      this.selectValue(value);
    } else {
      this.attributes.value = value;
    }
  }

  /** 当前选中状态独立于 `selected` 内容属性所表示的初始值。 */
  get selected(): boolean {
    return this.optionSelected;
  }

  set selected(value: boolean) {
    this.optionSelectionDirty = true;
    this.setOptionSelected(value);
  }

  private setOptionSelected(value: boolean): void {
    this.optionSelected = value;
    this.selectOwner()?.resetSelection([this]);
  }

  private selectOwner(): TestElement | null {
    const parent = this.parentElement;
    const select = parent?.tag === "optgroup" ? parent.parentElement : parent;
    return select?.tag === "select" ? select : null;
  }

  private selectOptions(): TestElement[] {
    return this.querySelectorAll("option").filter((option) => option.selectOwner() === this);
  }

  private selectValue(value: string): boolean {
    const options = this.selectOptions();
    const match = options.find((option) => option.value === value);
    for (const option of options) option.optionSelected = option === match;
    if (match === undefined) return false;
    match.optionSelectionDirty = true;
    return true;
  }

  private resetSelection(preferred: readonly TestElement[]): void {
    if (this.hasAttribute("multiple")) return;
    const options = this.selectOptions();
    const selected =
      options.findLast(
        (option) =>
          option.selected &&
          (preferred.includes(option) ||
            (option.parentElement?.tag === "optgroup" && preferred.includes(option.parentElement))),
      ) ?? options.findLast((option) => option.selected);
    if (selected !== undefined) {
      for (const option of options) option.optionSelected = option === selected;
      return;
    }

    const size = Number.parseInt(this.getAttribute("size") ?? "", 10);
    if (size !== 1 && size >= 0) return;
    const first = options.find(
      (option) =>
        !option.disabled &&
        !(option.parentElement?.tag === "optgroup" && option.parentElement.disabled),
    );
    if (first !== undefined) first.optionSelected = true;
  }

  /** 选项增删时同步单选状态，新增的显式选中项优先于已有选项。 */
  optionsChanged(inserted: readonly TestElement[] = []): void {
    if (this.tag !== "select") return;
    this.resetSelection(inserted);
    this.applySelectBinding();
  }

  /** Solid 的 DOM value 绑定会在选项插入时及微任务中补写尚未匹配的值。 */
  bindSelectValue(value: string): void {
    this.pendingSelectValue = value;
    this.applySelectBinding();
    queueMicrotask(() => this.applySelectBinding());
  }

  private applySelectBinding(): void {
    const value = this.pendingSelectValue;
    if (value !== null && this.selectValue(value)) {
      this.pendingSelectValue = null;
    }
  }

  get checked(): boolean {
    return this.attributes.checked === true;
  }

  set checked(value: boolean) {
    this.attributes.checked = value;
  }

  get disabled(): boolean {
    return this.hasAttribute("disabled");
  }

  get textContent(): string {
    return this.tag === "#text" ? this.text : this.nodes.map(textOf).join("");
  }

  get isConnected(): boolean {
    return currentTestDocument()?.find((node) => node === this) != null;
  }

  /** 属性空值与布尔假值都表示缺席。 */
  hasAttribute(name: string): boolean {
    const value = this.attributes[name];
    return Object.hasOwn(this.attributes, name) && value != null && value !== false;
  }

  getAttribute(name: string): string | null {
    if (!this.hasAttribute(name)) return null;
    const value = this.attributes[name];
    return value === true ? "" : String(value);
  }

  setAttribute(name: string, value: string): void {
    const selectsByDefault =
      this.tag === "option" &&
      name === "selected" &&
      !this.optionSelectionDirty &&
      !this.hasAttribute(name);
    this.attributes[name] = value;
    if (selectsByDefault) this.setOptionSelected(true);
  }

  /** 支持页面查询使用的标签、类名、禁用状态和单个属性选择器。 */
  matches(selector: string): boolean {
    return selector.split(",").some((part) => {
      const trimmed = part.trim();
      if (trimmed === ":disabled") return this.disabled;
      if (trimmed.startsWith(".")) return hasClass(this.attributes.class, trimmed.slice(1));
      const attribute = trimmed.match(/^\[([a-z0-9-]+)(?:="([^"]*)")?\]$/u);
      if (attribute?.[1] !== undefined) {
        const value = this.getAttribute(attribute[1]);
        return value !== null && (attribute[2] === undefined || value === attribute[2]);
      }
      return this.tag.toLowerCase() === trimmed.toLowerCase();
    });
  }

  querySelector(selector: string): TestElement | null {
    for (const part of selector.split(",")) {
      const trimmed = part.trim();
      const direct = trimmed.match(/^:scope\s*>\s*([a-z0-9-]+)$/iu);
      const found = findElement(this.nodes, (node) =>
        direct?.[1] === undefined
          ? node.matches(trimmed)
          : node.parentElement === this && node.matches(direct[1]),
      );
      if (found !== null) return found;
    }
    return null;
  }

  querySelectorAll(selector: string): TestElement[] {
    return findAll(this.nodes, (node) => node.matches(selector));
  }

  focus(): void {
    this.focusCount += 1;
    const document = currentTestDocument();
    if (document !== null) {
      document.activeElement = this;
      document.focusCalls.push(this);
    }
  }

  // 内存文档没有视口，也不会触发浏览器的原生对话框。
  scrollIntoView(): void {}
  showModal(): void {}
  close(): void {}

  click(): void {
    if (this.disabled) throw new Error(`按钮不可点击：${this.textContent.trim()}`);
    this.dispatchClick();
  }

  /** 人工派发事件，用于验证已禁用的处理器仍能拒绝提交。 */
  dispatchClick(): void {
    captureEvent(this, "onClick")();
    flush();
  }

  submit(): boolean {
    const prevented = captureEvent(this, "onSubmit")();
    flush();
    return prevented;
  }

  input(value: string): void {
    this.value = value;
    captureEvent(this, "onInput")();
    flush();
  }

  change(value?: string | boolean): void {
    if (typeof value === "string") {
      this.value = value;
    } else if (typeof value === "boolean") {
      this.checked = value;
    }
    captureEvent(this, "onChange")();
    flush();
  }

  blur(): void {
    captureEvent(this, "onBlur")();
    flush();
  }
}

function hasClass(value: unknown, name: string): boolean {
  if (typeof value === "string") return value.split(/\s+/u).includes(name);
  if (Array.isArray(value)) return value.some((item) => hasClass(item, name));
  return typeof value === "object" && value !== null && Reflect.get(value, name) === true;
}

/** 捕获当前处理器，供用例在换代后验证旧事件的行为。 */
export function captureEvent(
  element: TestElement,
  name: "onClick" | "onSubmit" | "onInput" | "onChange" | "onBlur",
): () => boolean {
  const handler = element.attributes[name];
  if (typeof handler !== "function") {
    throw new Error(`<${element.tag}> #${element.id} 缺少 ${name} 处理器`);
  }
  return () => {
    let defaultPrevented = false;
    handler({
      currentTarget: element,
      target: element,
      preventDefault() {
        defaultPrevented = true;
      },
      stopPropagation() {},
    });
    return defaultPrevented;
  };
}

/** 从内存树中查询真实挂载的节点，并记录节点与祖先的插入、移除。 */
export class TestDocument {
  readonly roots: TestElement[] = [];
  activeElement: TestElement | null = null;
  readonly focusCalls: TestElement[] = [];
  private readonly watchedNodes = new Map<TestElement, NodeMutation[]>();

  watchMounts(node: TestElement): readonly NodeMutation[] {
    const mutations: NodeMutation[] = [];
    this.watchedNodes.set(node, mutations);
    return mutations;
  }

  /** 从插入和移除入口记录子树变化，移除时父链还可用。 */
  recordSubtreeMutation(root: TestElement, mutation: NodeMutation): void {
    for (const [node, mutations] of this.watchedNodes) {
      for (
        let ancestor: TestElement | null = node;
        ancestor !== null;
        ancestor = ancestor.parentElement
      ) {
        if (ancestor === root) {
          mutations.push(mutation);
          break;
        }
      }
    }
  }

  add(root: unknown): void {
    if (typeof root === "function") {
      this.add(root());
    } else if (root instanceof TestElement) {
      this.roots.push(root);
    } else if (Array.isArray(root)) {
      for (const item of root) this.add(item);
    }
  }

  find(match: (element: TestElement) => boolean): TestElement | null {
    return findElement(this.roots, match);
  }

  getElementById(id: string): TestElement | null {
    return this.find((node) => node.id === id);
  }

  querySelector(selector: string): TestElement | null {
    return this.find((node) => node.matches(selector));
  }

  querySelectorAll(selector: string): TestElement[] {
    return findAll(this.roots, (node) => node.matches(selector));
  }
}

/** 读取当前用例安装的文档；实际浏览器文档与其他替身不参与渲染记录。 */
export function currentTestDocument(): TestDocument | null {
  const document: unknown = globalThis.document;
  return document instanceof TestDocument ? document : null;
}

/** 在用例内替换 DOM 全局属性，并在部分安装失败时恢复。 */
export function installTestDocument(): TestDocument & { restore(): void };
export function installTestDocument<T extends TestDocument>(document: T): T & { restore(): void };
export function installTestDocument(document = new TestDocument()) {
  const scope = cleanupScope();
  try {
    for (const [key, value] of [
      ["document", document],
      ["HTMLElement", TestElement],
    ] as const) {
      scope.defer(preserveProperty(globalThis, key));
      Object.defineProperty(globalThis, key, { configurable: true, value });
    }
    return Object.assign(document, { restore: scope.dispose });
  } catch (error) {
    scope.dispose();
    throw error;
  }
}

/** 夹具前提不满足时直接报错，避免空断言通过。 */
export function required<T>(value: T | null | undefined, label = "值"): T {
  if (value == null) throw new Error(`缺少${label}`);
  return value;
}

function textOf(node: unknown): string {
  if (typeof node === "function") return textOf(node());
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (node instanceof TestElement) return node.textContent;
  return typeof node === "string" || typeof node === "number" ? String(node) : "";
}

/** 展开响应式插槽、数组与实际挂载的子树。 */
export function visit(node: unknown, callback: (element: TestElement) => void): void {
  if (typeof node === "function") {
    visit(node(), callback);
  } else if (Array.isArray(node)) {
    for (const item of node) visit(item, callback);
  } else if (node instanceof TestElement) {
    callback(node);
    for (const read of node.nodes) visit(read, callback);
  }
}

/** 查找第一个匹配节点。 */
export function findElement(
  node: unknown,
  match: (element: TestElement) => boolean,
): TestElement | null {
  let found: TestElement | null = null;
  visit(node, (element) => {
    if (found === null && match(element)) found = element;
  });
  return found;
}

/** 查找全部匹配节点，保留文档顺序。 */
export function findAll(node: unknown, match: (element: TestElement) => boolean): TestElement[] {
  const found: TestElement[] = [];
  visit(node, (element) => {
    if (match(element)) found.push(element);
  });
  return found;
}
