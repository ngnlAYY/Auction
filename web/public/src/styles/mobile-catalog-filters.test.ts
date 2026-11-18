// 分类面板定位、遮罩与紧凑排版的媒体规则。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";

function readStyles(path: string): string {
  return readFileSync(new URL(path, import.meta.url), "utf8").replace(/\/\*[\s\S]*?\*\//gu, "");
}

const SHARED = readStyles("../../../../crates/auction-web-public/assets/mobile.css");
const CATALOG = readStyles("./mobile-catalog-filters.css");
const PHONE_MEDIA = "screen and (max-width: 48rem)";
const TOUCH_TABLET_MEDIA = "screen and (max-width: 64rem) and (pointer: coarse)";

function normalizeMedia(media: string): string {
  return media.trim().replace(/\s+/gu, " ");
}

function mediaForDeclaration(selector: RegExp, declaration: string): string[] {
  const matches: string[] = [];
  // 专属表的媒体块位于顶层，各块内的选择器使用两空格缩进。
  for (const [, media = "", rules = ""] of CATALOG.matchAll(
    /@media\s+([^{}]+)\{([\s\S]*?)^\}/gmu,
  )) {
    for (const [, selectors = "", declarations = ""] of rules.matchAll(/([^{}]+)\{([^{}]*)\}/gu)) {
      if (selector.test(selectors.trim()) && declarations.includes(declaration)) {
        matches.push(normalizeMedia(media));
      }
    }
  }
  return matches;
}

test("分类面板脱离文档流、全屏遮罩与定位基准都跟随共享手机条件", () => {
  const sharedMedia = /@media\s+([^{}]+)\{\s*\.public-desktop\s*\{\s*display:\s*none;/u.exec(
    SHARED,
  )?.[1];
  if (sharedMedia === undefined) {
    throw new Error("找不到共享样式隐藏桌面控件的媒体条件");
  }
  const active = normalizeMedia(sharedMedia);
  expect(active).toBe(`${PHONE_MEDIA}, ${TOUCH_TABLET_MEDIA}`);
  expect(
    mediaForDeclaration(/\.public-catalog-category-menu__panel$/u, "position: absolute;"),
  ).toEqual([active]);
  expect(
    mediaForDeclaration(/\.public-catalog-category-menu__backdrop$/u, "position: fixed;"),
  ).toEqual([active]);
  expect(mediaForDeclaration(/\.public-catalog-category-menu__backdrop$/u, "inset: 0;")).toEqual([
    active,
  ]);
  expect(
    mediaForDeclaration(
      /^\.public-auction-live \.public-auction-type-toolbar$/u,
      "position: relative;",
    ),
  ).toEqual([active]);
});

test("三行紧凑网格仅应用于窄屏，触控平板沿用共享布局", () => {
  expect(
    mediaForDeclaration(
      /\.public-auction-group-toolbar,/u,
      "grid-template-columns: var(--filter-label-size) minmax(0, 1fr) auto;",
    ),
  ).toEqual([PHONE_MEDIA]);
});
