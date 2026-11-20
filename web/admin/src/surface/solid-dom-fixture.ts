// 内存 DOM 上执行 Solid universal JSX 的插入、属性绑定与宿主装配。

import { ref as browserRef } from "@solidjs/web";
import {
  createMemo,
  createRenderEffect,
  type Element,
  flatten,
  flush,
  createComponent as mountComponent,
  untrack,
} from "solid-js";
import { currentTestDocument, type TestDocument, TestElement } from "./test-dom-fixture";

export {
  captureEvent,
  findAll,
  findElement,
  installTestDocument,
  required,
  TestDocument,
  TestElement,
  visit,
} from "./test-dom-fixture";

/** 点击已挂载的按钮，缺少真实事件绑定时让用例失败。 */
export function click(element: TestElement): void {
  if (element.tag !== "button") throw new Error("点击目标不是按钮");
  element.click();
}

/** 返回控件由 JSX 写入的 value 快照，供用例区分用户输入与渲染回填。 */
export function valueUpdates(element: TestElement): readonly unknown[] {
  return element.propWrites.filter(({ key }) => key === "value").map(({ value }) => value);
}

let componentDiagnostics = false;

/** 在专门核对开发期诊断的用例中启用浏览器组件体与 ref 读取规则。 */
export function enableSolidComponentDiagnostics(): () => void {
  const previous = componentDiagnostics;
  componentDiagnostics = true;
  return () => {
    componentDiagnostics = previous;
  };
}

/** 在组件 owner 内执行真实组件，保持 universal 输出的响应式边界。 */
export function createComponent<P extends object>(
  component: (props: P) => Element,
  props: P,
): Element {
  if (componentDiagnostics) return mountComponent(component, props, component.name);
  return mountComponent((input: P) => untrack(() => component(input)), props, component.name);
}

/** 创建内存元素或静态文本节点。 */
export function createElement(tag: string, props: Record<string, unknown> = {}): TestElement {
  return new TestElement(tag, props);
}

/** 文本节点的身份独立于其父节点。 */
export function createTextNode(text: string): TestElement {
  const node = createElement("#text", { text });
  node.text = text;
  return node;
}

const placements = new WeakMap<TestElement, () => void>();

/** 移除当前插入位置，先记录旧父链，再把节点从原槽位中释放。 */
export function removeNode(node: TestElement): void {
  placements.get(node)?.();
}

function place(parent: TestElement, child: TestElement, removeFromSlot: () => void): void {
  removeNode(child);
  child.parentElement = parent;
  currentTestDocument()?.recordSubtreeMutation(child, "insert");
  placements.set(child, () => {
    currentTestDocument()?.recordSubtreeMutation(child, "remove");
    placements.delete(child);
    child.parentElement = null;
    removeFromSlot();
  });
}

function insertSlot(parent: TestElement, read: () => unknown, marker: TestElement | null): void {
  if (marker === null) {
    parent.nodes.push(read);
    return;
  }
  const index = parent.nodes.findIndex((slot) => slot() === marker);
  if (index < 0) throw new Error("编译器指定的子节点锚点未挂载");
  parent.nodes.splice(index, 0, read);
}

function updateSelect(
  parent: TestElement,
  changed: readonly TestElement[],
  inserted = false,
): void {
  const select = parent.tag === "optgroup" ? parent.parentElement : parent;
  if (select?.tag !== "select") return;
  const options = changed.filter((node) => node.tag === "option" || node.tag === "optgroup");
  if (options.length > 0) select.optionsChanged(inserted ? options : []);
}

/** 插入静态节点；移动已有节点时先从旧槽位移除，即使父节点相同。 */
export function insertNode(
  parent: TestElement,
  node: TestElement,
  marker: TestElement | null = null,
): void {
  if (marker === node && node.parentElement === parent) return;
  const read = () => node;
  place(parent, node, () => {
    const index = parent.nodes.indexOf(read);
    if (index >= 0) parent.nodes.splice(index, 1);
    updateSelect(parent, [node]);
  });
  insertSlot(parent, read, marker);
  updateSelect(parent, [node], true);
}

