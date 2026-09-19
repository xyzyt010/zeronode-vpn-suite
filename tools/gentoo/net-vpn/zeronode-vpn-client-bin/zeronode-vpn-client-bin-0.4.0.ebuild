# Copyright 2026 ZeroNode
# Distributed under the terms of the MIT license.

EAPI=8

inherit systemd

DESCRIPTION="ZeroNode VPN desktop client - WireGuard/OpenVPN/Shadowsocks/PPTP/Tor (egui, X11+Wayland)"
HOMEPAGE="https://github.com/xyzyt010/zeronode-vpn-suite"
MY_PV="0.4.0"

SRC_URI="
	amd64? ( https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v${MY_PV}/zeronode-vpn-client-${MY_PV}-gentoo-x86_64.tar.gz )
	arm64? ( https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/v${MY_PV}/zeronode-vpn-client-${MY_PV}-gentoo-aarch64.tar.gz )
"
S="${WORKDIR}/zeronode-vpn-client-${MY_PV}-gentoo-$(usex amd64 x86_64 aarch64)"

LICENSE="MIT"
SLOT="0"
KEYWORDS="~amd64 ~arm64"
RESTRICT="mirror strip"

RDEPEND="
	net-misc/iproute2
	net-firewall/iptables
	sys-apps/kmod
	sys-auth/polkit
	dev-libs/libevent
	dev-libs/openssl
	sys-libs/zlib
	x11-libs/gtk+:3
	x11-libs/libX11
	x11-libs/libxdo
"
BDEPEND=""

src_install() {
	dobin vpn-client
	domenu io.zeronode.vpn.desktop
	doicon -s 512 io.zeronode.vpn.png
	insinto /usr/share/vpn-client/tor-linux
	doins tor-linux/tor tor-linux/geoip tor-linux/geoip6 tor-linux/lyrebird
	fperms 0755 /usr/share/vpn-client/tor-linux/tor /usr/share/vpn-client/tor-linux/lyrebird
	systemd_dounit zeronode-vpn-helper.service
	newinitd zeronode-vpn-helper.openrc zeronode-vpn-helper
}

pkg_postinst() {
	elog "Enable + start the privileged helper (systemd):"
	elog "  systemctl enable --now zeronode-vpn-helper"
	elog "…or on OpenRC:"
	elog "  rc-update add zeronode-vpn-helper default && rc-service zeronode-vpn-helper start"
	elog "Optional per-protocol packages:"
	elog "  emerge net-vpn/openvpn net-vpn/wireguard-tools net-dialup/ppp net-vpn/tor"
}
