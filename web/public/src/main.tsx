//! Public 浏览器入口：安装错误钩子、按固定顺序加载样式并挂载根组件。

import "../../../crates/auction-web-shared/style/base.css";
import "../../../crates/auction-web-public/assets/public.css";
import "../../../crates/auction-web-public/assets/desktop.css";
import "../../../crates/auction-web-public/assets/mobile.css";
import "../../../crates/auction-web-shared/style/console.css";
// Solid 版专属的手机样式，排在共用样式之后；Rust 版不加载。
import "./styles/mobile-topbar.css";
import "./styles/mobile-bid-log.css";

import { installErrorHook } from "@auction/web-shared/ui/bootstrap";
import { render } from "@solidjs/web";

import { App } from "./surface/router";

installErrorHook();

const root = document.getElementById("main");
if (root === null) {
  throw new Error("Public 入口缺少 #main 挂载点");
}
render(() => <App />, root);