function elementsIn(value: unknown): TestElement[] {
  if (value instanceof TestElement) return [value];
  return Array.isArray(value) ? value.flatMap(elementsIn) : [];
}

function withoutNode(value: unknown, node: TestElement): unknown {
  if (value === node) return null;
  if (Array.isArray(value)) return value.map((child) => withoutNode(child, node));
  return value;
}

/** 按编译器提供的锚点挂载动态槽位，保留相同节点并记录实际移除、移动和插入。 */
export function insert(
  parent: TestElement,
  node: unknown,
  marker: TestElement | null = null,
): void {
  if (node instanceof TestElement) {
    insertNode(parent, node, marker);
    return;
  }
  let content: unknown = null;
  const mounted: TestElement[] = [];
  insertSlot(parent, () => content, marker);
  const commit = (next: unknown) => {
    const children = elementsIn(next);
    const added: TestElement[] = [];
    for (const child of [...mounted]) {
      if (!children.includes(child)) {
        removeNode(child);
      }
    }
    for (const [index, child] of children.entries()) {
      if (mounted[index] === child) continue;
      place(parent, child, () => {
        mounted.splice(mounted.indexOf(child), 1);
        content = withoutNode(content, child);
        updateSelect(parent, [child]);
      });
      mounted.splice(index, 0, child);
      added.push(child);
    }
    content = next;
    updateSelect(parent, added, true);
  };
  effect(
    () => {
      const first = typeof node === "function" ? node() : node;
      const next = flatten(first, { doNotUnwrap: true, skipNonRendered: true });
      if (typeof next === "function") {
        effect(() => flatten(next, { skipNonRendered: true }), commit);
        return;
      }
      return () => commit(next);
    },
    (apply) => apply?.(),
  );
}

/** 以同步渲染的顺序计算 JSX。 */
export function memo<T>(compute: () => T) {
  return createMemo(compute, { sync: true });
}

/** 在当前渲染 owner 中提交 DOM 更新。 */
export function effect<T>(compute: () => T, apply: (value: T) => void): void {
  createRenderEffect(compute, apply, { sync: true, transparent: true });
}

/** 合并 JSX 展开属性，后一份优先。 */
export { merge as mergeProps } from "solid-js";

/** 把展开的属性写入节点，保留响应式 getter 与写入次序。 */
export function spread(
  element: TestElement,
  sources: readonly Record<string, unknown>[],
  skipChildren = false,
): void {
  effect(
    () => Object.assign({}, ...sources),
    (props) => {
      for (const [key, value] of Object.entries(props)) {
        if (key !== "children" || !skipChildren) setProp(element, key, value);
      }
    },
  );
}

/** 记录 Solid 的属性写入，原生控件模拟的直接赋值不计入。 */
export function setProp(element: TestElement, key: string, value: unknown): void {
  if (key === "ref" && typeof value === "function") {
    value(element);
    return;
  }
  element.propWrites.push({ key, value });
  if (element.tag === "select" && key === "value" && typeof value === "string") {
    element.bindSelectValue(value);
  } else if (element.tag === "option" && key === "selected") {
    element.selected = Boolean(value);
  } else {
    element.attributes[key] = value;
  }
}

/** 转发编译器生成的 ref 访问器。 */
export function ref(callback: unknown, element: TestElement): void {
  if (componentDiagnostics && typeof callback === "function") {
    Reflect.apply(browserRef, undefined, [callback, element]);
    return;
  }
  const target = typeof callback === "function" ? callback() : callback;
  if (typeof target === "function") target(element);
}

/** 用响应式根槽位消费上下文和动态分支。 */
export function mountNode(document: TestDocument, node: unknown): void {
  const container = createElement("div");
  insert(container, node);
  document.add(container);
}

/** 排空内存回执和 Solid 微任务，保留测试对定时器推进的控制。 */
export async function settle(rounds = 48): Promise<void> {
  for (let round = 0; round < rounds; round += 1) {
    await Promise.resolve();
    flush();
  }
}
