// 外壳回执和来源页共享的一次性 Key 展示围栏。

import { createContext } from "solid-js";
import type { KeyVisibility } from "./session";

/** 订阅当期展示许可及用于作废旧打开动作的版本。 */
export const KeyVisibilityContext = createContext<() => KeyVisibility>(() => ({
  hidden: false,
  revision: 0,
}));
