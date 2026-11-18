// 在生产外壳的焦点陷阱下提供 DOM 查询、捕获阶段和提交后的 MutationObserver。

import { untrack } from "solid-js";
import { DomElement } from "./navigation-fixture.test";

type Find = (match: (element: DomElement) => boolean, parent?: DomElement) => DomElement | null;

const DIALOG = '[role="dialog"][aria-modal="true"]';

function matches(element: DomElement, selector: string): boolean {
  switch (selector) {
    case DIALOG:
      return (
        element.getAttribute("role") === "dialog" && element.getAttribute("aria-modal") === "true"
      );
    case "[autofocus]":
      return element.attributes.autofocus === true;
    case ":disabled":
      return element.attributes.disabled === true;
    default:
      return (
        selector.startsWith("a[href],button") &&
        (element.tag === "button" ||
          element.tag === "input" ||
          element.getAttribute("href") !== null)
      );
  }
}

/** 由测试在实际事件前后驱动生产监听器，连通性以当前渲染树为准。 */
export function installDialogFocus() {
  let find: Find = () => null;
  let active: DomElement | null = null;
  const observers = new Set<(records: readonly unknown[]) => void>();
  const adapted = new WeakSet<DomElement>();
  const connected = (element: DomElement) => find((node) => node === element) !== null;
  const nodes = (selector: string, parent?: DomElement) => {
    const result: DomElement[] = [];
    find((element) => {
      if (element !== parent && matches(element, selector)) result.push(adapt(element));
      return false;
    }, parent);
    return Object.assign(result, { item: (index: number) => result[index] ?? null });
  };

  function adapt(element: DomElement): DomElement {
    if (adapted.has(element)) return element;
    adapted.add(element);
    Object.defineProperty(element, "isConnected", { get: () => connected(element) });
    return Object.assign(element, {
      focus() {
        active = element;
      },
      matches: (selector: string) => matches(element, selector),
      closest: (selector: string) => (matches(element, selector) ? element : null),
      getClientRects: () => (connected(element) ? [{}] : []),
      contains: (other: DomElement) => find((node) => node === other, element) !== null,
      isSameNode: (other: DomElement) => element === other,
      querySelector: (selector: string) => nodes(selector, element).item(0),
      querySelectorAll: (selector: string) => nodes(selector, element),
    });
  }

  const documentFixture = Object.assign(new EventTarget(), {
    body: { style: { getPropertyValue: () => "", setProperty() {}, removeProperty() {} } },
    documentElement: { setAttribute() {} },
    hidden: false,
    scrollingElement: null,
    querySelectorAll: nodes,
    getElementById(id: string) {
      const element = find((node) => node.attributes.id === id);
      return element === null ? null : adapt(element);
    },
  });
  const activeElement = () => (active !== null && connected(active) ? active : null);
  Object.defineProperty(documentFixture, "activeElement", { get: activeElement });

  class Observer {
    constructor(private readonly callback: (records: readonly unknown[]) => void) {}
    observe() {
      observers.add(this.callback);
    }
    disconnect() {
      observers.delete(this.callback);
    }
  }

  const globals = {
    document: documentFixture,
    Element: DomElement,
    HTMLElement: DomElement,
    MutationObserver: Observer,
  };
  const previous = Object.keys(globals).map(
    (key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)] as const,
  );
  for (const [key, value] of Object.entries(globals)) {
    Object.defineProperty(globalThis, key, { configurable: true, value });
  }
  return {
    attach(lookup: Find) {
      // 浏览器查询已提交的 DOM。替身读取对应 Solid 节点时也不建立订阅。
      find = (match, parent) => untrack(() => lookup(match, parent));
    },
    captureClick(element: DomElement) {
      const event = new Event("click");
      Object.defineProperty(event, "target", { value: adapt(element) });
      documentFixture.dispatchEvent(event);
    },
    commit() {
      for (const observer of observers) observer([]);
    },
    activeElement,
    restore() {
      for (const [key, descriptor] of previous) {
        if (descriptor === undefined) Reflect.deleteProperty(globalThis, key);
        else Object.defineProperty(globalThis, key, descriptor);
      }
    },
  };
}
