// Admin 浏览器应用入口：安装错误钩子、按固定顺序加载样式并挂载根组件。

import "../../../crates/auction-web-shared/style/base.css";
import "../../../crates/auction-web-admin/assets/admin.css";
import "../../../crates/auction-web-admin/assets/consignment.css";
import "../../../crates/auction-web-admin/assets/desktop.css";
import "../../../crates/auction-web-admin/assets/mobile.css";
import "../../../crates/auction-web-shared/style/console.css";
// 让构建产出带指纹的站点图标，由后端从 /favicon.ico 映射到该文件。
import "../../../crates/auction-web-shared/assets/favicon.svg?url";

import { installErrorHook } from "@auction/web-shared/ui/bootstrap";

import { run } from "./surface";

// 在挂载应用前安装全局错误兜底：未捕获异常与拒绝后向用户展示静态错误横幅。
installErrorHook();
run();
