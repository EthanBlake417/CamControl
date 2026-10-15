//! Windows: the GET icon and file details in the .exe itself (Explorer, taskbar).
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../assets/get_logo.ico");
        res.set("ProductName", "CamControl");
        res.set("FileDescription", "CamControl");
        res.set("CompanyName", "Grid Evolution Technologies");
        if let Err(e) = res.compile() {
            println!("cargo:warning=No icon in the exe: {e}");
        }
    }
    println!("cargo:rerun-if-changed=../assets/get_logo.ico");
}
