// Embeds the icon and version information into the Windows executables.
fn main() {
    println!("cargo:rerun-if-changed=assets/rocketzip.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/rocketzip.ico")
            .set("ProductName", "RocketZip")
            .set("FileDescription", "RocketZip archiver")
            .set("CompanyName", "RocketZip")
            .set("LegalCopyright", "RocketZip contributors");
        if let Err(e) = res.compile() {
            // Missing resource compiler: still build, just without the embedded icon.
            println!("cargo:warning=icon not embedded ({e}); install the Windows SDK to include it");
        }
    }
}
