// 生产入口在语言就绪前保留静态错误提示，随后同步实际生效的译文。

import { expect, spyOn, test } from "bun:test";
import { LANGUAGE_STORAGE_KEY } from "@auction/web-shared/ui/i18n";
import { dispatch, waitFor } from "../commands/competition-fixture.test";
import { installDialogFocus } from "../shell/dialog-focus-fixture.test";
import { mountPublicApp } from "../shell/navigation-fixture.test";
import { reject, success } from "../shell/session-fixture";
import { PUBLIC_LOCALE_CATALOG } from "./locale-catalog";

test("启动错误使用已加载的语言，等待或切换失败时保留现有提示", async () => {
  const html = await Bun.file(new URL("../../../index.html", import.meta.url)).text();
  const staticText = /<p class="app-boot__error" role="alert">([^<]+)<\/p>/u.exec(html)?.[1];
  if (staticText === undefined) throw new Error("入口文档缺少静态错误提示");
  const zh = await Bun.file(
    new URL("../../../../../crates/auction-web-public/i18n/zh-CN.ftl", import.meta.url),
  ).text();
  const banner = { textContent: staticText };
  const documentLanguage: string[] = [];
  const dom = installDialogFocus();
  Object.assign(document, {
    documentElement: {
      setAttribute(name: string, value: string) {
        if (name === "lang") documentLanguage.push(value);
      },
    },
    querySelector(selector: string) {
      return selector === "#main > .app-boot > .app-boot__error" ? banner : null;
    },
  });
  const savedLanguage = Promise.withResolvers<Response>();
  const missingLanguage = Promise.withResolvers<Response>();
  const localeRequests: string[] = [];
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      async (input: RequestInfo | URL) => {
        const path = String(input);
        const locale = PUBLIC_LOCALE_CATALOG.sources.find((source) => source.url === path);
        if (locale !== undefined) {
          localeRequests.push(locale.id);
          return locale.id === "zh-CN" ? savedLanguage.promise : missingLanguage.promise;
        }
        if (path === "/api/v1/session") return success(null);
        if (path === "/health/ready") return success({ ready: true });
        if (path.startsWith("/api/v1/auctions/current")) return success({ auction: null });
        return reject("not_found", 404);
      },
      { preconnect() {} },
    ),
  );
  let page: Awaited<ReturnType<typeof mountPublicApp>> | null = null;
  try {
    const app = await mountPublicApp("/", new Map([[LANGUAGE_STORAGE_KEY, "zh-CN"]]));
    page = app;
    dom.attach(app.find);
    await waitFor(() => localeRequests.length === 1);
    expect(banner.textContent).toBe(staticText);
    savedLanguage.resolve(new Response(zh));
    await waitFor(() => documentLanguage.at(-1) === "zh-CN");
    expect(banner.textContent).toBe("页面运行出现异常，请刷新页面重试。");

    const select = app.find((node) => node.getAttribute("id") === "language");
    if (select === null) throw new Error("缺少语言选择器");
    const input = Object.assign(select, { value: "ja" });
    dispatch(input, "Change");
    await waitFor(() => localeRequests.length === 2);
    expect(banner.textContent).toBe("页面运行出现异常，请刷新页面重试。");
    missingLanguage.resolve(new Response(null, { status: 503 }));
    await waitFor(
      () => app.find((node) => node.getAttribute("class") === "language-picker__error") !== null,
    );
    expect(banner.textContent).toBe("页面运行出现异常，请刷新页面重试。");
    input.value = "en";
    dispatch(input, "Change");
    await waitFor(() => documentLanguage.at(-1) === "en");
    expect(banner.textContent).toBe(staticText);
    expect(localeRequests).toEqual(["zh-CN", "ja"]);
  } finally {
    savedLanguage.resolve(new Response(zh));
    missingLanguage.resolve(new Response(null, { status: 503 }));
    page?.dispose();
    fetcher.mockRestore();
    dom.restore();
  }
});
