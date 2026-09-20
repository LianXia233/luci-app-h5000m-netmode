#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work_dir="${RUNNER_TEMP:-/tmp}/h5000m-netmode-sdk"
output_dir="${repo_dir}/dist-release"
base_url="https://downloads.openwrt.org/snapshots/targets/mediatek/filogic"

mkdir -p "${work_dir}" "${output_dir}"
find "${output_dir}" -mindepth 1 -maxdepth 1 -delete

# Gate before anything is downloaded.  po2lmo deletes its own output when no entry
# survives it (every msgstr equal to its msgid), so the i18n package is built
# successfully, installs successfully, and translates nothing: the plugin keeps
# rendering English strings in a Chinese UI.  v1.6.0 shipped exactly that - the
# released luci-i18n-h5000m-netmode-zh-cn apk contained no .lmo at all.  Failing
# here costs a second; failing after the SDK build costs five minutes and a release.
( cd "${repo_dir}" && python3 tools/check_catalog.py --self-test && python3 tools/check_catalog.py )

cd "${work_dir}"
curl -fsSLO "${base_url}/sha256sums"
archive="$(awk '/openwrt-sdk-.*Linux-x86_64\.tar\.zst$/ { print $2; exit }' sha256sums | sed 's/^\*//')"
test -n "${archive}"
curl -fL --retry 5 "${base_url}/${archive}" -o "${archive}"
grep "[ *]${archive}$" sha256sums | sha256sum -c -
tar --zstd -xf "${archive}"
sdk_dir="$(find "${work_dir}" -maxdepth 1 -type d -name 'openwrt-sdk-*' | head -n 1)"
test -n "${sdk_dir}"

cd "${sdk_dir}"
./scripts/feeds update -a
./scripts/feeds install luci-base

# 关闭 LuCI 的 JS 压缩。
#
# luci.mk 按 CONFIG_LUCI_JSMIN 决定是否对 htdocs/**/*.js 调用 jsmin,而该开关在
# luci-base 的 Kconfig 里 default y,所以发布包里的 netmode.js 一直是压缩产物:
# 设备上 45124 字节 / 123 行,仓库源码 66876 字节 / 1298 行。两者 md5 天然不等,
# 线上排障时没法用 diff 直接比对源码,热更与回滚也只能重新打包。
# 关掉之后包内 js 与源码逐字节一致,代价是包体积增加约 22 KB。
# .config 里的 CONFIG_LUCI_JSMIN 之外这里再关一次 LUCI_MINIFY_JS:后者决定 JsMin
# 宏的展开,两个开关任一失效都不至于让压缩悄悄回来。
sed -i 's/^LUCI_MINIFY_JS?=1/LUCI_MINIFY_JS:=0/' feeds/luci/luci.mk
grep -q '^LUCI_MINIFY_JS:=0' feeds/luci/luci.mk || {
	echo "::error::failed to disable LUCI_MINIFY_JS in feeds/luci/luci.mk"
	exit 1
}

perl -0pi -e 's/(config ALL\n\s+bool "Select all userspace packages by default"\n\s+default )y/${1}n/' Config.in
perl -0pi -e 's/(config TARGET_MULTI_PROFILE\n\s+bool\n\s+default )y/${1}n/; s/(config TARGET_ALL_PROFILES\n\s+bool\n\s+default )y/${1}n/; s/(config TARGET_DEVICE_mediatek_filogic_DEVICE_[^\n]+\n\s+bool\n\s+default )y/${1}n/g' Config-build.in
sed -i 's/^[[:space:]]*default m$/\tdefault n/' Config-build.in

mkdir -p package/h5000m-custom
rsync -a --exclude '.git/' --exclude '.github/' --exclude 'scripts/' --exclude 'dist-release/' "${repo_dir}/" package/h5000m-custom/luci-app-h5000m-netmode/
cat > .config <<'EOF'
CONFIG_TARGET_mediatek=y
CONFIG_TARGET_mediatek_filogic=y
# CONFIG_ALL is not set
# CONFIG_ALL_KMODS is not set
# CONFIG_ALL_NONSHARED is not set
CONFIG_PACKAGE_luci-app-h5000m-netmode=m
CONFIG_LUCI_LANG_zh_Hans=y
# 包内 js 与仓库源码逐字节一致,便于比对线上文件、热更与回滚。
# CONFIG_LUCI_JSMIN is not set
EOF
make defconfig
make package/h5000m-custom/luci-app-h5000m-netmode/compile -j"$(nproc)" V=s

find bin -type f \( -name 'luci-app-h5000m-netmode-*.apk' -o -name 'luci-app-h5000m-netmode_*.ipk' -o -name 'luci-i18n-h5000m-netmode-zh-cn-*.apk' -o -name 'luci-i18n-h5000m-netmode-zh-cn_*.ipk' \) -exec cp -f {} "${output_dir}/" \;
test "$(find "${output_dir}" -type f \( -name '*.apk' -o -name '*.ipk' \) | wc -l)" -ge 2

# 断言:包内的 netmode.js 与仓库源码逐字节一致,证明 JS 压缩确实关掉了。
#
# "关了开关"和"产物里真的没压缩"是两件事:luci.mk 的开关名、默认值或调用点都
# 可能随上游变化,而压缩后的文件照样能装、能跑,页面上看不出任何异常——只有把
# 字节拿出来比对才能发现。压缩版 123 行、未压缩 1084 行,一行 cmp 就能判死。
assert_js_unminified() {
	local apk_file verify_dir inner packed src_js
	src_js="${repo_dir}/htdocs/luci-static/resources/view/h5000m/netmode.js"
	apk_file="$(find "${output_dir}" -maxdepth 1 -name 'luci-app-h5000m-netmode-*.apk' | head -n 1)"
	[ -n "${apk_file}" ] || return 0

	verify_dir="$(mktemp -d)"
	tar -xzf "${apk_file}" -C "${verify_dir}"
	inner="$(find "${verify_dir}" -maxdepth 1 -name 'data.tar.*' | head -n 1)"
	[ -n "${inner}" ] || {
		echo "::error::${apk_file} has no data.tar.* payload; the apk layout changed"
		return 1
	}
	mkdir -p "${verify_dir}/data"
	case "${inner}" in
		*.gz)  tar -xzf "${inner}" -C "${verify_dir}/data" ;;
		*.zst) tar --zstd -xf "${inner}" -C "${verify_dir}/data" ;;
		*) echo "::error::unknown payload compression: ${inner}"; return 1 ;;
	esac
	packed="${verify_dir}/data/www/luci-static/resources/view/h5000m/netmode.js"
	[ -f "${packed}" ] || {
		echo "::error::netmode.js missing from the built apk"; return 1; }

	if ! cmp -s "${src_js}" "${packed}"; then
		echo "::error::packaged netmode.js differs from the source - JS minification is back on"
		echo "  source: $(wc -c < "${src_js}") bytes / $(wc -l < "${src_js}") lines"
		echo "  packed: $(wc -c < "${packed}") bytes / $(wc -l < "${packed}") lines"
		return 1
	fi
	echo "netmode.js is shipped unminified ($(wc -c < "${packed}") bytes)"
	return 0
}

assert_js_unminified

cp public-key.pem "${output_dir}/openwrt-sdk-build.pem"
(cd "${output_dir}" && find . -maxdepth 1 -type f \( -name '*.apk' -o -name '*.ipk' -o -name 'openwrt-sdk-build.pem' \) -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS)
