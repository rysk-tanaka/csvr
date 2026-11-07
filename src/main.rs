mod app;
#[cfg(target_family = "wasm")]
mod browser;
mod chart;
#[cfg(not(target_family = "wasm"))]
mod native;
#[cfg(target_family = "wasm")]
mod web;
#[cfg(target_family = "wasm")]
mod welcome;

fn main() {
    #[cfg(not(target_family = "wasm"))]
    native::run();
    #[cfg(target_family = "wasm")]
    web::run();
}
