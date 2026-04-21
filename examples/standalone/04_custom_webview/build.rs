fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        println!("cargo:rustc-link-arg=-z");
        println!("cargo:rustc-link-arg=max-page-size=16384");
    }
}
