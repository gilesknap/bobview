//! Give the browser build a bigger stack. usvg and resvg recurse once per
//! level of SVG nesting, about 28 KiB a level when rendering, and the default
//! wasm stack is 1 MiB; overflowing it kills the page. `svg` bounds the
//! nesting and references it parses and renders well inside 1 MiB, so this
//! is only headroom: in Chromium, deep recursion usually reaches the
//! browser's own stack limit before 8 MiB of wasm stack, and neither is
//! survivable.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo:rustc-link-arg-bins=-zstack-size=8388608");
    }
}
