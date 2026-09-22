fn main() {
    println!("cargo:rerun-if-changed=assets/windows/snail.rc");
    println!("cargo:rerun-if-changed=assets/windows/snail.ico");

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // rustc embeds the process manifest on MSVC targets. This resource adds Snail's icon and
        // version metadata without introducing a second MANIFEST/1 entry.
        embed_resource::compile_for(
            "assets/windows/snail.rc",
            &["snail", "snail-cli"],
            embed_resource::NONE,
        )
        .manifest_optional()
        .expect("compile Snail's Windows icon and version metadata");
    }
}
