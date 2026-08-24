// Always prompt UAC / run elevated: system-wide OpenVPN (DCO/TAP + routes) and
// Tor (Wintun + tun2proxy) both require Administrator on Windows.
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
    // Always re-run when flag assets change.
    println!("cargo:rerun-if-changed=assets/flags");

    // Stage country flag PNGs next to the built binary (target/{debug,release}/assets/flags).
    // get_flag_uri() looks in exe_dir/assets/flags first.
    stage_flag_assets();

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
    res.set("FileDescription", "ZeroNode VPN desktop client");
    res.set("InternalName", "vpn-client");
    res.set("OriginalFilename", "vpn-client.exe");
    res.set(
        "Comments",
        "ZeroNode VPN client with admin-elevated hosting and tunnel controls",
    );
    res.set("LegalCopyright", "Copyright © 2026 ZeroNode");
    res.compile().unwrap();
}

/// Copy `assets/flags/*.{png,svg}` into `CARGO_TARGET_DIR/{profile}/assets/flags`
/// so the running exe can resolve flag images without depending on cwd.
fn stage_flag_assets() {
    let manifest_dir = match std::env::var_os("CARGO_MANIFEST_DIR") {
        Some(d) => std::path::PathBuf::from(d),
        None => return,
    };
    let src = manifest_dir.join("assets").join("flags");
    if !src.is_dir() {
        return;
    }

    // OUT_DIR looks like .../target/{debug|release}/build/vpn-client-*/out
    // Walk up until we find a directory that already has (or will host) the binary.
    let out_dir = match std::env::var_os("OUT_DIR") {
        Some(d) => std::path::PathBuf::from(d),
        None => return,
    };
    // Prefer PROFILE + CARGO_TARGET_DIR when available.
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            // Fallback: OUT_DIR/../../.. is typically target/{profile}
            out_dir
                .ancestors()
                .nth(3)
                .map(|p| p.to_path_buf())
        });

    let Some(target_root) = target_dir else {
        return;
    };
    // If we got target/{profile} via ancestors, use it directly; if CARGO_TARGET_DIR,
    // append profile.
    let stage_base = if target_root.ends_with(&profile) {
        target_root
    } else {
        target_root.join(&profile)
    };
    let dest = stage_base.join("assets").join("flags");
    let _ = std::fs::create_dir_all(&dest);

    if let Ok(entries) = std::fs::read_dir(&src) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if ext.eq_ignore_ascii_case("png") || ext.eq_ignore_ascii_case("svg") {
                if let Some(name) = path.file_name() {
                    let _ = std::fs::copy(&path, dest.join(name));
                }
            }
        }
    }
}
