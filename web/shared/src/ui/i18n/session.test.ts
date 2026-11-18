// 语言切换、选择器的显示语言和翻译失败处理。

import { expect, spyOn, test } from "bun:test";
import { createRoot, For } from "solid-js";
import { LANGUAGE_STORAGE_KEY, type LocaleCatalog, SUPPORTED_LOCALES } from "./catalog";
import { buildBundle } from "./resource";
import type { LocaleSession } from "./session";

Object.defineProperty(globalThis, "__AUCTION_BUILD_ID__", {
  configurable: true,
  value: "test-build",
});
Object.defineProperty(globalThis, "__AUCTION_PACKAGE_VERSION__", {
  configurable: true,
  value: "0.0.0",
});

const { createLocaleSession, LanguagePicker, LocaleSessionContext, t, translationFailed } =
  await import(".");

const CATALOG: LocaleCatalog = {
  defaultLocale: "en",
  defaultFtl: "greeting = Hello\nlanguage-switch-failed = Failed to load language\n",
  sources: [
    { id: "ja", url: "/ja.ftl" },
    { id: "ko", url: "/ko.ftl" },
  ],
};

function localeHarness() {
  const bundle = buildBundle("en", CATALOG.defaultFtl, "en", null);
  if (!bundle.ok) {
    throw new Error(bundle.error);
  }
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  const stored = new Map([[LANGUAGE_STORAGE_KEY, "en"]]);
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {
      localStorage: {
        getItem: (key: string) => stored.get(key) ?? null,
        setItem: (key: string, value: string) => stored.set(key, value),
      },
    },
  });
  const requests: { signal: AbortSignal | null; resolve: (response: Response) => void }[] = [];
  const mockFetch = Object.assign(
    (_: RequestInfo | URL, init?: RequestInit) => {
      const { promise, resolve } = Promise.withResolvers<Response>();
      requests.push({ signal: init?.signal ?? null, resolve });
      return promise;
    },
    { preconnect: () => undefined },
  );
  const fetchSpy = spyOn(globalThis, "fetch").mockImplementation(mockFetch);
  const { session, dispose } = createRoot((dispose) => ({
    session: createLocaleSession(bundle.value, "en"),
    dispose,
  }));
  session.bootstrap(CATALOG);
  return {
    session,
    request(index: number) {
      const request = requests[index];
      if (request === undefined) {
        throw new Error(`缺少第 ${index + 1} 次译文请求`);
      }
      return request;
    },
    stored,
    dispose,
    cleanup() {
      dispose();
      for (const request of requests) {
        request.resolve(new Response(null, { status: 503 }));
      }
      fetchSpy.mockRestore();
      if (previous === undefined) {
        Reflect.deleteProperty(globalThis, "window");
      } else {
        Object.defineProperty(globalThis, "window", previous);
      }
    },
  };
}

// 等 fetch、正文读取和会话提交的微任务排空。
const settleRequests = () => Bun.sleep(0);

// 读取原生控件和选项的属性，使用测试转译器提供的 JSX 替身。
function pickerSelection(session: LocaleSession) {
  const jsx = Object.getOwnPropertyDescriptor(globalThis, "jsx");
  if (jsx === undefined) {
    throw new Error("缺少 JSX 测试替身");
  }
  const selection: { value: unknown; selected: unknown[] } = { value: null, selected: [] };
  Object.defineProperty(globalThis, "jsx", {
    configurable: true,
    value: (tag: unknown, props: Record<string, unknown> | null, child: unknown) => {
      if (tag === For && typeof child === "function") {
        selection.selected.length = 0;
        for (const locale of SUPPORTED_LOCALES) {
          child(() => locale);
        }
      } else if (tag === "select") {
        selection.value = props?.value;
      } else if (tag === "option" && props?.selected === true) {
        selection.selected.push(props.value);
      }
      return null;
    },
  });
  try {
    createRoot((dispose) => {
      try {
        const render: unknown = LocaleSessionContext({
          value: session,
          get children() {
            return LanguagePicker({ label: "Language", catalog: CATALOG });
          },
        });
        if (typeof render !== "function") {
          throw new Error("语言作用域缺少渲染访问器");
        }
        render();
      } finally {
        dispose();
      }
    });
    return selection;
  } finally {
    Object.defineProperty(globalThis, "jsx", jsx);
  }
}

