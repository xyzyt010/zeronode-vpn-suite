# OpenVPN (managed strip-down)

ZeroNode looks for `openvpn.exe` in this order:

1. `ZERONODE_OPENVPN` environment variable  
2. Next to `vpn-client.exe` (`openvpn.exe` or `openvpn/openvpn.exe`)  
3. This folder: `assets/openvpn/openvpn.exe`  
4. App data: `%AppData%/…/bin/openvpn/openvpn.exe`  
5. System install (`Program Files\OpenVPN\bin\openvpn.exe`)  
6. `PATH`  
7. Auto-provision: winget OpenVPN Community, then MSI admin-extract into app data  

If you already have OpenVPN installed, nothing is downloaded.  
To force a local binary, drop `openvpn.exe` (and required DLLs) here or under the managed `bin/openvpn` directory.
