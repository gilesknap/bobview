//! Give the browser build a bigger stack. usvg and resvg recurse once per
//! level of SVG nesting, about 28 KiB a level when rendering, and the default
//! wasm stack is 1 MiB; overflowing it kills the page. `svg` bounds the
//! nesting it renders, but `<use>` chains that usvg expands while parsing
//! (up to its own limits) still need more than 1 MiB.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        println!("cargo:rustc-link-arg-bins=-zstack-size=8388608");
    }
}