test("ja → ko → ja 只提交最后一次选择，同语言的旧失败无效", async () => {
  const { session, request, stored, cleanup } = localeHarness();
  try {
    session.switch(CATALOG, "ja");
    session.switch(CATALOG, "ko");
    session.switch(CATALOG, "ja");

    request(0).resolve(new Response(null, { status: 503 }));
    await settleRequests();
    expect(session.error()).toBeNull();
    expect(session.current()).toBe("en");
    expect(session.selected()).toBe("ja");
    expect(stored.get(LANGUAGE_STORAGE_KEY)).toBe("en");
    expect(request(0).signal?.aborted).toBe(true);
    expect(request(1).signal?.aborted).toBe(true);
    expect(request(2).signal?.aborted).toBe(false);

    request(2).resolve(new Response("greeting = こんにちは\n"));
    await settleRequests();
    request(1).resolve(new Response("greeting = 안녕하세요\n"));
    await settleRequests();
    expect(session.current()).toBe("ja");
    expect(session.selected()).toBe("ja");
    expect(session.error()).toBeNull();
    expect(t("greeting")).toBe("こんにちは");
    expect(stored.get(LANGUAGE_STORAGE_KEY)).toBe("ja");
  } finally {
    cleanup();
  }
});

test("选择器跟随在途目标，失败或选回当前语言后回到已生效语言", async () => {
  const { session, request, stored, cleanup } = localeHarness();
  try {
    expect(pickerSelection(session)).toEqual({ value: "en", selected: ["en"] });
    session.switch(CATALOG, "ja");
    expect(pickerSelection(session)).toEqual({ value: "ja", selected: ["ja"] });
    expect(session.current()).toBe("en");
    expect(t("greeting")).toBe("Hello");
    request(0).resolve(new Response(null, { status: 503 }));
    await settleRequests();
    expect(pickerSelection(session)).toEqual({ value: "en", selected: ["en"] });
    expect(session.error()).toBe("language-switch-failed");
    expect(stored.get(LANGUAGE_STORAGE_KEY)).toBe("en");
    session.switch(CATALOG, "en");
    const errorAfterReselect = session.error();

    session.switch(CATALOG, "ja");
    expect(pickerSelection(session)).toEqual({ value: "ja", selected: ["ja"] });
    session.switch(CATALOG, "en");
    expect(pickerSelection(session)).toEqual({ value: "en", selected: ["en"] });
    request(1).resolve(new Response("greeting = こんにちは\n"));
    await settleRequests();
    expect(pickerSelection(session)).toEqual({ value: "en", selected: ["en"] });
    expect(session.current()).toBe("en");
    expect(session.error()).toBeNull();
    expect(errorAfterReselect).toBeNull();
    expect(t("greeting")).toBe("Hello");
    expect(stored.get(LANGUAGE_STORAGE_KEY)).toBe("en");
    expect(request(1).signal?.aborted).toBe(true);
  } finally {
    cleanup();
  }
});

test("会话卸载时中止在途译文，迟到成功不写入语言或偏好", async () => {
  const { session, request, stored, dispose, cleanup } = localeHarness();
  try {
    session.switch(CATALOG, "ja");
    dispose();
    expect(request(0).signal?.aborted).toBe(true);
    request(0).resolve(new Response("greeting = こんにちは\n"));
    await settleRequests();
    expect(session.current()).toBe("en");
    expect(stored.get(LANGUAGE_STORAGE_KEY)).toBe("en");
  } finally {
    cleanup();
  }
});

test("translation_failures_panic_in_debug_builds：调试构建下翻译失败抛出错误", () => {
  const previousDev = process.env.DEV;
  process.env.DEV = "true";
  try {
    expect(() => translationFailed("missing-key", "unknown variable")).toThrow(
      "翻译 missing-key 失败",
    );
  } finally {
    if (previousDev === undefined) {
      delete process.env.DEV;
    } else {
      process.env.DEV = previousDev;
    }
  }
});
