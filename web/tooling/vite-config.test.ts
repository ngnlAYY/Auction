// 生产 HTML 和图标的产物约定，使用实际 Vite 构建但不写入文件。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { build, type Plugin } from "vite";

import { surfaceConfig } from "./vite-config";

const MODULE_SCRIPT_ELEMENT = /<script(?=\s)[^>]*\stype="module"[^>]*><\/script>/;

async function buildPublic(added?: Plugin) {
  const config = surfaceConfig("public");
  const result = await build({
    ...config,
    configFile: false,
    logLevel: "silent",
    plugins: [...(config.plugins ?? []), added],
    build: { ...config.build, write: false, reportCompressedSize: false },
  });
  if (Array.isArray(result) || !("output" in result)) {
    throw new Error("预期 Public 构建只生成一个产物目录");
  }
  return result.output;
}

test("生产入口保留 Rocket Loader 豁免并引用带指纹的共享图标", async () => {
  const output = await buildPublic();
  const index = output.find((item) => item.fileName === "index.html");
  if (index?.type !== "asset" || typeof index.source !== "string") {
    throw new Error("Public 产物缺少入口 HTML");
  }
  const scripts = index.source.match(/<script\b[^>]*\stype="module"[^>]*>/g);
  expect(scripts).toHaveLength(1);
  expect(scripts?.[0]).toMatch(/\sdata-cfasync="false"[^>]*\ssrc="\/assets\/[^"]+\.js"/);

  const iconPath = index.source.match(
    /<link rel="icon"[^>]*\shref="\/(assets\/favicon-[\w-]+\.svg)"/,
  )?.[1];
  const icon = output.find((item) => item.fileName === iconPath);
  if (icon?.type !== "asset") {
    throw new Error("Public 入口应当引用带指纹的图标文件");
  }
  const sourceIcon = readFileSync(
    new URL("../../crates/auction-web-shared/assets/favicon.svg", import.meta.url),
  );
  expect(Buffer.from(icon.source)).toEqual(sourceIcon);
});

test("入口前的注释含同名脚本时，只为实际入口添加豁免", async () => {
  const output = await buildPublic({
    name: "test-comment-before-entry",
    enforce: "pre",
    transformIndexHtml: {
      order: "post",
      handler: (html) => html.replace(MODULE_SCRIPT_ELEMENT, "<!--\n$&\n-->\n$&"),
    },
  });
  const index = output.find((item) => item.fileName === "index.html");
  expect(index?.type === "asset" ? index.source : null).toMatch(
    /<!--\n<script type="module"[^>]*><\/script>\n-->\n<script data-cfasync="false" type="module"/,
  );
});

test.each([
  {
    name: "丢失豁免属性",
    transform: (html: string) => html.replace(' data-cfasync="false"', ""),
    error: "Rocket Loader 豁免",
  },
  {
    name: "存在两个模块入口",
    transform: (html: string) =>
      html.replace("</head>", '<script type="module" src="/extra.js"></script></head>'),
    error: "恰好包含一个带 src 的模块脚本",
  },
  {
    name: "只有注释中的模块入口",
    transform: (html: string) => html.replace(MODULE_SCRIPT_ELEMENT, "<!-- $& -->"),
    error: "恰好包含一个带 src 的模块脚本",
  },
])("最终 HTML $name 时拒绝构建", async ({ transform, error }) => {
  await expect(
    buildPublic({
      name: "test-change-final-html",
      transformIndexHtml: { order: "post", handler: transform },
    }),
  ).rejects.toThrow(error);
});
