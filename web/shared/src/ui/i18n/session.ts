// 语言会话、当前 Fluent bundle 与翻译失败处理。
// 连续切换只应用最后一个目标；会话任务随根组件销毁而取消。

import type { FluentBundle, FluentVariable } from "@fluent/bundle";
import { createContext, createEffect, useContext } from "solid-js";

import { setItem } from "../../platform/storage";
import { type Cell, createCell } from "../../reactive/cell";
import { createTaskScope } from "../../reactive/tasks";
import { err, ok } from "../../result";
import { reportBrowserError } from "../bootstrap";
import {
  initialLanguage,
  LANGUAGE_STORAGE_KEY,
  type LocaleCatalog,
  type LocaleCode,
  parseSupported,
} from "./catalog";
import { buildBundle, errorText, fetchCatalogLocale } from "./resource";

/** 语言切换失败时显示的稳定译键。 */
export type LocaleError = "language-switch-failed";

interface LocaleState {
  readonly current: LocaleCode;
  readonly pending: LocaleCode | null;
  readonly ready: boolean;
  readonly error: LocaleError | null;
}

/** 当前 bundle 与 `t` 共用；成功切换语言后替换，订阅者随信号重新格式化文案。 */
const activeBundle: Cell<FluentBundle | null> = createCell(null);
let i18nFailureReported = false;

/** 翻译消息；参数数字先转成无分组十进制文本，避免 Fluent 按语言插入千分位。 */
export function t(id: string, args?: Record<string, string | number>): string {
  const bundle = activeBundle();
  if (bundle === null) {
    return translationFailed(id, "语言资源尚未初始化");
  }
  const message = bundle.getMessage(id);
  if (message === undefined || message.value === null) {
    return translationFailed(id, `缺少消息 ${id}`);
  }
  const errors: Error[] = [];
  let formatted: string;
  try {
    formatted = bundle.formatPattern(
      message.value,
      args === undefined ? null : fluentArguments(args),
      errors,
    );
  } catch (failure) {
    return translationFailed(id, failure);
  }
  const firstError = errors[0];
  if (firstError !== undefined) {
    return translationFailed(id, firstError);
  }
  return withoutFluentIsolation(formatted);
}

/** 开发构建立即暴露翻译错误；发布构建显示消息 ID，且页面生命周期只上报一次。 */
export function translationFailed(id: string, error: unknown): string {
  if (import.meta.env.DEV) {
    throw new Error(`翻译 ${id} 失败：${errorText(error)}`);
  }
  if (!i18nFailureReported) {
    i18nFailureReported = true;
    reportBrowserError("i18n_format");
  }
  return id;
}

/** 移除 Fluent 自动包裹动态变量的最外层 FSI/PDI，保留变量内部成对的隔离控制符。 */
export function withoutFluentIsolation(value: string): string {
  if (!value.includes("\u2068") && !value.includes("\u2069")) {
    return value;
  }
  let depth = 0;
  let result = "";
  for (const character of value) {
    if (character === "\u2068" && depth === 0) {
      depth = 1;
      continue;
    }
    if (character === "\u2068") {
      depth += 1;
      result += character;
      continue;
    }
    if (character === "\u2069" && depth === 1) {
      depth = 0;
      continue;
    }
    if (character === "\u2069" && depth > 1) {
      depth -= 1;
    }
    result += character;
  }
  return result;
}

/** 按需语言会话：持有当前 bundle 与语言信号；根组件创建一次并经 context 提供。 */
export interface LocaleSession {
  current(): LocaleCode;
  isReady(): boolean;
  error(): LocaleError | null;
  /** 首屏加载：默认语言直接就绪；已保存的非默认语言先取回再渲染。 */
  bootstrap(catalog: LocaleCatalog): void;
  /** 切换语言并持久化；非法语言码回退默认语言。 */
  switch(catalog: LocaleCatalog, code: string): void;
}

