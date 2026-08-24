Name:           zeronode-vpn-client
Version:        0.2.0
Release:        1%{?dist}
Summary:        ZeroNode VPN desktop client — WireGuard/OpenVPN/Shadowsocks/PPTP/Tor (egui+glow, X11+Wayland)
License:        MIT
URL:            https://github.com/xyzyt010/zeronode-vpn-suite
Source0:        %{name}-%{version}.tar.gz
BuildRequires:  cargo, rust, pkg-config
Requires:       nftables, iproute, kmod, polkit, hicolor-icon-theme
Requires:       xdg-desktop-portal
Recommends:     openvpn, wireguard-tools, pptp, ppp, dejavu-sans-fonts
# Fedora Recommends via weak deps: suggest when available (F41+)
Recommends:     (xdg-desktop-portal-gtk or xdg-desktop-portal-kde)
Suggests:       libayatana-appindicator-gtk3

%description
ZeroNode VPN desktop client. 5 protocols (WireGuard kernel+boringtun, OpenVPN, Shadowsocks/Outline, PPTP, Tor expert bundle 15.0.17 tun2proxy), pixel-identical to Windows on Fedora Workstation X11 and GNOME Wayland (single binary winit 0.30 x11+wayland, pkexec).

%prep
%autosetup -p1

%build
# Prefer prebuilt glibc 2.31 binary if present (from zigbuild), else build
if [ -f target/x86_64-unknown-linux-gnu/release/vpn-client ]; then
  echo "Using prebuilt x86_64-unknown-linux-gnu.2.31 binary"
else
  cargo build --release -p vpn-client
fi

%install
BIN_SRC="target/x86_64-unknown-linux-gnu/release/vpn-client"
[ -f "$BIN_SRC" ] || BIN_SRC="target/release/vpn-client"
install -Dm755 "$BIN_SRC" "%{buildroot}%{_bindir}/vpn-client"
install -Dm644 "apps/client/assets/debian/io.zeronode.vpn.desktop" "%{buildroot}%{_datadir}/applications/io.zeronode.vpn.desktop"
install -Dm644 "assets/icon.png" "%{buildroot}%{_datadir}/icons/hicolor/512x512/apps/io.zeronode.vpn.png"
if [ -f "apps/client/assets/tor-linux/tor" ]; then
  install -Dm755 "apps/client/assets/tor-linux/tor" "%{buildroot}%{_datadir}/vpn-client/tor-linux/tor"
  install -Dm644 "apps/client/assets/tor-linux/geoip" "%{buildroot}%{_datadir}/vpn-client/tor-linux/geoip"
  install -Dm644 "apps/client/assets/tor-linux/geoip6" "%{buildroot}%{_datadir}/vpn-client/tor-linux/geoip6"
fi
if [ -d "apps/client/assets/flags" ]; then
  mkdir -p "%{buildroot}%{_datadir}/vpn-client/flags"
  cp -a "apps/client/assets/flags/"* "%{buildroot}%{_datadir}/vpn-client/flags/" 2>/dev/null || true
fi
install -Dm644 README.md "%{buildroot}%{_datadir}/doc/%{name}/README.md"

%files
%{_bindir}/vpn-client
%{_datadir}/applications/io.zeronode.vpn.desktop
%{_datadir}/icons/hicolor/512x512/apps/io.zeronode.vpn.png
%{_datadir}/vpn-client/
%{_datadir}/doc/%{name}/README.md
%license LICENSE

%changelog
* Sun Aug 24 2026 ZeroNode <zeronode@local> - 0.2.0-1
- Multi-distro: Fedora 40/41/42 X11+Wayland, Debian, Arch, same binary
