// 配置页面需要的真实表单、回执区和文档入口共用 Admin 渲染夹具。

import { expect, test } from "bun:test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { createComponent, createElement, insert } from "../solid-dom-fixture";

/** 加载配置测试用到的组件并核对实际使用的导出。 */
export async function loadConfigFixture() {
  const [
    form,
    number,
    announcer,
    validation,
    review,
    runtimeSwitches,
    pendingPanel,
    worlds,
    mailTemplates,
    document,
  ] = await Promise.all([
    loadRenderModule<typeof import("./form")>(
      new URL("./form/index.tsx", import.meta.url).pathname,
      "RuntimeConfigFields",
    ),
    loadRenderModule<typeof import("./form/number")>(
      new URL("./form/number.tsx", import.meta.url).pathname,
      "NumberInput",
      "FieldAnnouncerContext",
    ),
    loadRenderModule<typeof import("./field-announcement")>(
      new URL("./field-announcement.tsx", import.meta.url).pathname,
      "FieldLiveRegion",
      "useFieldAnnouncer",
    ),
    loadRenderModule<typeof import("./validation")>(
      new URL("./validation.tsx", import.meta.url).pathname,
      "collectIssues",
      "colorLevelPath",
      "ErrorSummary",
      "FormIssuesContext",
    ),
    loadRenderModule<typeof import("./review")>(
      new URL("./review.tsx", import.meta.url).pathname,
      "ConfigChangeList",
    ),
    loadRenderModule<typeof import("./runtime-switches")>(
      new URL("./runtime-switches.tsx", import.meta.url).pathname,
      "RuntimeSwitchCard",
    ),
    loadRenderModule<typeof import("../pending/panel")>(
      new URL("../pending/panel.tsx", import.meta.url).pathname,
      "PendingCommandNotices",
    ),
    loadRenderModule<typeof import("./worlds")>(
      new URL("./worlds.tsx", import.meta.url).pathname,
      "AdminBusinessWorlds",
    ),
    loadRenderModule<typeof import("./mail-templates")>(
      new URL("./mail-templates.tsx", import.meta.url).pathname,
      "MailTemplateCard",
    ),
    loadRenderModule<typeof import("./document")>(
      new URL("./document.tsx", import.meta.url).pathname,
      "RuntimeConfigDocument",
    ),
  ]);
  return {
    form,
    number,
    announcer,
    validation,
    review,
    runtimeSwitches,
    pendingPanel,
    worlds,
    mailTemplates,
    document,
    createRootElement: () => createElement("div"),
    createComponent,
    insert,
  };
}

test("配置渲染夹具可把内存节点插入它分配的各自容器", async () => {
  const fixture = await loadConfigFixture();
  const first = fixture.createRootElement();
  const second = fixture.createRootElement();
  const child = createElement("span");
  fixture.insert(first, child);
  expect(first.querySelector("span")).toBe(child);
  expect(second.querySelector("span")).toBeNull();
});
