pub mod app;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    #[cfg(feature = "wasm-debug")]
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(app::App);
}