/** 根组件创建一次会话并经 context 提供；会话任务随根组件销毁而取消。 */
export function createLocaleSession(
  initialBundle: FluentBundle,
  defaultLocale: LocaleCode,
): LocaleSession {
  activeBundle.set(initialBundle);
  const state = createCell<LocaleState>({
    current: defaultLocale,
    pending: null,
    ready: false,
    error: null,
  });
  const tasks = createTaskScope();

  const current = (): LocaleCode => state().current;
  const isReady = (): boolean => state().ready;
  const error = (): LocaleError | null => state().error;

  const load = (catalog: LocaleCatalog, target: LocaleCode, persist: boolean): void => {
    state.update((previous) => ({
      ...previous,
      pending: target,
      error: null,
    }));
    tasks.spawn(async (context) => {
      const fetched =
        target === catalog.defaultLocale
          ? ok<string | null>(null)
          : await fetchCatalogLocale(catalog, target, context);
      const built = fetched.ok
        ? buildBundle(catalog.defaultLocale, catalog.defaultFtl, target, fetched.value)
        : err<string>(fetched.error);
      if (!context.alive()) {
        return;
      }
      // 连续切换只应用最后一次选择：被更新目标替换的加载结果直接丢弃。
      if (state.peek().pending !== target) {
        return;
      }
      if (built.ok) {
        state.set({
          current: target,
          pending: null,
          ready: true,
          error: null,
        });
        activeBundle.set(built.value);
        if (persist) {
          setItem(LANGUAGE_STORAGE_KEY, target);
        }
        return;
      }
      state.update((previous) => ({
        ...previous,
        pending: null,
        ready: true,
        error: "language-switch-failed",
      }));
    });
  };

  const bootstrap = (catalog: LocaleCatalog): void => {
    const target = initialLanguage(catalog.defaultLocale);
    if (target === catalog.defaultLocale) {
      state.update((previous) => ({ ...previous, ready: true }));
      return;
    }
    load(catalog, target, false);
  };

  const switchLocale = (catalog: LocaleCatalog, code: string): void => {
    const target = parseSupported(code) ?? catalog.defaultLocale;
    if (target === current()) {
      // 沿用源码：即使传入非法 code，当前已是回退目标时仍原样保存选择值。
      setItem(LANGUAGE_STORAGE_KEY, code);
      return;
    }
    load(catalog, target, true);
  };

  return {
    current,
    isReady,
    error,
    bootstrap,
    switch: switchLocale,
  };
}

export const LocaleSessionContext = createContext<LocaleSession>();

export function useLocaleSession(): LocaleSession {
  return useContext(LocaleSessionContext);
}

/** 当前语言；组件内读取会订阅变化。 */
export function currentLanguage(): LocaleCode {
  return useLocaleSession().current();
}

/** 把当前语言同步到 `<html lang>`；屏幕阅读器、翻译提示与 `:lang()` 都依赖该属性。 */
export function syncDocumentLanguage(language: string): void {
  if (typeof document === "undefined") {
    return;
  }
  // 入口文档的 lang 只是默认语言的静态兜底，前端启动前无法按 hv.lang 修正。
  document.documentElement?.setAttribute("lang", language);
}

/** 语言变化时同步 `<html lang>`；各 surface 根组件在创建会话后调用一次。 */
export function useLocaleEffects(session: LocaleSession): void {
  createEffect(
    () => session.current(),
    (language) => syncDocumentLanguage(language),
  );
}

function fluentArguments(args: Record<string, string | number>): Record<string, FluentVariable> {
  const values: Record<string, FluentVariable> = {};
  for (const [name, value] of Object.entries(args)) {
    values[name] = typeof value === "number" ? displayNumber(value) : value;
  }
  return values;
}

function displayNumber(value: number): string {
  if (Number.isNaN(value)) {
    return "NaN";
  }
  if (value === Number.POSITIVE_INFINITY) {
    return "inf";
  }
  if (value === Number.NEGATIVE_INFINITY) {
    return "-inf";
  }
  if (Object.is(value, -0)) {
    return "-0";
  }
  if (Number.isSafeInteger(value)) {
    return String(value);
  }
  // Rust 的 f64 Display 始终写十进制；JS 在极大或极小时改用指数，这里把指数展开。
  return expandDecimalExponent(String(value));
}

function expandDecimalExponent(text: string): string {
  const match = /^(-?)(\d+)(?:\.(\d+))?e([+-]?\d+)$/.exec(text);
  if (match === null) {
    return text;
  }
  const [, sign, whole, fraction = "", exponentText] = match;
  if (sign === undefined || whole === undefined || exponentText === undefined) {
    return text;
  }
  const digits = whole + fraction;
  const decimalIndex = whole.length + Number(exponentText);
  if (decimalIndex <= 0) {
    return `${sign}0.${"0".repeat(-decimalIndex)}${digits}`;
  }
  if (decimalIndex >= digits.length) {
    return `${sign}${digits}${"0".repeat(decimalIndex - digits.length)}`;
  }
  return `${sign}${digits.slice(0, decimalIndex)}.${digits.slice(decimalIndex)}`;
}
