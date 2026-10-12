fn main() {
    println!("cargo:rerun-if-changed=assets/Info.plist");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let manifest_dir =
            std::env::var("CARGO_MANIFEST_DIR").expect("Cargo must provide the package directory");
        println!(
            "cargo:rustc-link-arg-bin=vu=-Wl,-sectcreate,__TEXT,__info_plist,{manifest_dir}/assets/Info.plist"
        );
    }
}
