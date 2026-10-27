fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Keep Makepad on its native event thread. The same bounded Document history
    // can exceed Windows' default console/GUI stack during Serde validation.
    // This reserves address space for this binary only, without changing the
    // document limits, solver capacity, or linker default stack commit size.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
    {
        println!("cargo:rustc-link-arg-bin=ai_revit_desktop=/STACK:8388608");
    }
}
