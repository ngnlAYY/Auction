// T10 页面用例装配内存节点、独立中文译文与 Solid 挂载生命周期。

import { expect, test } from "bun:test";
import { createRoot, flush, untrack } from "solid-js";
import { loadRenderModule } from "../solid-render-fixture.test";
import { cleanupScope } from "../test-scope-fixture";
import { installAdminTranslation } from "./admin-locale-fixture.test";
import {
  installTestDocument,
  mountNode,
  settle,
  type TestDocument,
  TestElement,
} from "./solid-dom-fixture";

/** 装配凭据导入夹具，参数来自同一具名组件的声明。 */
export async function loadCredentialForm() {
  const { CredentialFormFixture } = await loadRenderModule<
    Pick<typeof import("./credentials/form-fixture"), "CredentialFormFixture">
  >(new URL("./credentials/form-fixture.tsx", import.meta.url).pathname, "CredentialFormFixture");
  return CredentialFormFixture;
}

/** 装配凭据行或列表夹具，参数来自同一具名组件的声明。 */
export async function loadCredentialTable() {
  const { CredentialTableFixture } = await loadRenderModule<
    Pick<typeof import("./credentials/table-fixture"), "CredentialTableFixture">
  >(new URL("./credentials/table-fixture.tsx", import.meta.url).pathname, "CredentialTableFixture");
  return CredentialTableFixture;
}

/** 装配用户创建夹具，参数来自同一具名组件的声明。 */
export async function loadUserCreate() {
  const { UserCreateFixture } = await loadRenderModule<
    Pick<typeof import("./users/create-fixture"), "UserCreateFixture">
  >(new URL("./users/create-fixture.tsx", import.meta.url).pathname, "UserCreateFixture");
  return UserCreateFixture;
}

/** 装配真实用户筛选表单，参数来自生产组件声明。 */
export async function loadUserFilters() {
  const { AdminUserFilters } = await loadRenderModule<
    Pick<typeof import("./users/table"), "AdminUserFilters">
  >(new URL("./users/table.tsx", import.meta.url).pathname, "AdminUserFilters");
  return AdminUserFilters;
}

/** 在同一次挂载内处理事件，退出时恢复文档、译文与组件作用域。 */
export async function withT10View(
  render: () => unknown,
  run: (document: TestDocument) => Promise<void> | void,
): Promise<void> {
  const scope = cleanupScope();
  try {
    scope.defer(installAdminTranslation());
    const document = installTestDocument();
    scope.defer(document.restore);
    createRoot((cleanup) => {
      scope.defer(cleanup);
      mountNode(document, () => untrack(render));
    });
    await settle();
    await run(document);
  } finally {
    scope.dispose();
  }
}

/** 派发可取消的表单事件，并核对真实处理器阻止了默认提交。 */
export function submitForm(node: TestElement): void {
  const handler = node.attributes.onSubmit;
  if (node.tag !== "form" || typeof handler !== "function") {
    throw new Error("表单没有提交处理器");
  }
  const event = new Event("submit", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "currentTarget", { value: node });
  handler(event);
  expect(event.defaultPrevented).toBe(true);
}

/** 按 DOM id 获取节点；缺失时立即令用例失败。 */
export function field(document: TestDocument, id: string): TestElement {
  const node = document.getElementById(id);
  if (node === null) throw new Error(`找不到控件 #${id}`);
  return node;
}

/** 按选择器唯一定位控件或容器。 */
export function onlyElement(document: TestDocument, selector: string): TestElement {
  const nodes = document.querySelectorAll(selector);
  const node = nodes[0];
  if (nodes.length !== 1 || node === undefined) throw new Error(`${selector} 应恰有一个节点`);
  return node;
}

/** 对控件发出焦点事件，并刷新事件造成的响应式更新。 */
export function focusControl(node: TestElement): void {
  node.focus();
  const handler = node.attributes.onFocus;
  if (typeof handler !== "function") throw new Error("控件没有焦点处理器");
  handler({ currentTarget: node });
  flush();
}

/** 查找实际渲染的按钮；无匹配时返回空供测试检查卸载。 */
export function findButton(document: TestDocument, label: string): TestElement | null {
  return document.querySelectorAll("button").find((node) => node.textContent === label) ?? null;
}

/** 查找测试接下来要操作的按钮。 */
export function button(document: TestDocument, label: string): TestElement {
  const node = findButton(document, label);
  if (node === null) throw new Error(`找不到按钮「${label}」`);
  return node;
}

/** 在启用的真实按钮上触发真实处理器，并刷新异步和响应式更新。 */
export async function clickButton(document: TestDocument, label: string): Promise<void> {
  const node = button(document, label);
  if (node.hasAttribute("disabled") || typeof node.attributes.onClick !== "function") {
    throw new Error(`按钮「${label}」当前不可用`);
  }
  node.click();
  await settle();
}

/** 查询内存树中与目标文本完全相同的节点。 */
export function hasText(document: TestDocument, text: string): boolean {
  return containsText(document.roots, text);
}

function containsText(node: unknown, text: string): boolean {
  if (typeof node === "function") return containsText(node(), text);
  if (Array.isArray(node)) return node.some((child) => containsText(child, text));
  if (node instanceof TestElement) {
    return node.tag === "#text" ? node.text === text : containsText(node.nodes, text);
  }
  return node === text;
}

test("表单夹具把节点设为事件当前目标，并要求阻止默认提交", () => {
  const form = new TestElement("form", {
    onSubmit(event: Event) {
      expect<unknown>(event.currentTarget).toBe(form);
      expect(event.cancelable).toBe(true);
      event.preventDefault();
    },
  });
  submitForm(form);
  expect(() => submitForm(new TestElement("form", { onSubmit() {} }))).toThrow();
});
