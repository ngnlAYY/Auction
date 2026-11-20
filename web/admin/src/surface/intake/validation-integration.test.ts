// 用真实录入表单检查 JSON 字段提示、聚焦与请求边界。

import { expect, spyOn, test } from "bun:test";
import { fixtureText } from "../../api/auction-contract-fixture.test";
import { equipmentJson, equipmentJsonWithBase } from "../../api/intake-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import {
  type EditorTest,
  editField,
  settleEditor,
  submitForm,
  takeWrite,
  withEditor,
} from "../auctions/editor-fixture.test";
import { createComponent } from "../solid-dom-fixture";
import { validCanonicalEquipmentJson } from "./validation";

async function withJsonIntake(
  text: string,
  run: (page: EditorTest) => Promise<void>,
): Promise<void> {
  const { AdminIntakeForm } = await loadRenderModule<typeof import(".")>(
    `${import.meta.dir}/index.tsx`,
    "AdminIntakeForm",
  );
  await withEditor(
    () => createComponent(AdminIntakeForm, { worldId: "main" }),
    async (page) => {
      page.field("auction-intake-source").change("json");
      await settleEditor();
      const input = page.field("auction-intake-json");
      await editField(input, text);
      await editField(page.field("auction-intake-price"), "1000");
      await editField(page.field("auction-intake-player"), "Player");
      await run(page);
    },
  );
}

async function expectJsonFieldError(text: string): Promise<void> {
  await withJsonIntake(text, async (page) => {
    const input = page.field("auction-intake-json");
    await submitForm(page.form());
    expect(input.getAttribute("aria-invalid")).toBe("true");
    expect(input.getAttribute("aria-describedby")).toContain("auction-intake-json-error");
    expect(page.field("auction-intake-json-error").textContent).toBe(
      "请输入有效的规范装备 JSON 对象。",
    );
    expect(document.activeElement?.id).toBe("auction-intake-json");
    expect(page.takeWrites()).toEqual([]);
  });
}

test("被覆盖的 prefix 代理项在录入表单标错并聚焦 JSON 字段", async () => {
  await expectJsonFieldError(equipmentJson(undefined, String.raw`"prefix":"\uD800","prefix":""`));
});

test("被覆盖的属性字符串仍须是合法 Unicode", async () => {
  await expectJsonFieldError(
    equipmentJson(String.raw`"Defense":{"value":"\uD800"},"Defense":{"value":"2"}`),
  );
});

test("被覆盖的属性里重复的具名字段在 JSON 字段标错", async () => {
  await expectJsonFieldError(
    equipmentJson('"Defense":{"base":0,"base":1},"Defense":{"value":"2"}'),
  );
});

// 与 Rust 不同（Rust 逐次解码、发送前拒绝），登记在 web/README.md 的差异表。
test("同名属性的非法首项可被合法末项覆盖，录入只发送末值", async () => {
  const text = equipmentJson('"Defense":{"value":true},"Defense":{"value":"2"}');
  await withJsonIntake(text, async (page) => {
    page.reply(201, fixtureText("lot-intake-command"));
    await submitForm(page.form());
    expect(page.field("auction-intake-json").hasAttribute("aria-invalid")).toBe(false);
    const request = takeWrite(page);
    expect(request.method).toBe("POST");
    expect(request.path).toBe("/api/v1/admin/worlds/main/auction-intake/items");
    expect(request.body).toContain('"stats":{"Defense":{"value":"2"}}');
    expect(request.body).not.toContain('"value":true');
  });
});

test("重复根字段 level 通过表单校验，由接口准备阶段返回整表错误", async () => {
  const text = equipmentJson(undefined, undefined, '5,"level":7');
  await withJsonIntake(text, async (page) => {
    page.field("auction-intake-player").focus();
    const fetch = spyOn(globalThis, "fetch");
    try {
      await submitForm(page.form());
      expect(page.field("auction-intake-json").hasAttribute("aria-invalid")).toBe(false);
      expect(document.activeElement?.id).toBe("auction-intake-player");
      expect(page.hasText("请输入有效的规范装备 JSON 对象。")).toBe(false);
      expect(page.hasText("装备录入失败")).toBe(true);
      expect(page.hasText("请求参数无效。请检查输入内容后重试。")).toBe(true);
      expect(fetch).not.toHaveBeenCalled();
    } finally {
      fetch.mockRestore();
    }
  });
});

test.each(["0.1", "0.10000000000000001", "0.1234567890123456789", "-0"])(
  "表单接受范围内的 f64 写法 %s",
  (base) => {
    expect(validCanonicalEquipmentJson(equipmentJsonWithBase(base))).toBe(true);
  },
);

test("表单接受带负零的同名属性，按末值校验业务范围", () => {
  const text = equipmentJson(
    '"Defense":{"value":"1","base":-0},"Defense":{"value":"2","base":0.1}',
  );
  expect(validCanonicalEquipmentJson(text)).toBe(true);
});

test.each(["1e400", "1000000.01", "-0.1"])("在 JSON 字段拒绝无效 base %s", async (base) => {
  await expectJsonFieldError(equipmentJsonWithBase(base));
});

test("表单按最后一次出现的 level 检查整数词法", () => {
  const acceptsLevel = (level: string) =>
    validCanonicalEquipmentJson(equipmentJson(undefined, undefined, level));
  expect(acceptsLevel("7")).toBe(true);
  expect(acceptsLevel("7.0")).toBe(false);
  expect(acceptsLevel('5.0,"level":7')).toBe(true);
  expect(acceptsLevel('5,"level":7.0')).toBe(false);
  expect(acceptsLevel('5.0,"level":null')).toBe(true);
  expect(acceptsLevel(String.raw`5,"\u006cevel":7.0`)).toBe(false);
});
