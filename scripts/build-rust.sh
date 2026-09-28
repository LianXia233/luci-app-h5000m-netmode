#!/usr/bin/env bash
#
# Build the Rust backend from src/ and install it into the package tree.
#
# root/usr/sbin/h5000m-netmode* is a prebuilt artefact that lives in git, so a
# change to src/ can quietly ship as an old binary: the package still installs,
# the service still starts, and only the behaviour is wrong. This script is the
# way back from src/ to the artefact - run it after touching Rust code and
# commit the result, or let scripts/build-release.sh run it for you.
#
# Usage:
#   scripts/build-rust.sh [target]
#
# Target defaults to the one this package ships (aarch64, musl, static).
# Set H5000M_RUST_TARGET to cross-compile for something else.

set -euo pipefail

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target="${1:-${H5000M_RUST_TARGET:-aarch64-unknown-linux-musl}}"
out_dir="${repo_dir}/target/${target}/release"

cd "${repo_dir}"

command -v cargo >/dev/null 2>&1 || {
	echo "::error::cargo not found; install the Rust toolchain first" >&2
	exit 1
}

# rustup is optional (distro cargo works too), and the target may already be
# installed; neither failure is fatal here.
rustup target add "${target}" 2>/dev/null || true

# rust-lld lets the musl target link without a cross C toolchain. Fall back to
# the default linker when it is unavailable, so a host build still works.
if ! RUSTFLAGS="-C linker=rust-lld" cargo build --release --target "${target}"; then
	echo "rust-lld unavailable, retrying with the default linker" >&2
	cargo build --release --target "${target}"
fi

for bin in h5000m-netmode h5000m-netmode-status; do
	built="${out_dir}/${bin}"
	[ -f "${built}" ] || {
		echo "::error::${built} was not produced" >&2
		exit 1
	}
	file "${built}" | grep -q 'statically linked' || {
		echo "::error::${built} is not statically linked; it would fail on a musl router" >&2
		exit 1
	}
	install -m 0755 "${built}" "${repo_dir}/root/usr/sbin/${bin}"
done

echo "installed $(cd "${repo_dir}" && git --no-pager diff --stat -- root/usr/sbin 2>/dev/null | tail -n 1 || true)"
echo "root/usr/sbin/h5000m-netmode{,-status} rebuilt from src/ for ${target}"
