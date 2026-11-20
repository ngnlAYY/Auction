// 错误摘要通过独立模块实例使用生产翻译入口，避免修改共享语言包。

declare module "@auction/web-shared/ui/i18n/session.ts?config-diagnostic" {
  /** 在用例的响应式根下装配真实语言会话。 */
  export const createLocaleSession: typeof import("@auction/web-shared/ui/i18n/session").createLocaleSession;
  /** 格式化独立语言会话当前的译文。 */
  export const t: typeof import("@auction/web-shared/ui/i18n/session").t;
}
