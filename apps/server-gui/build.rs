const REQUIRE_ADMIN_MANIFEST: &str = r#"
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
        <requestedPrivileges>
            <requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
        </requestedPrivileges>
    </security>
</trustInfo>
</assembly>
"#;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let png_bytes = include_bytes!("../../assets/icon.png");
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0]);
    ico.extend_from_slice(&[1, 0]);
    ico.extend_from_slice(&[1, 0]);
    ico.push(0);
    ico.push(0);
    ico.push(0);
    ico.push(0);
    ico.extend_from_slice(&[1, 0]);
    ico.extend_from_slice(&[32, 0]);
    ico.extend_from_slice(&(png_bytes.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22u32.to_le_bytes());
    ico.extend_from_slice(png_bytes);

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest_path = std::path::Path::new(&out_dir).join("icon.ico");
    std::fs::write(&dest_path, ico).unwrap();

    let mut res = winres::WindowsResource::new();
    res.set_icon(dest_path.to_str().unwrap());
    res.set_manifest(REQUIRE_ADMIN_MANIFEST);
    res.set("CompanyName", "ZeroNode");
    res.set("ProductName", "ZeroNode VPN");
    res.set("FileDescription", "ZeroNode VPN server dashboard");
    res.set("InternalName", "vpn-server-gui");
    res.set("OriginalFilename", "vpn-server-gui.exe");
    res.set(
        "Comments",
        "ZeroNode VPN server dashboard requiring admin elevation for service management",
    );
    res.set("LegalCopyright", "Copyright © 2026 ZeroNode");
    res.compile().unwrap();
}
