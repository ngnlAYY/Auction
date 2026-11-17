// Public 应用的 Vite 入口配置；全部选项来自共享的 surfaceConfig。

import { defineConfig } from "vite";
import { surfaceConfig } from "../tooling/vite-config";

export default defineConfig(surfaceConfig("public"));
