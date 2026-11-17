// 将浏览器可视区域投影为 CSS 变量，供软键盘与缩放期间的固定弹窗使用。
// 没有 VisualViewport 时保留 CSS 的 vh/dvh 回退；不干预浏览器缩放或页面滚动。

import { onSettled } from "solid-js";

const VIEWPORT_PROPERTIES = [
  "--visual-viewport-width",
  "--visual-viewport-height",
  "--visual-viewport-left",
  "--visual-viewport-top",
] as const;

interface PreviousProperty {
  readonly name: string;
  readonly value: string;
  readonly priority: string;
}

interface ViewportGuard {
  cleanup(): void;
}

/** 滚动事件常不改变可见尺寸；只格式化并写入变化项，不逐项往返读取 CSS 字符串。 */
function synchronize(viewport: VisualViewport, root: HTMLElement, lastDimensions: number[]): void {
  const width = viewport.width;
  const height = viewport.height;
  const dimensions = [width, height, viewport.offsetLeft, viewport.offsetTop];
  if (
    dimensions.some((value) => !Number.isFinite(value)) ||
    width <= 0 ||
    height <= 0 ||
    dimensions.every((value, index) => value === lastDimensions[index])
  ) {
    return;
  }
  const style = root.style;
  for (let index = 0; index < VIEWPORT_PROPERTIES.length; index += 1) {
    const name = VIEWPORT_PROPERTIES[index];
    const value = dimensions[index];
    if (name === undefined || value === undefined || lastDimensions[index] === value) {
      continue;
    }
    try {
      style.setProperty(name, `${value}px`);
      lastDimensions[index] = value;
    } catch {
      // 单项写入失败时保留旧值，下一个尺寸变化仍会重试该项。
    }
  }
}

/** 安装监听并返回卸载守卫；没有 VisualViewport 或 DOM 根时返回 `null`。 */
function installViewportGuard(): ViewportGuard | null {
  if (typeof window === "undefined" || typeof document === "undefined") {
    return null;
  }
  const viewport: VisualViewport | null | undefined = window.visualViewport;
  if (viewport === null || viewport === undefined) {
    return null;
  }
  const root = document.documentElement;
  if (!(root instanceof HTMLElement)) {
    return null;
  }
  const style = root.style;
  const previous: PreviousProperty[] = VIEWPORT_PROPERTIES.map((name) => {
    try {
      return {
        name,
        value: style.getPropertyValue(name),
        priority: style.getPropertyPriority(name),
      };
    } catch {
      return { name, value: "", priority: "" };
    }
  });
  const lastDimensions = [Number.NaN, Number.NaN, Number.NaN, Number.NaN];
  synchronize(viewport, root, lastDimensions);
  const callback = () => synchronize(viewport, root, lastDimensions);
  const restore = () => {
    const currentStyle = root.style;
    for (const property of previous) {
      try {
        if (property.value.length === 0) {
          currentStyle.removeProperty(property.name);
        } else {
          currentStyle.setProperty(property.name, property.value, property.priority);
        }
      } catch {
        // 恢复单项失败不阻止恢复其余属性；原始样式快照保持不变。
      }
    }
  };
  let cleaned = false;
  const cleanup = () => {
    if (cleaned) {
      return;
    }
    cleaned = true;
    viewport.removeEventListener("resize", callback);
    viewport.removeEventListener("scroll", callback);
    restore();
  };

  try {
    viewport.addEventListener("resize", callback);
    viewport.addEventListener("scroll", callback);
  } catch {
    // 安装到一半失败时不能留下半套监听或已写入的变量。
    cleanup();
    return null;
  }
  return { cleanup };
}

/** 每个 surface 的基础样式组件只安装一份监听；卸载时恢复原样式并移除监听。 */
export function useVisualViewport(): void {
  onSettled(() => {
    const guard = installViewportGuard();
    return () => guard?.cleanup();
  });
}
