include $(TOPDIR)/rules.mk

PKG_NAME:=luci-app-h5000m-netmode
PKG_VERSION:=1.8.2
PKG_RELEASE:=1
PKG_LICENSE:=Apache-2.0
PKG_LICENSE_FILES:=LICENSE

# The backend is a Rust crate (src/ + Cargo.toml). The OpenWrt buildroot has no
# Rust toolchain, so the package ships the prebuilt static ELF under
# root/usr/sbin. Rebuild it from source before packaging:
#
#   scripts/build-rust.sh
#
# scripts/build-release.sh does this automatically on every release build, and
# CI both reports drift between src/ and the committed binaries and, on main,
# commits freshly built ones.
LUCI_TITLE:=H5000M network priority switch
LUCI_DEPENDS:=+luci-base
LUCI_PKGARCH:=all

define Package/luci-app-h5000m-netmode/conffiles
/etc/config/h5000m_netmode
endef

include $(TOPDIR)/feeds/luci/luci.mk

# call BuildPackage - OpenWrt buildroot signature
