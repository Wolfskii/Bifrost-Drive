fn main() {
    // The COM entry points (DllGetClassObject and friends) are exported by rustc without the
    // PRIVATE keyword the linker would like to see for these well-known names. The warning is
    // harmless noise.
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        println!("cargo:rustc-cdylib-link-arg=/IGNORE:4104");
    }
}
