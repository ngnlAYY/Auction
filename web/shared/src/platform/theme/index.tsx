// 界面主题基础设施：深色（默认，保持既有视觉契约）与浅色两套设计 token。
// 偏好键固定为 `hv.theme`，只接受 `dark` / `light`，缺失、非法或存储不可用时使用深色；
// 主题经 `<html data-theme>` 生效，配色全部由 base.css 与各 surface 的 token 覆盖决定，组件不写颜色。
// 静态启动骨架无法在脚本就绪前读取偏好，因此始终以深色显示。

import type { JSX } from "@solidjs/web";
import { createContext, createEffect, useContext } from "solid-js";

import { type Cell, createCell } from "../../reactive/cell";
import { getItem, setItem } from "../storage";

/** 主题偏好的 `localStorage` 键；Public 与 Admin 同源共享，不得重命名。 */
const THEME_STORAGE_KEY = "hv.theme";

/** 受支持的界面主题。 */
export type Theme = "dark" | "light";

/** 存储值与 `<html data-theme>` 属性值。 */
export function themeCode(theme: Theme): "dark" | "light" {
  return theme;
}

/** 只接受闭集取值，调用方对 `null` 回退默认主题。 */
export function themeParse(code: string): Theme | null {
  if (code === "dark" || code === "light") {
    return code;
  }
  return null;
}

/** 浏览器界面色（`<meta name="theme-color">`），与 base.css 中对应主题的 `--color-bg` 一致。 */
function themeBrowserColor(theme: Theme): string {
  return theme === "dark" ? "#0e1116" : "#f6f8fa";
}

/** 根组件提供的当前主题；`ThemePicker` 通过上下文读写，避免各页面各自持有状态。 */
export interface ThemeState {
  readonly theme: Cell<Theme>;
}

/** 外壳把 `useThemeEffects()` 的结果放入此上下文，供主题切换器读写同一状态。 */
export const ThemeStateContext = createContext<ThemeState>();

/** 读取已保存的主题偏好；存储不可用或值非法时返回 `null`。 */
function storedTheme(): Theme | null {
  const value = getItem(THEME_STORAGE_KEY);
  return value === null ? null : themeParse(value);
}

/** 把主题同步到 `<html data-theme>` 与浏览器界面色。 */
function applyTheme(theme: Theme): void {
  if (typeof document === "undefined") {
    return;
  }
  try {
    document.documentElement?.setAttribute("data-theme", themeCode(theme));
    document
      .querySelector('meta[name="theme-color"]')
      ?.setAttribute("content", themeBrowserColor(theme));
  } catch {
    // 属性写入失败只影响主题呈现，不改变存储偏好或当前交互状态。
  }
}

/**
 * 各 surface 根组件调用一次：恢复已保存主题并在主题变化时同步 DOM。
 * 根组件须把返回值放入 `ThemeStateContext`；首次渲染前即写入属性，避免先闪现默认配色。
 */
export function useThemeEffects(): ThemeState {
  const restored = storedTheme() ?? "dark";
  applyTheme(restored);
  const theme = createCell(restored);
  createEffect(
    () => theme(),
    (current) => applyTheme(current),
  );
  return { theme };
}

/** 主题切换器；写入偏好失败只影响下次启动恢复，不影响本次切换。 */
export function ThemePicker(props: {
  label: string;
  darkLabel: string;
  lightLabel: string;
}): JSX.Element {
  const state = useContext(ThemeStateContext);
  const current = () => state.theme();
  return (
    <label class="theme-picker">
      <span class="theme-picker__label">{props.label}</span>
      <select
        id="theme"
        name="theme"
        class="theme-picker__select"
        value={themeCode(current())}
        onChange={(event) => {
          const selected = themeParse(event.currentTarget.value) ?? "dark";
          state.theme.set(selected);
          setItem(THEME_STORAGE_KEY, themeCode(selected));
        }}
      >
        <option value={themeCode("dark")} selected={current() === "dark"}>
          {props.darkLabel}
        </option>
        <option value={themeCode("light")} selected={current() === "light"}>
          {props.lightLabel}
        </option>
      </select>
    </label>
  );
}
