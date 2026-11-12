fn main() {
    // Layout and painting recurse with the depth of the document. Windows gives the main thread
    // 1 MiB by default; reserve 32 MiB of address space instead (committed lazily, so this costs
    // no RAM until it is used).
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:33554432");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
