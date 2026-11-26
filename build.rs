// Icône et informations de version du programme Windows (mam3d.exe).
fn main() {
    println!("cargo:rerun-if-changed=assets/mam3d.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/mam3d.ico")
            .set("ProductName", "mam3d")
            .set("FileDescription", "mam3d - images 2D en modèles 3D pour Blender")
            .set("CompanyName", "mam3d")
            .set("LegalCopyright", "mam3d");
        // Sans outil de ressources Windows, le programme se construit quand même (sans icône).
        if let Err(e) = res.compile() {
            println!("cargo:warning=icône non intégrée : {e}");
        }
    }
}
