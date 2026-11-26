// L'installateur embarque mam3d.exe (chemin donné par MAM3D_EXE) et porte l'icône de mam3d.
fn main() {
    println!("cargo:rerun-if-env-changed=MAM3D_EXE");
    println!("cargo:rerun-if-env-changed=MAM3D_VERSION");
    println!("cargo:rerun-if-changed=../assets/mam3d.ico");
    match std::env::var("MAM3D_EXE") {
        Ok(p) => println!("cargo:rerun-if-changed={p}"),
        Err(_) => panic!("MAM3D_EXE doit indiquer le mam3d.exe à embarquer : lancez ../make_installer.ps1"),
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../assets/mam3d.ico")
            .set("ProductName", "mam3d")
            .set("FileDescription", "Installateur de mam3d")
            .set("CompanyName", "mam3d")
            // Installation pour l'utilisateur seul : pas besoin des droits administrateur.
            .set_manifest(
                r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges>
<requestedExecutionLevel level="asInvoker" uiAccess="false"/>
</requestedPrivileges></security></trustInfo></assembly>"#,
            );
        if let Err(e) = res.compile() {
            println!("cargo:warning=icône non intégrée : {e}");
        }
    }
}
