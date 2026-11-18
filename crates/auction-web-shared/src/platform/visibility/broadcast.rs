//! 同源标签页之间的固定消息通知；构造失败时由调用方继续使用本地复核。

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "BroadcastChannel")]
    type BrowserChannel;
    #[wasm_bindgen(catch, constructor, js_class = "BroadcastChannel")]
    fn new(name: &str) -> Result<BrowserChannel, JsValue>;
    #[wasm_bindgen(method, setter, js_class = "BroadcastChannel", js_name = "onmessage")]
    fn set_onmessage(this: &BrowserChannel, callback: Option<&js_sys::Function>);
    #[wasm_bindgen(catch, method, js_class = "BroadcastChannel", js_name = "postMessage")]
    fn post_message(this: &BrowserChannel, message: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(method, js_class = "BroadcastChannel")]
    fn close(this: &BrowserChannel);

    #[wasm_bindgen(js_name = "MessageEvent")]
    type BrowserMessage;
    #[wasm_bindgen(method, getter, structural)]
    fn data(this: &BrowserMessage) -> JsValue;
}

/// 只收发一种调用方指定的固定消息。释放监听时关闭浏览器通道。
pub struct BroadcastNotice {
    #[cfg(target_arch = "wasm32")]
    channel: BrowserChannel,
    #[cfg(target_arch = "wasm32")]
    _callback: Closure<dyn FnMut(BrowserMessage)>,
    #[cfg(target_arch = "wasm32")]
    message: &'static str,
}

impl BroadcastNotice {
    /// 打开同源通道。仅合法的固定消息会触发回调；宿主或无法创建通道时返回空。
    pub fn open(
        name: &str,
        message: &'static str,
        on_receive: impl FnMut() + 'static,
    ) -> Option<Self> {
        #[cfg(target_arch = "wasm32")]
        {
            let channel = BrowserChannel::new(name).ok()?;
            let mut on_receive = on_receive;
            let callback =
                Closure::<dyn FnMut(BrowserMessage)>::new(move |event: BrowserMessage| {
                    if matches_notice(event.data().as_string().as_deref(), message) {
                        on_receive();
                    }
                });
            channel.set_onmessage(Some(callback.as_ref().unchecked_ref()));
            Some(Self { channel, _callback: callback, message })
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (name, message, on_receive);
            None
        }
    }

    /// 通知同源的其他订阅者；该消息的字段与内容不能由请求结果覆盖。
    pub fn notify(&self) {
        #[cfg(target_arch = "wasm32")]
        let _ = self.channel.post_message(self.message);
    }
}

#[cfg(target_arch = "wasm32")]
impl Drop for BroadcastNotice {
    fn drop(&mut self) {
        self.channel.set_onmessage(None);
        self.channel.close();
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn matches_notice(received: Option<&str>, expected: &str) -> bool {
    received == Some(expected)
}

#[cfg(test)]
mod tests {
    use super::matches_notice;

    #[test]
    fn notices_match_only_the_exact_message() {
        assert!(matches_notice(Some("session-ended"), "session-ended"));
        assert!(!matches_notice(Some("session-ended:operator"), "session-ended"));
        assert!(!matches_notice(None, "session-ended"));
    }
}
