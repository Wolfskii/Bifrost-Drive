fn main() {
    if std::env::var_os("CARGO_FEATURE_NATIVE").is_some() {
        winfsp_wrs_build::build();
    }
}
