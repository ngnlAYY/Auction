// 手机菜单的常规与强制颜色图标样式。

import { expect, test } from "bun:test";

test("手机菜单保留常规图标，强制颜色使用可见的系统色图形", async () => {
  const css = await Bun.file(new URL("./mobile-topbar.css", import.meta.url)).text();
  const forced = css.indexOf("@media screen and (max-width: 48rem) and (forced-colors: active)");
  expect(forced).toBeGreaterThan(0);
  const normal = css.slice(0, forced);
  expect(normal).toMatch(/\.public-shell__nav-toggle::before\s*\{[^}]*linear-gradient/u);
  const override = css.slice(forced);
  expect(override).toMatch(
    /\.public-shell__nav-toggle\[aria-expanded="false"\]::before\s*\{[^}]*color: ButtonText/u,
  );
  expect(override).toMatch(/content: "☰";[^}]*font-size: var\(--text-lg\)/u);
  expect(override).not.toContain('aria-expanded="true"');
  expect(override).not.toContain("forced-color-adjust: none");
});
