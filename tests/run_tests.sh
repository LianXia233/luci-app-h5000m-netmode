#!/bin/sh
#
# Deterministic unit tests for the h5000m-netmode backend.
#
# The backend decides which uplink is live and keeps IPv4 and IPv6 on the same
# exit, so the interesting cases are exactly the ones that are unsafe to provoke
# on a production router: policy routing, a foreign IPv6 default, a modem with no
# IPv6 interface, a half-committed switch, a lost hotplug event.  Those cases are
# constructed here instead.
#
# The script under test runs unmodified.  Only uci/ubus/ip/jsonfilter/pgrep/
# logger/ifup/ifdown/ping and the sysfs root are mocked, through the seams the
# script exposes; the interpreter and every other utility are the real ones.
#
# The `ip` mock is a small routing table, not a stub that returns canned answers:
# it keeps `$SC/routes/default4` / `default6` as the live table and reproduces the
# kernel semantics this design depends on (a `replace` is keyed on the metric and
# swaps the slot atomically, an `add` at an occupied metric fails with EEXIST, a
# `del` only matches the exact gateway/device pair).  tests/run_netns.sh pins the
# same six behaviours against a real kernel, so a wrong assumption here shows up
# as a failing test rather than as a silent pass.
#
# All writes land inside the throwaway scenario directory, so running this on a
# live router cannot touch its configuration.
#
# The suite is POSIX sh on purpose: run it on the router to validate against
# BusyBox ash, and in CI to validate against dash.
#
# Usage:
#   sh tests/run_tests.sh [path/to/h5000m-netmode] [name-substring]
# Default target is the installed backend, so the suite can run on the device.
#
# Test: tests/run_tests.sh

set -u

script="${1:-/usr/sbin/h5000m-netmode}"
tests_dir="$(cd "$(dirname "$0")" && pwd)"
mockbin="$tests_dir/mockbin"

if [ ! -f "$script" ]; then
	printf 'backend not found: %s\n' "$script"
	exit 1
fi
chmod +x "$mockbin"/* 2>/dev/null

checks=0
failures=0
SC=""
rc=0

# ---------------------------------------------------------------------------
# assertions
# ---------------------------------------------------------------------------

check_eq() { # <label> <expected> <actual>
	checks=$((checks + 1))
	if [ "$2" != "$3" ]; then
		failures=$((failures + 1))
		printf '  FAIL %s: expected [%s] got [%s]\n' "$1" "$2" "$3"
	fi
}

check_ok() { # <label> <rc>
	checks=$((checks + 1))
	if [ "$2" != "0" ]; then
		failures=$((failures + 1))
		printf '  FAIL %s: condition not satisfied\n' "$1"
	fi
}

check_contains() { # <label> <needle> <file>
	checks=$((checks + 1))
	if [ ! -f "$3" ] || ! grep -q -- "$2" "$3"; then
		failures=$((failures + 1))
		printf '  FAIL %s: [%s] not found in %s\n' "$1" "$2" "${3##*/}"
	fi
}

check_absent() { # <label> <needle> <file>
	checks=$((checks + 1))
	if [ -f "$3" ] && grep -q -- "$2" "$3"; then
		failures=$((failures + 1))
		printf '  FAIL %s: [%s] unexpectedly present in %s\n' "$1" "$2" "${3##*/}"
	fi
}

# ---------------------------------------------------------------------------
# scenario construction
# ---------------------------------------------------------------------------

new_scenario() {
	if [ -n "$SC" ] && [ "${H5_KEEP:-0}" != "1" ]; then rm -rf "$SC"; fi
	SC="$(mktemp -d)"
	mkdir -p "$SC/uci" "$SC/ubus" "$SC/iwinfo" "$SC/routes" "$SC/sysfs" "$SC/addr"
}

uci_set() {
	printf '%s' "$2" > "$SC/uci/$1"
}

uci_del() {
	rm -f "$SC/uci/$1"
}

uci_of() { # <key> -> value or empty
	local v=""
	if [ -f "$SC/uci/$1" ]; then
		IFS= read -r v < "$SC/uci/$1" || true
	fi
	printf '%s' "$v"
}

read_file() { # <path> -> first line, or empty
	local v=""
	if [ -f "$1" ]; then
		IFS= read -r v < "$1" || true
	fi
	printf '%s' "$v"
}

ubus_set() { # <section> <key=rawjson> ...
	local sec="$1"
	shift
	local body=""
	local kv
	for kv in "$@"; do
		body="$body${body:+,}\"${kv%%=*}\":${kv#*=}"
	done
	printf '{%s}' "$body" > "$SC/ubus/$sec.json"
}

ubus_del() {
	rm -f "$SC/ubus/$1.json"
}

# `ubus call network.wireless status` fixture.  Deliberately shaped like the real
# reply - one object per radio, interfaces nested one level below - so the
# wildcard paths the backend reads (`@.*.up`, `@.*.interfaces[*].config.ssid`,
# `@.*.interfaces[*].ifname`) are exercised against the same depth the router
# returns rather than against a flattened convenience object.
wifi_set() { # <radio0-up> <radio1-up> <ssid>
	printf '{"radio0":{"up":%s,"config":{"band":"2g"},"interfaces":[{"section":"default_radio0","ifname":"phy0-ap0","config":{"mode":"ap","ssid":"%s"}}]},' \
		"$1" "$3" > "$SC/wireless.json"
	printf '"radio1":{"up":%s,"config":{"band":"5g"},"interfaces":[{"section":"default_radio1","ifname":"phy1-ap0","config":{"mode":"ap","ssid":"%s"}}]}}' \
		"$2" "$3" >> "$SC/wireless.json"
}

wifi_del() {
	rm -f "$SC/wireless.json"
}

wifi_assoc() { # <ifname> <mac> [<mac> ...]
	local dev="$1"
	shift
	local body="" mac
	for mac in "$@"; do
		body="$body${body:+,}{\"mac\":\"$mac\",\"signal\":-55}"
	done
	printf '{"results":[%s]}' "$body" > "$SC/iwinfo/$dev.json"
}

set_route() { # <slot> <content|''>
	if [ -n "$2" ]; then
		printf '%s\n' "$2" > "$SC/routes/$1"
	else
		rm -f "$SC/routes/$1"
	fi
}

routes() { # <default4> <default6> <get4> <get6>
	set_route default4 "$1"
	set_route default6 "$2"
	set_route get4 "$3"
	set_route get6 "$4"
}

# Per-device addresses, as the kernel would report them:
#   addr eth1 4 192.168.88.187/24
#   addr eth1 6 2409:8800::187/64
# `6l` marks a link-local address, which must never count as an IPv6 address an
# exit can be promoted on.
addr() { # <dev> <4|6|6l> <address>
	printf '%s %s\n' "$2" "$3" >> "$SC/addr/$1"
}

addr_clear() { # <dev>
	rm -f "$SC/addr/$1"
}

netdev() { # <dev> [carrier]
	mkdir -p "$SC/sysfs/$1"
	if [ -n "${2:-}" ]; then
		printf '%s' "$2" > "$SC/sysfs/$1/carrier"
	fi
}

# A transparent-proxy tunnel: a netdev that carries traffic but is not an uplink.
# ARPHRD_NONE (65534) is what a TUN reports, which is how the backend tells a
# tunnel apart from a NIC (ARPHRD_ETHER = 1) without hard-coding device names.
tunnel() { # <dev>
	mkdir -p "$SC/sysfs/$1"
	printf '%s' 65534 > "$SC/sysfs/$1/type"
}

# Reachability: every probe succeeds unless a rule in $SC/ping-fail says otherwise
# (`dev=eth2 fam=6`, `fam=6`, `dev=eth1`, ...).
ping_fail() { # <rule> ...
	local rule
	: > "$SC/ping-fail"
	for rule in "$@"; do
		printf '%s\n' "$rule" >> "$SC/ping-fail"
	done
}

ping_ok() {
	rm -f "$SC/ping-fail"
}

# Make the kernel refuse one routing operation: `route_fail dev=eth2 fam=6
# verb=replace`.
route_fail() { # <rule> ...
	local rule
	: > "$SC/route-fail"
	for rule in "$@"; do
		printf '%s\n' "$rule" >> "$SC/route-fail"
	done
}

route_fail_clear() {
	rm -f "$SC/route-fail"
}

# ---------------------------------------------------------------------------
# running the backend
# ---------------------------------------------------------------------------

# Every run goes through one environment: the seams the backend exposes, plus the
# mock bin directory.  Nothing else about the host is modified.
_run_env() {
	env H5_SCEN="$SC" \
		H5_MOCKBIN="$mockbin" \
		H5000M_LOCK_DIR="$SC/lock" \
		H5000M_LOCK_WAIT="${H5000M_LOCK_WAIT:-0}" \
		H5000M_HEALTH_STATE="$SC/health" \
		H5000M_STATE_FILE="$SC/state" \
		H5000M_SYSFS_NET="$SC/sysfs" \
		H5000M_SELF="$script" \
		H5000M_WORKER="$script" \
		H5000M_FOREGROUND="${H5_FOREGROUND:-0}" \
		${H5_UPTIME:+H5000M_UPTIME_FILE="$H5_UPTIME"} \
		PATH="$mockbin:$PATH" \
		"$@"
}

run() {
	_run_env sh "$script" "$@" > "$SC/out.txt" 2> "$SC/err.txt"
	rc=$?
}

# Run in the background: used where the command is expected to block (a debounced
# hotplug event, a switch worker) and the test needs to do something else while it
# does.
run_bg() {
	_run_env sh "$script" "$@" > "$SC/out-bg.txt" 2> "$SC/err-bg.txt" &
	bg_pid=$!
}

# The same as run(), but forcing the synchronous path of an action that is async
# by default (`set`, `align`), so a test about the switch itself does not have to
# poll for the worker it just started.  The async contract has its own test.
run_wait() {
	H5_FOREGROUND=1
	run "$@"
	H5_FOREGROUND=0
}

sv() { # status value of a key from the last run
	sed -n "s/^$1=//p" "$SC/out.txt" | head -n 1
}

state_of() { # value of a key in the task state file
	local v=""
	if [ -f "$SC/state" ]; then
		while IFS= read -r line; do
			case "$line" in
				"$1="*) v="${line#"$1="}" ;;
			esac
		done < "$SC/state"
	fi
	printf '%s' "$v"
}

health_of() { # value of a key in the health cache
	local v=""
	if [ -f "$SC/health" ]; then
		while IFS= read -r line; do
			case "$line" in
				"$1="*) v="${line#"$1="}" ;;
			esac
		done < "$SC/health"
	fi
	printf '%s' "$v"
}

writes_has() {
	[ -f "$SC/uci-writes.log" ] && grep -q "^$1\$" "$SC/uci-writes.log"
}

writes_count() {
	if [ -f "$SC/uci-writes.log" ]; then
		grep -c "^$1\$" "$SC/uci-writes.log"
	else
		echo 0
	fi
}

writes_total() {
	if [ -f "$SC/uci-writes.log" ]; then
		grep -c . "$SC/uci-writes.log"
	else
		echo 0
	fi
}

route_ops() { # number of live routing operations performed
	if [ -f "$SC/route-actions.log" ]; then
		grep -c . "$SC/route-actions.log"
	else
		echo 0
	fi
}

route_op_has() { # <exact operation>
	[ -f "$SC/route-actions.log" ] && grep -q -e "$1" "$SC/route-actions.log"
}

# A default route is removed only in the two places that must not blackhole the
# router: reaping a route whose device is gone, and parking a route at a hole.
# Everything else moves a route by replacing the slot it owns, so a nonzero count
# here is the signal that something is being torn down instead of moved.
# Routing operations that touched one metric slot (the live slot is 10): the
# point of several tests is that a failure before the commit must not disturb the
# exit that is currently carrying traffic, while warming a standby route at a
# worse metric is allowed.
route_ops_at() {
	if [ -f "$SC/route-actions.log" ]; then
		grep -c "metric $1$" "$SC/route-actions.log"
	else
		echo 0
	fi
}

route_dels() {
	if [ -f "$SC/route-actions.log" ]; then
		grep -c '^ip -[46] route del ' "$SC/route-actions.log"
	else
		echo 0
	fi
}

active_dev() { # <4|6>: the device the live table would use
	local f="$SC/routes/default$1"
	[ -f "$f" ] || return 1
	awk '
		function metric_of(line,   i, n, a, m, fam) {
			m = (fam == "6") ? 1024 : 0
			n = split(line, a, /[ \t]+/)
			for (i = 1; i <= n; i++)
				if (a[i] == "metric") m = a[i + 1] + 0
			return m
		}
		{ m = metric_of($0); if (!found || m < best) { found = 1; best = m; line = $0 } }
		END {
			if (found) {
				n = split(line, a, /[ \t]+/)
				for (i = 1; i < n; i++)
					if (a[i] == "dev") { print a[i + 1]; exit }
			}
		}
	' "$f"
}

active_metric() { # <4|6>
	local f="$SC/routes/default$1"
	[ -f "$f" ] || return 1
	awk -v fam="$1" '
		function metric_of(line,   i, n, a, m) {
			m = (fam == "6") ? 1024 : 0
			n = split(line, a, /[ \t]+/)
			for (i = 1; i <= n; i++)
				if (a[i] == "metric") m = a[i + 1] + 0
			return m
		}
		{ m = metric_of($0); if (!found || m < best) { found = 1; best = m } }
		END { if (found) print best }
	' "$f"
}

# The oracle the previous release used: the first default route in the main
# table, attributed by device name.  Kept here because several tests assert what
# it got wrong - it is the regression, not the reference.
old_owner() { # <dev> <wan devices> <modem devices>
	case " $2 " in
		*" $1 "*) echo wan; return 0 ;;
	esac
	case " $3 " in
		*" $1 "*) echo modem; return 0 ;;
	esac
	if [ -n "$1" ]; then echo other; else echo none; fi
}

# Does any route line in <file> egress through <dev>?  Used to reproduce what the
# previous device resolution saw.
route_on_dev() { # <routes text> <dev>
	printf '%s\n' "$1" | awk -v dev="$2" '
		{
			for (i = 1; i < NF; i++)
				if ($i == "dev" && $(i + 1) == dev)
					found = 1
		}
		END { print found ? 1 : 0 }
	'
}

action_index() { # 1-based line number of an exact action, empty when absent
	[ -f "$SC/iface-actions.log" ] || return 1
	awk -v pat="$1" '$0 == pat { print NR; exit }' "$SC/iface-actions.log"
}

action_count() {
	if [ -f "$SC/iface-actions.log" ]; then
		grep -c . "$SC/iface-actions.log"
	else
		echo 0
	fi
}

action_has() {
	[ -f "$SC/iface-actions.log" ] && grep -q -e "$1" "$SC/iface-actions.log"
}

log_has() {
	[ -f "$SC/logger.log" ] && grep -q -- "$1" "$SC/logger.log"
}

# A switch worker runs in the background by design, so a test that starts one has
# to wait for it to reach a terminal state before it can look at the result.  The
# wait is bounded and reported, so a wedged worker fails the test instead of
# hanging the suite.
wait_switch_done() { # [max seconds]
	local limit="${1:-10}" waited=0 state=""
	while [ "$waited" -lt "$limit" ]; do
		state="$(state_of state)"
		case "$state" in
			COMMITTED|FAILED|IDLE) printf '%s\n' "$state"; return 0 ;;
		esac
		sleep 1
		waited=$((waited + 1))
	done
	printf '%s\n' "${state:-timeout}"
	return 1
}

# A deterministic clock.  /proc/uptime cannot be controlled from a test, and the
# centisecond clock has an arithmetic edge case (a fraction starting with a zero)
# that must be pinned rather than left to chance.
set_clock() { # <seconds with two decimals, e.g. 1234.08>
	printf '%s 42.00\n' "$1" > "$SC/uptime"
	H5_UPTIME="$SC/uptime"
}

clear_clock() {
	H5_UPTIME=""
}

# ---------------------------------------------------------------------------
# baseline scenario mirroring the production H5000M
# ---------------------------------------------------------------------------

WAN_ROUTE='default via 192.168.88.1 dev eth1 proto static src 192.168.88.187 metric 10'
MODEM_ROUTE='default via 10.13.35.1 dev eth2 proto static metric 50'

base_scenario() {
	new_scenario

	uci_set network.loopback interface
	uci_set network.lan interface
	uci_set network.wan interface
	uci_set network.wan6 interface
	uci_set network.2_1 interface
	uci_set network.2_1v6 interface

	uci_set network.lan.device br-lan
	uci_set network.wan.device eth1
	uci_set network.wan.proto dhcp
	uci_set network.wan6.device eth1
	uci_set network.2_1.device eth2
	uci_set network.2_1.modem_config 2_1
	uci_set network.2_1v6.device '@2_1'
	uci_set network.2_1v6.modem_config 2_1
	uci_set network.2_1v6.defaultroute 0
	uci_set network.2_1v6.auto 1
	uci_set network.wan6.defaultroute 1
	uci_set network.wan6.auto 1

	uci_set h5000m_netmode.settings settings
	uci_set h5000m_netmode.settings.mode wan_first
	uci_set h5000m_netmode.settings.ipv6_owner wan
	# Health probing is opt-out, so every other case pins it off: their behaviour
	# must not depend on a probe verdict, and the default itself is covered by
	# test_health_probe_defaults_on.
	uci_set h5000m_netmode.settings.health_check 0
	# The switch waits are real, but a test must not sit through the production
	# timeout to observe a timeout, and the settle window only delays the
	# assertion.  The production defaults are asserted in
	# test_switch_limits_are_bounded.
	uci_set h5000m_netmode.settings.switch_wait_ipv4 1
	uci_set h5000m_netmode.settings.switch_wait_ipv6 1
	uci_set h5000m_netmode.settings.switch_settle 0
	uci_set h5000m_netmode.settings.switch_settle_warm 0
	uci_set h5000m_netmode.settings.switch_budget 30
	uci_set h5000m_netmode.settings.probe_timeout 1

	netdev eth0 0
	netdev eth1 1
	netdev eth2 1
	netdev br-lan 1

	addr eth1 4 192.168.88.187/24
	addr eth1 6 2409:8800::187/64
	addr eth2 4 10.13.35.40/24
	addr eth2 6 2409:8a00::40/64

	ubus_set wan 'up=true' 'available=true' 'pending=false' 'l3_device="eth1"' 'device="eth1"'
	ubus_set wan6 'up=true' 'available=true' 'pending=false' 'l3_device="eth1"' 'device="eth1"'
	ubus_set 2_1 'up=true' 'available=true' 'pending=false' 'l3_device="eth2"' 'device="eth2"'
	ubus_set 2_1v6 'up=false' 'available=true' 'pending=false' 'device="eth2"'
}

# Both exits complete and reachable: the state a switch starts from.
both_exits_up() {
	routes "$WAN_ROUTE
$MODEM_ROUTE" \
		'default via fe80::1 dev eth1 metric 10
default via fe80::2 dev eth2 metric 50' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::1 dev eth1 src 2409:8800::187 uid 0'
}

# ---------------------------------------------------------------------------
# exit detection
# ---------------------------------------------------------------------------

# The live exit must come from the kernel FIB.  A policy rule can send traffic
# through a table the main table knows nothing about, which is how a manager ends
# up reporting the standby as active (or never switching at all).
test_fib_oracle_beats_main_table() {
	base_scenario
	routes "$WAN_ROUTE
$MODEM_ROUTE" '' \
		'1.1.1.1 via 10.13.35.1 dev eth2 table 1000 src 10.13.35.40 uid 0' ''

	run status
	check_eq 'fib-oracle rc' '0' "$rc"
	check_eq 'fib-oracle egress4' 'eth2' "$(sv egress4)"
	check_eq 'fib-oracle active4' 'modem' "$(sv active4)"

	# The previous oracle reads the main table only and reports the wired WAN
	# while the kernel is really using the 5G modem.
	old="$(printf '%s\n' "$WAN_ROUTE" | awk '{ for (i=1;i<NF;i++) if ($i=="dev") { print $(i+1); exit } }')"
	check_eq 'fib-oracle old oracle disagrees' 'wan' "$(old_owner "$old" 'eth1' 'eth2')"
}

# Dump order is not a contract.  The old code took the first line; the fix takes
# the lowest metric, which is what the kernel actually selects.
test_metric_order_is_not_dump_order() {
	base_scenario
	routes "$MODEM_ROUTE
$WAN_ROUTE" '' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' ''

	run status
	check_eq 'metric-order rc' '0' "$rc"
	check_eq 'metric-order egress4 from get' 'eth1' "$(sv egress4)"
	check_eq 'metric-order default4 is the lowest metric' '1' \
		"$(printf '%s' "$(sv default4)" | grep -c 'metric 10')"

	# Neither the new nor the old code may pick the modem just because its line is
	# printed first: the kernel prefers the lower metric, and so must the report.
	check_eq 'metric-order active4' 'wan' "$(sv active4)"
}

# netifd hands back a symbolic reference (`@2_1`) for an interface that shares a
# device with another section.  Comparing that string against a route's `dev`
# never matches, which is how the modem IPv6 route became invisible and the exit
# looked permanently split.
test_symbolic_device_reference_is_resolved() {
	base_scenario
	ubus_del 2_1v6
	routes "$WAN_ROUTE
$MODEM_ROUTE" \
		'default via fe80::942b:33ff:fecd:8306 dev eth2 metric 50' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::942b:33ff:fecd:8306 dev eth2 src 2409::1 uid 0'

	run status
	check_eq 'symbolic-dev modem_device' 'eth2' "$(sv modem_device)"
	check_eq 'symbolic-dev modem_devices' 'eth2' "$(sv modem_devices)"
	check_eq 'symbolic-dev modem6_ready' '1' "$(sv modem6_ready)"
	check_eq 'symbolic-dev active6' 'modem' "$(sv active6)"

	# The previous resolution fell back to the raw UCI reference, and "@2_1" never
	# matches a route's dev - so readiness was reported as 0 while the modem IPv6
	# route was right there on eth2.
	check_eq 'symbolic-dev old value matches nothing' '0' \
		"$(route_on_dev 'default via fe80::1 dev eth2 metric 50' '@2_1')"
	check_eq 'symbolic-dev new value matches the route' '1' \
		"$(route_on_dev 'default via fe80::1 dev eth2 metric 50' 'eth2')"
}

# A transparent proxy puts the IPv6 default route on a TUN (singtun0) while the
# IPv4 default route stays on the physical uplink.  This app only manages the
# physical WAN/modem exits, so the virtual TUN inherits the IPv4 physical owner
# from the same state snapshot instead of becoming a fake third exit.
#
# The original code compared the two interface NAMES, so `eth2` vs `singtun0`
# never matched and every proxy user saw a permanent "exit split" warning with a
# button telling them to "align" an exit that was already aligned.
test_proxy_tunnel_is_not_a_family_split() {
	base_scenario
	tunnel singtun0
	routes "$MODEM_ROUTE" \
		'default via fe80::c08d:2eff:fec4:3402 dev singtun0 metric 512' \
		'1.1.1.1 via 10.13.35.1 dev eth2 src 10.13.35.40 uid 0' \
		'2606:4700:4700::1111 via fe80::c08d:2eff:fec4:3402 dev singtun0 src 2409::1 uid 0'

	run status
	check_eq 'tunnel rc' '0' "$rc"
	check_eq 'tunnel egress4' 'eth2' "$(sv egress4)"
	check_eq 'tunnel egress6' 'singtun0' "$(sv egress6)"
	check_eq 'tunnel active4' 'modem' "$(sv active4)"
	check_eq 'tunnel active6 follows physical IPv4' 'modem' "$(sv active6)"
	check_eq 'tunnel is not reported as a split' '0' "$(sv split)"
	check_eq 'tunnel keeps daed state inert' 'none' "$(sv daed_exit_state)"
}

# The same proxy bound to the wired WAN must be attributed to the WAN.
test_proxy_tunnel_attributes_to_the_proxy_exit() {
	base_scenario
	tunnel singtun0
	routes "$WAN_ROUTE" \
		'default via fe80::1 dev singtun0 metric 512' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::1 dev singtun0 src 2409::1 uid 0'

	run status
	check_eq 'tunnel-wan active4' 'wan' "$(sv active4)"
	check_eq 'tunnel-wan active6 follows physical IPv4' 'wan' "$(sv active6)"
	check_eq 'tunnel-wan is not a split' '0' "$(sv split)"
}

# A real split - one family on each uplink - must still be reported, even when a
# proxy tunnel is in the path for one of the families.
test_real_family_split_is_still_reported() {
	base_scenario
	routes "$WAN_ROUTE" \
		"$MODEM_ROUTE" \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::1 dev eth2 src 2409::1 uid 0'

	run status
	check_eq 'real-split active4' 'wan' "$(sv active4)"
	check_eq 'real-split active6' 'modem' "$(sv active6)"
	check_eq 'real-split is reported' '1' "$(sv split)"
	check_eq 'real-split names both families' 'ipv4=wan ipv6=modem' "$(sv split_detail)"
}

# HomeProxy/sing-box do not need a daed-style exit record: the virtual TUN is not
# managed, and its physical owner follows IPv4.
test_tunnel_without_proxy_state_follows_ipv4_physical_exit() {
	base_scenario
	tunnel singtun0
	routes "$MODEM_ROUTE" \
		'default via fe80::1 dev singtun0 metric 512' \
		'1.1.1.1 via 10.13.35.1 dev eth2 src 10.13.35.40 uid 0' \
		'2606:4700:4700::1111 via fe80::1 dev singtun0 src 2409::1 uid 0'

	run status
	check_eq 'homeproxy-tunnel active4' 'modem' "$(sv active4)"
	check_eq 'homeproxy-tunnel active6 follows physical IPv4' 'modem' "$(sv active6)"
	check_eq 'homeproxy-tunnel split' '0' "$(sv split)"
}

# ---------------------------------------------------------------------------
# the same-exit invariant
# ---------------------------------------------------------------------------

# IPv4 on the wired WAN, IPv6 leaked onto the modem: the invariant is restored by
# moving BOTH families onto one group - never by deleting a family's route.  The
# old repair deleted the modem's IPv6 default route, which is exactly the
# "IPv6 follows IPv4 by taking IPv6 away" pattern this release removes.
test_split_egress_is_repaired_without_dropping_a_family() {
	base_scenario
	routes "$WAN_ROUTE
$MODEM_ROUTE" \
		'default via fe80::1 dev eth1 metric 50
default via fe80::2 dev eth2 metric 10' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::2 dev eth2 src 2409:8a00::40 uid 0'
	ubus_set 2_1v6 'up=true' 'available=true' 'pending=false' 'l3_device="eth2"' 'device="eth2"'

	run status
	check_eq 'split-before split' '1' "$(sv split)"
	check_eq 'split-before active4' 'wan' "$(sv active4)"
	check_eq 'split-before active6' 'modem' "$(sv active6)"

	run reconcile
	check_eq 'split-rc' '0' "$rc"

	# Both families now leave through the same group - the policy primary, which
	# is the complete exit here - and it is the group they both use from now on.
	check_eq 'split ipv4 slot stays on the primary' 'eth1' "$(active_dev 4)"
	check_eq 'split ipv6 slot moved to the primary' 'eth1' "$(active_dev 6)"
	check_eq 'split ipv4 metric' '10' "$(active_metric 4)"
	check_eq 'split ipv6 metric' '10' "$(active_metric 6)"
	check_eq 'split owner recorded' 'wan' "$(uci_of h5000m_netmode.settings.ipv6_owner)"

	# The standby keeps a warm default route at the standby metric ...
	check_contains 'split keeps a warm modem route' 'default via 10.13.35.1 dev eth2 metric 50' \
		"$SC/routes/default4"
	check_contains 'split parks the modem IPv6 route as the standby' \
		'default via fe80::2 dev eth2 metric 50' "$SC/routes/default6"
	# ... and no family was taken away to make the invariant hold.
	check_absent 'split deletes no IPv6 route' 'route del' "$SC/route-actions.log"
	check_absent 'split deletes no route at all' '^ip -6 route del' "$SC/route-actions.log"
	check_absent 'split keeps the modem IPv6 interface up' '^ifdown' "$SC/iface-actions.log"
	check_eq 'split leaves IPv6 managed' '1' "$(uci_of network.wan6.auto)"
	check_eq 'split keeps wan6 able to install a route' '1' "$(uci_of network.wan6.defaultroute)"
}

# The mirror image: the modem is live for IPv4, the wired IPv6 route is the one
# that is left over, and the modem is the complete exit.
test_failover_aligns_both_families_without_shutting_anything_down() {
	base_scenario
	uci_set h5000m_netmode.settings.mode modem_first
	routes 'default via 10.13.35.1 dev eth2 metric 10
default via 192.168.88.1 dev eth1 metric 50' \
		'default via fe80::2 dev eth2 metric 50
default via fe80::1 dev eth1 metric 10' \
		'1.1.1.1 via 10.13.35.1 dev eth2 src 10.13.35.40 uid 0' \
		'2606:4700:4700::1111 via fe80::1 dev eth1 src 2409:8800::187 uid 0'
	ubus_set wan6 'up=true' 'available=true' 'pending=false' 'l3_device="eth1"' 'device="eth1"'

	run status
	check_eq 'failover-before split' '1' "$(sv split)"
	check_eq 'failover-before active4' 'modem' "$(sv active4)"
	check_eq 'failover-before active6' 'wan' "$(sv active6)"

	run reconcile
	check_eq 'failover rc' '0' "$rc"
	check_eq 'failover ipv4 stays on the modem' 'eth2' "$(active_dev 4)"
	check_eq 'failover ipv6 joins the modem' 'eth2' "$(active_dev 6)"
	check_eq 'failover owner recorded' 'modem' "$(uci_of h5000m_netmode.settings.ipv6_owner)"
	check_contains 'failover parks the wired IPv6 route as the standby' \
		'default via fe80::1 dev eth1 metric 50' "$SC/routes/default6"
	check_absent 'failover never deletes an IPv6 route' '^ip -6 route del' "$SC/route-actions.log"
	check_absent 'failover never calls ifdown' '^ifdown' "$SC/iface-actions.log"
	check_eq 'failover keeps wan6 enabled' '1' "$(uci_of network.wan6.auto)"
	check_eq 'failover keeps modem IPv6 enabled' '1' "$(uci_of network.2_1v6.auto)"
}

# With no IPv4 default there is nothing to stay consistent with, so IPv6 must be
# left alone instead of being torn down.
test_no_ipv4_default_keeps_ipv6_untouched() {
	base_scenario
	routes '' 'default via fe80::942b:33ff:fecd:8306 dev eth1 metric 512' '' \
		'2606:4700:4700::1111 via fe80::942b:33ff:fecd:8306 dev eth1 src 2409:8800::187 uid 0'

	run reconcile
	check_eq 'no-v4 rc' '0' "$rc"
	check_eq 'no-v4 wan6.defaultroute untouched' '1' "$(uci_of network.wan6.defaultroute)"
	check_eq 'no-v4 wan6.auto untouched' '1' "$(uci_of network.wan6.auto)"
	check_eq 'no-v4 no iface churn' '0' "$(action_count)"
	check_eq 'no-v4 no network commit' '0' "$(writes_count 'commit network')"
	check_eq 'no-v4 no route surgery' '0' "$(route_ops)"

	run status
	check_eq 'no-v4 active4' 'none' "$(sv active4)"
	check_eq 'no-v4 ipv6 owner is not called a split' 'wan' "$(sv ipv6_owner)"
	check_eq 'no-v4 split flag' '0' "$(sv split)"
}

# A target without an IPv6 interface cannot be a complete dual-stack exit.  It is
# refused with a reason instead of being activated and having IPv6 switched off,
# which is what the previous release did here.
test_dual_stack_target_is_required() {
	base_scenario
	uci_del network.2_1v6
	uci_del network.2_1v6.device
	uci_del network.2_1v6.modem_config
	uci_del network.2_1v6.defaultroute
	uci_del network.2_1v6.auto
	ubus_del 2_1v6
	both_exits_up

	run status
	check_eq 'no-modem6 capable' '0' "$(sv ipv6_capable_modem)"
	check_eq 'no-modem6 is not a complete exit' '0' "$(sv group_ready_modem)"
	check_eq 'no-modem6 owner is never off' 'wan' "$(sv ipv6_owner)"

	run switch-worker modem_first 1
	check_eq 'no-modem6 switch rc' '1' "$rc"
	check_eq 'no-modem6 switch state' 'FAILED' "$(state_of state)"
	check_eq 'no-modem6 failure reason' 'ipv6_not_configured' "$(state_of reason)"

	# IPv4 and IPv6 both stay on the wired WAN: no half-dual-stack, no IPv6 route
	# taken away, no interface touched.
	check_eq 'no-modem6 ipv4 unchanged' 'eth1' "$(active_dev 4)"
	check_eq 'no-modem6 ipv6 unchanged' 'eth1' "$(active_dev 6)"
	check_eq 'no-modem6 no surgery on the live slot' '0' "$(route_ops_at 10)"
	check_eq 'no-modem6 no iface churn' '0' "$(action_count)"
	check_eq 'no-modem6 wan6 stays enabled' '1' "$(uci_of network.wan6.auto)"
	check_eq 'no-modem6 wan6 keeps its route' '1' "$(uci_of network.wan6.defaultroute)"
}

# A consistent state must produce no writes and no interface churn, and a second
# run must be equally quiet.
test_stable_state_produces_no_churn() {
	base_scenario
	both_exits_up

	run reconcile
	check_eq 'stable rc' '0' "$rc"
	check_eq 'stable no iface actions' '0' "$(action_count)"
	check_eq 'stable no route surgery' '0' "$(route_ops)"
	check_eq 'stable no network commit' '0' "$(writes_count 'commit network')"
	check_eq 'stable ipv6_owner unchanged' 'wan' "$(uci_of h5000m_netmode.settings.ipv6_owner)"

	run status
	check_eq 'stable reports one exit' 'wan' "$(sv group_active)"
	check_eq 'stable reports no split' '0' "$(sv split)"

	run reconcile
	check_eq 'stable is idempotent' '0' "$(action_count)"
	check_eq 'stable is idempotent for routes' '0' "$(route_ops)"
}

# `status` is polled by LuCI every five seconds: it must be read-only and must keep
# every key the shipped frontend consumes.
test_status_is_read_only_and_complete() {
	base_scenario
	both_exits_up

	run status
	check_eq 'status rc' '0' "$rc"
	check_eq 'status writes nothing' '0' "$(writes_total)"
	check_eq 'status touches no route' '0' "$(route_ops)"
	check_eq 'status touches no interface' '0' "$(action_count)"

	for key in mode ipv6_policy ipv6_owner ipv6_desired ipv6_capable_wan ipv6_capable_modem \
		wan_present wan_available wan_pending wan_carrier wan_up wan6_up wan4_ready \
		wan6_ready wan_device wan_devices wan_device_source \
		modem_present modem_available modem_pending modem_carrier modem_up modem6_up \
		modem4_ready modem6_ready modem_device modem_devices modem_device_source \
		modem_interface modem6_interface egress4 egress6 active4 active6 split \
		default4 default6 daed_exit_state watcher watch_interval health_check \
		wan_health modem_health \
		wifi_total wifi_up wifi_ssid wifi_clients \
		eth_fallback wan_metric wan6_metric usb_metric usbv6_metric wan_defaultroute \
		wan6_defaultroute wan6_auto usb_defaultroute usbv6_defaultroute usbv6_auto \
		modem_metric; do
		check_eq "status key $key" '1' "$(grep -c "^$key=" "$SC/out.txt")"
	done
}

# The Wi-Fi tile is the only place a radio that failed to come up is visible, so
# the status call has to carry the real count.  Two radios, one up, two stations:
# anything that collapses the wildcard path to a single value reports 1/1 and
# hides the dead radio.
test_wireless_state_is_reported() {
	base_scenario
	routes "$WAN_ROUTE" '' '1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' ''

	wifi_set true false OWRT
	wifi_assoc phy0-ap0 'aa:bb:cc:dd:ee:01' 'aa:bb:cc:dd:ee:02'
	wifi_assoc phy1-ap0

	run status
	check_eq 'wireless rc' '0' "$rc"
	check_eq 'wireless radios total' '2' "$(sv wifi_total)"
	check_eq 'wireless radios up' '1' "$(sv wifi_up)"
	check_eq 'wireless ssid' 'OWRT' "$(sv wifi_ssid)"
	check_eq 'wireless clients' '2' "$(sv wifi_clients)"

	# Both radios up, three stations across two BSS: the count must be the sum.
	wifi_set true true OWRT
	wifi_assoc phy0-ap0 'aa:bb:cc:dd:ee:01' 'aa:bb:cc:dd:ee:02'
	wifi_assoc phy1-ap0 'aa:bb:cc:dd:ee:03'

	run status
	check_eq 'wireless all radios up' '2' "$(sv wifi_up)"
	check_eq 'wireless clients summed' '3' "$(sv wifi_clients)"

	# No wireless module at all: status must still succeed and report zeros.
	wifi_del
	run status
	check_eq 'wireless absent rc' '0' "$rc"
	check_eq 'wireless absent total' '0' "$(sv wifi_total)"
	check_eq 'wireless absent up' '0' "$(sv wifi_up)"
	check_eq 'wireless absent clients' '0' "$(sv wifi_clients)"
}

# Health probing is opt-out: the diagnostic exists for the case where an uplink is
# proto-up but nothing passes, which is exactly the case a default install would
# otherwise never notice.  Three behaviours have to hold - absent means on, an
# explicit off value means off, and the cached verdict is not re-probed on every
# reconcile (the watchdog runs every few seconds and must not spend traffic on it).
test_health_probe_defaults_on() {
	base_scenario
	uci_del h5000m_netmode.settings.health_check
	both_exits_up

	run status
	check_eq 'health absent means on' '1' "$(sv health_check)"

	for value in 0 off false no; do
		uci_set h5000m_netmode.settings.health_check "$value"
		run status
		check_eq "health off via $value" '0' "$(sv health_check)"
	done

	# Back to the default, and let a reconcile produce a verdict.
	uci_del h5000m_netmode.settings.health_check
	run reconcile
	check_eq 'health caches wan verdict' '1' "$(health_of wan)"
	check_eq 'health caches modem verdict' '1' "$(health_of modem)"
	check_eq 'health caches the IPv6 verdicts' '11' "$(health_of wan6)$(health_of modem6)"
	check_eq 'health records a timestamp' '1' "$(grep -c '^ts=' "$SC/health")"

	# A verdict younger than health_probe_interval must be reused even though a
	# fresh probe would contradict it (the ping mock always succeeds).
	printf 'wan=0\nmodem=0\nwan6=0\nmodem6=0\nts=%s\n' "$(date +%s)" > "$SC/health"
	run reconcile
	check_eq 'health throttles a fresh verdict' '0' "$(health_of wan)"

	# interval=0 disables the throttle and the verdict is refreshed again.
	uci_set h5000m_netmode.settings.health_probe_interval 0
	run reconcile
	check_eq 'health interval 0 refreshes' '1' "$(health_of wan)"

	# A stale verdict is refreshed without touching the interval.
	printf 'wan=0\nmodem=0\nwan6=0\nmodem6=0\nts=1\n' > "$SC/health"
	uci_del h5000m_netmode.settings.health_probe_interval
	run reconcile
	check_eq 'health refreshes a stale verdict' '1' "$(health_of wan)"
}

# hotplug delegates classification to the backend so a modem section without the
# qmodem marker still triggers a reconcile.
test_iface_role_classification() {
	base_scenario
	uci_set network.3_1 interface
	uci_set network.3_1.device eth3

	local pair section want
	for pair in 'wan:wan' 'wan6:wan' 'lan:other' 'loopback:other' '2_1:modem' \
		'2_1v6:modem' '3_1:modem' 'nonesuch:other'; do
		section="${pair%%:*}"
		want="${pair#*:}"
		run iface-role "$section"
		check_eq "iface-role $section" "$want" "$(sed -n 1p "$SC/out.txt")"
	done

	run iface-role 'bad;name'
	check_eq 'iface-role rejects junk' '64' "$rc"
}

test_eth_fallback_sections_are_not_modem() {
	base_scenario
	uci_set h5000m_netmode.settings.eth_fallback 3_1
	uci_set network.3_1 interface
	uci_set network.3_1.device eth3

	run iface-role 3_1
	check_eq 'eth-fallback section excluded' 'other' "$(sed -n 1p "$SC/out.txt")"

	run iface-role 2_1
	check_eq 'eth-fallback still finds the modem' 'modem' "$(sed -n 1p "$SC/out.txt")"
}

# UCI rewrites the whole config file on every `uci set`, so a mapping saved while
# another run is in flight used to be silently overwritten.  Every mutating action
# now takes the lock, and a dead lock holder is recovered from.
test_manual_mapping_is_serialised() {
	base_scenario
	routes "$WAN_ROUTE
$MODEM_ROUTE" '' '1.1.1.1 via 192.168.88.1 dev eth9 src 192.168.88.187 uid 0' ''

	mkdir -p "$SC/lock"
	printf '%s' "$$" > "$SC/lock/pid"
	run set-device-map wan eth9
	check_eq 'manual-map concurrent rc' '2' "$rc"
	check_eq 'manual-map concurrent wrote nothing' '' "$(uci_of h5000m_netmode.settings.wan_device)"

	printf '%s' '999999' > "$SC/lock/pid"
	run set-device-map wan eth9
	check_eq 'manual-map stale-lock rc' '0' "$rc"
	check_eq 'manual-map stale-lock recovered' 'eth9' \
		"$(uci_of h5000m_netmode.settings.wan_device)"

	run status
	check_eq 'manual-map source' 'manual' "$(sv wan_device_source)"
	check_eq 'manual-map active4' 'wan' "$(sv active4)"
}

test_shared_device_is_reported() {
	base_scenario
	uci_set h5000m_netmode.settings.modem_device eth1
	routes "$WAN_ROUTE" '' '1.1.1.1 via 192.168.88.1 dev eth1 uid 0' ''

	run status
	check_eq 'shared device mapping is manual' 'manual' "$(sv modem_device_source)"
	check_eq 'shared device keeps the wired uplink' 'eth1' "$(sv wan_devices)"
	check_eq 'shared device is taken away from the modem' '' "$(sv modem_devices)"

	run reconcile
	check_contains 'shared device warning' 'mapped to both exits' "$SC/logger.log"
	check_eq 'shared device warning appears once' '1' "$(grep -c 'mapped to both exits' "$SC/logger.log")"
	check_eq 'shared device starts no switch' '0' "$(route_ops)"
	check_eq 'shared device writes nothing' '0' "$(writes_total)"
}

test_usage_errors() {
	base_scenario

	run bogus
	check_eq 'usage unknown action' '64' "$rc"

	run set nonsense
	check_eq 'usage invalid mode' '64' "$rc"

	run set-device-map wan
	check_eq 'usage set-device-map missing arg' '64' "$rc"

	run set-device-map wanx eth1
	check_eq 'usage set-device-map bad role' '64' "$rc"

	run eth-fallback bogus
	check_eq 'usage eth-fallback bad subcommand' '64' "$rc"
}

# hotplug and the watchdog can fire while an apply is still running.  Dropping the
# event leaves the state stale until the next one, so reconcile waits for the lock
# instead of failing immediately - and still reports failure if the holder is alive.
test_reconcile_lock_behaviour() {
	base_scenario
	routes "$WAN_ROUTE
$MODEM_ROUTE" \
		'default via fe80::1 dev eth2 metric 50' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::1 dev eth2 src 2409::1 uid 0'

	# A dead lock holder is recovered from, not waited on.
	mkdir -p "$SC/lock"
	printf '%s' '999999' > "$SC/lock/pid"
	run reconcile
	check_eq 'reconcile recovers a stale lock' '0' "$rc"
	check_eq 'reconcile cleared the stale lock' '' "$(uci_of h5000m_netmode.settings.wan_device)"

	# A live holder makes reconcile give up after the bounded wait.
	mkdir -p "$SC/lock"
	printf '%s' "$$" > "$SC/lock/pid"
	run reconcile
	check_eq 'reconcile gives up on a live holder' '2' "$rc"
	check_eq 'reconcile left the live lock alone' "$$" "$(read_file "$SC/lock/pid")"
	rm -rf "$SC/lock"

	# A switch in flight owns the network state: reconcile must not touch it, and
	# must not sit on the lock waiting either.
	base_scenario
	printf 'state=WAIT_IPV6\npid=%s\n' "$$" > "$SC/state"
	run reconcile
	check_eq 'reconcile defers to a running switch' '0' "$rc"
	check_eq 'reconcile left the switch state alone' 'WAIT_IPV6' "$(state_of state)"
	check_eq 'reconcile did no route surgery' '0' "$(route_ops)"
}

# ---------------------------------------------------------------------------
# switching
# ---------------------------------------------------------------------------

# Scenario A: both exits are complete, the user switches to 5G.  Both families
# move together, the previous exit keeps its addresses and a warm default route,
# and nothing is torn down.
test_switch_moves_both_families_to_the_target() {
	base_scenario
	both_exits_up

	run switch-worker modem_first 1
	check_eq 'switch-5g rc' '0' "$rc"
	check_eq 'switch-5g state' 'COMMITTED' "$(state_of state)"
	check_eq 'switch-5g result' 'ok' "$(state_of result)"
	check_eq 'switch-5g applied mode' 'modem_first' "$(state_of applied_mode)"
	check_eq 'switch-5g ipv4 moved' 'eth2' "$(active_dev 4)"
	check_eq 'switch-5g ipv6 moved' 'eth2' "$(active_dev 6)"
	check_eq 'switch-5g ipv4 slot' '10' "$(active_metric 4)"
	check_eq 'switch-5g ipv6 slot' '10' "$(active_metric 6)"

	# The old exit is demoted, never taken down: its route is still there at the
	# standby metric, so a failure of the new exit fails over instantly.
	check_contains 'switch-5g keeps the wired WAN warm' \
		'default via 192.168.88.1 dev eth1 metric 50' "$SC/routes/default4"
	check_contains 'switch-5g keeps the wired IPv6 warm' \
		'default via fe80::1 dev eth1 metric 50' "$SC/routes/default6"
	check_eq 'switch-5g touches no interface' '0' "$(action_count)"
	check_eq 'switch-5g deletes no route' '0' "$(route_dels)"

	# The plan is persisted, so a reboot reproduces the same topology and netifd
	# re-announces exactly the state the kernel is already in.
	check_eq 'switch-5g persists the modem metric' '1' "$(writes_count 'set network.2_1.metric=10')"
	check_eq 'switch-5g persists the WAN metric' '1' "$(writes_count 'set network.wan.metric=50')"
	check_eq 'switch-5g persists the mode' '1' "$(writes_count 'set h5000m_netmode.settings.mode=modem_first')"
	check_absent 'switch-5g never switches IPv6 off' 'auto=0' "$SC/uci-writes.log"

	# The log has to answer "what happened" without a packet capture.
	check_contains 'switch-5g logs the direction' 'switch wan -> modem' "$SC/logger.log"
	check_contains 'switch-5g logs IPv4 readiness' 'IPv4 ready' "$SC/logger.log"
	check_contains 'switch-5g logs IPv6 connectivity' 'IPv6 connectivity OK' "$SC/logger.log"
	check_contains 'switch-5g logs the commit' 'switch committed' "$SC/logger.log"
}

# Scenario B: the same in the other direction.
test_switch_back_to_the_wired_wan() {
	base_scenario
	both_exits_up

	run switch-worker modem_first 1
	run switch-worker wan_first 2
	check_eq 'switch-back rc' '0' "$rc"
	check_eq 'switch-back committed' 'COMMITTED' "$(state_of state)"
	check_eq 'switch-back applied mode' 'wan_first' "$(state_of applied_mode)"
	check_eq 'switch-back ipv4' 'eth1' "$(active_dev 4)"
	check_eq 'switch-back ipv6' 'eth1' "$(active_dev 6)"
	check_eq 'switch-back ipv4 slot' '10' "$(active_metric 4)"
	check_eq 'switch-back ipv6 slot' '10' "$(active_metric 6)"
	check_contains 'switch-back keeps 5G warm' \
		'default via 10.13.35.1 dev eth2 metric 50' "$SC/routes/default4"
	check_contains 'switch-back keeps 5G IPv6 warm' \
		'default via fe80::2 dev eth2 metric 50' "$SC/routes/default6"
	check_eq 'switch-back touches no interface' '0' "$(action_count)"
	check_eq 'switch-back deletes nothing' '0' "$(route_dels)"
}

# Scenario C: the target's IPv4 works but its IPv6 does not pass traffic.  The
# switch must not be committed - a half dual-stack exit is worse than staying
# where we are - and the old exit must be untouched.
test_broken_target_ipv6_keeps_the_old_exit() {
	base_scenario
	both_exits_up
	ping_fail 'dev=eth2 fam=6'

	run switch-worker modem_first 1
	check_eq 'broken-v6 rc' '1' "$rc"
	check_eq 'broken-v6 state' 'FAILED' "$(state_of state)"
	check_eq 'broken-v6 reason' 'ipv6_unreachable' "$(state_of reason)"
	check_eq 'broken-v6 ipv4 stays' 'eth1' "$(active_dev 4)"
	check_eq 'broken-v6 ipv6 stays' 'eth1' "$(active_dev 6)"
	check_eq 'broken-v6 no surgery on the live slot' '0' "$(route_ops_at 10)"
	check_eq 'broken-v6 no interface churn' '0' "$(action_count)"
	check_eq 'broken-v6 mode unchanged' 'wan_first' "$(uci_of h5000m_netmode.settings.mode)"
	check_contains 'broken-v6 logged' 'switch failed: ipv6_unreachable' "$SC/logger.log"
	check_contains 'broken-v6 logged as a rollback' 'rollback' "$SC/logger.log"

	run status
	check_eq 'broken-v6 status state' 'FAILED' "$(sv switch_state)"
	check_eq 'broken-v6 status reason' 'ipv6_unreachable' "$(sv switch_reason)"
	check_eq 'broken-v6 status split' '0' "$(sv split)"
}

# Scenario D: the target has no IPv4 at all yet (the modem is still dialling).
# The switch waits, gives up within its bound and leaves the wired WAN alone.
test_target_without_ipv4_never_touches_the_old_exit() {
	base_scenario
	routes "$WAN_ROUTE" 'default via fe80::1 dev eth1 metric 10' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::1 dev eth1 src 2409:8800::187 uid 0'
	addr_clear eth2
	uci_set h5000m_netmode.settings.switch_wait_ipv4 3

	run switch-worker modem_first 1
	check_eq 'target-not-ready rc' '1' "$rc"
	check_eq 'target-not-ready state' 'FAILED' "$(state_of state)"
	check_eq 'target-not-ready reason' 'ipv4_not_ready' "$(state_of reason)"
	check_eq 'target-not-ready ipv4 stays' 'eth1' "$(active_dev 4)"
	check_eq 'target-not-ready ipv6 stays' 'eth1' "$(active_dev 6)"
	check_eq 'target-not-ready no surgery on the live slot' '0' "$(route_ops_at 10)"

	# The bounded retry is an ifup, and it is the only interface operation: the
	# old exit is never brought down.
	# Exactly two bounded re-announces: the preparation of a target that is not up
	# yet, and one retry while waiting for its address.
	check_eq 'target-not-ready retried, bounded' '2' "$(action_count)"
	check_contains 'target-not-ready asked netifd to retry' '^ifup 2_1$' "$SC/iface-actions.log"
	check_absent 'target-not-ready never ifdowns' '^ifdown' "$SC/iface-actions.log"
}

# Scenario E: the target carries IPv4 only.  Refused: the project requires a
# complete dual-stack exit, and the refusal must not cost the user their IPv6.
test_ipv4_only_target_is_refused() {
	base_scenario
	both_exits_up
	addr_clear eth2
	addr eth2 4 10.13.35.40/24
	routes "$WAN_ROUTE
default via 10.13.35.1 dev eth2 metric 50" \
		'default via fe80::1 dev eth1 metric 10' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::1 dev eth1 src 2409:8800::187 uid 0'

	run switch-worker modem_first 1
	check_eq 'single-stack rc' '1' "$rc"
	check_eq 'single-stack state' 'FAILED' "$(state_of state)"
	check_eq 'single-stack reason' 'ipv6_not_ready' "$(state_of reason)"
	check_eq 'single-stack ipv4 stays' 'eth1' "$(active_dev 4)"
	check_eq 'single-stack ipv6 stays' 'eth1' "$(active_dev 6)"
	check_eq 'single-stack no surgery on the live slot' '0' "$(route_ops_at 10)"
	check_absent 'single-stack removes no IPv6 route' 'route del' "$SC/route-actions.log"
	check_eq 'single-stack wan6 stays enabled' '1' "$(uci_of network.wan6.auto)"
	check_eq 'single-stack wan6 keeps its route' '1' "$(uci_of network.wan6.defaultroute)"
}

# Scenario G: the second half of the commit fails at the kernel level.  The first
# half must be undone, and the user must end up on the exit they started from -
# never on a half-moved state.
test_half_committed_switch_is_rolled_back() {
	base_scenario
	both_exits_up
	route_fail 'dev=eth2 fam=6 verb=replace'

	run switch-worker modem_first 1
	check_eq 'half-commit rc' '1' "$rc"
	check_eq 'half-commit state' 'FAILED' "$(state_of state)"
	check_eq 'half-commit reason' 'ipv6_commit_failed' "$(state_of reason)"

	# Both families are back on the wired WAN, at the metric the plan gives it.
	check_eq 'half-commit rolled back ipv4' 'eth1' "$(active_dev 4)"
	check_eq 'half-commit rolled back ipv6' 'eth1' "$(active_dev 6)"
	check_eq 'half-commit rolled back ipv4 metric' '10' "$(active_metric 4)"
	check_eq 'half-commit rolled back ipv6 metric' '10' "$(active_metric 6)"
	check_eq 'half-commit touched no interface' '0' "$(action_count)"
	check_contains 'half-commit logged the rollback' 'rollback to wan' "$SC/logger.log"

	run status
	check_eq 'half-commit split' '0' "$(sv split)"
	check_eq 'half-commit active owner' 'wan' "$(sv group_active)"
}

# Scenario F: a second click arrives while the first switch is still running.
# It must not start a second worker, and the last request must be the one that
# ends up applied.
test_switch_requests_are_serialised() {
	base_scenario
	both_exits_up
	# Slowed probes keep the first switch in flight while the second click
	# arrives - the same window in which the production defaults verify the
	# target - without making the test depend on any other timing.
	printf '1' > "$SC/ping-delay"
	uci_set h5000m_netmode.settings.switch_settle 2

	run set modem_first
	check_eq 'async set returns immediately' '0' "$rc"
	check_contains 'async set reports a task' 'state=started' "$SC/out.txt"
	check_eq 'async set starts a worker' '1' "$(state_of gen)"

	# Bounded wait for the worker to own the state: a worker that never starts
	# fails this test instead of hanging it.
	i=0
	busy=""
	while [ "$i" -lt 8 ]; do
		run status
		busy="$(sv switch_busy)"
		[ "$busy" = "1" ] && break
		sleep 1
		i=$((i + 1))
	done
	check_eq 'async worker is running' '1' "$busy"

	run set wan_first
	check_eq 'the second click is refused, not queued twice' '3' "$rc"
	check_contains 'the second click reports busy' 'state=busy' "$SC/out.txt"
	check_eq 'the second click bumps the generation' '2' "$(state_of gen)"
	check_eq 'the second click starts no worker' '1' "$(grep -c 'switch wan -> modem' "$SC/logger.log")"

	# The running worker finishes its own switch and then applies the queued
	# request, so the exit the user clicked last is the one that ends up live.
	i=0
	settled=""
	while [ "$i" -lt 60 ]; do
		run status
		if [ "$(sv switch_busy)" = '0' ] && [ "$(state_of applied_mode)" = 'wan_first' ]; then
			settled=1
			break
		fi
		sleep 1
		i=$((i + 1))
	done
	rm -f "$SC/ping-delay"
	check_eq 'the queued request was applied' '1' "${settled:-0}"
	check_eq 'the queued request committed' 'COMMITTED' "$(state_of state)"
	check_eq 'the queued request became the mode' 'wan_first' \
		"$(uci_of h5000m_netmode.settings.mode)"
	check_eq 'both families ended on the wired WAN' 'eth1 eth1' "$(active_dev 4) $(active_dev 6)"
	check_eq 'two switches ran, one worker' '1' "$(grep -c 'switch modem -> wan' "$SC/logger.log")"
	check_eq 'nothing was torn down' '0' "$(action_count)"
}

# A worker killed mid-switch (reboot, OOM, `kill -9`) must not block the next one.
test_dead_worker_does_not_block_the_next_switch() {
	base_scenario
	both_exits_up
	printf 'state=SWITCHING\npid=999999\ntarget=modem\n' > "$SC/state"

	run status
	check_eq 'dead worker is not busy' '0' "$(sv switch_busy)"
	check_eq 'dead worker reads as failed' 'FAILED' "$(sv switch_state)"

	run switch-worker modem_first 1
	check_eq 'next switch proceeds' '0' "$rc"
	check_eq 'next switch committed' 'COMMITTED' "$(state_of state)"
	check_eq 'next switch moved the exit' 'eth2' "$(active_dev 4)"
}

# Scenario H: a modem re-registering fires several hotplug events in a row.  They
# must collapse into one evaluation - the previous version reconciled per event,
# which is how a burst turned into overlapping switch attempts.
test_hotplug_burst_is_coalesced() {
	base_scenario
	both_exits_up
	uci_set h5000m_netmode.settings.hotplug_debounce 3

	run_bg notify wan6 ifup
	pid="$bg_pid"
	sleep 1
	run notify wan ifup
	run notify 2_1 ifup
	wait "$pid"

	check_eq 'every event was counted' '3' "$(state_of hotplug_events)"
	check_eq 'the burst produced one evaluation' '1' "$(state_of hotplug_runs)"
	check_eq 'one debounce log line' '1' "$(grep -c 'hotplug wan6/ifup' "$SC/logger.log")"
	check_eq 'a stable state stays quiet' '0' "$(route_ops)"
	check_eq 'and touched no interface' '0' "$(action_count)"
}

# Scenario I: one failed probe is not a failover.  The verdict and the streak are
# recorded, the exit stays where it is, and a good probe clears the streak instead
# of ratcheting towards a switch.
test_one_failed_probe_does_not_fail_over() {
	base_scenario
	uci_set h5000m_netmode.settings.mode modem_first
	uci_set h5000m_netmode.settings.health_check 1
	uci_set h5000m_netmode.settings.health_probe_interval 0
	uci_set h5000m_netmode.settings.probe_fail_streak 3
	routes 'default via 10.13.35.1 dev eth2 metric 10
default via 192.168.88.1 dev eth1 metric 50' \
		'default via fe80::1 dev eth1 metric 50
default via fe80::2 dev eth2 metric 10' \
		'1.1.1.1 via 10.13.35.1 dev eth2 src 10.13.35.40 uid 0' \
		'2606:4700:4700::1111 via fe80::2 dev eth2 src 2409:8a00::40 uid 0'
	ping_fail 'dev=eth2 fam=6'

	run reconcile
	check_eq 'one bad probe is recorded' '1' "$(health_of modem6_fail)"
	check_eq 'one bad probe does not move IPv6' 'eth2' "$(active_dev 6)"
	check_eq 'one bad probe does no route surgery' '0' "$(route_ops)"
	check_eq 'one bad probe keeps the exit' 'modem' "$(uci_of h5000m_netmode.settings.ipv6_owner)"

	ping_ok
	run reconcile
	check_eq 'a good probe clears the streak' '0' "$(health_of modem6_fail)"
	check_eq 'the exit never moved' 'eth2' "$(active_dev 6)"
	check_eq 'still no route surgery' '0' "$(route_ops)"
}

# Scenario I, continued: the loss is not transient.  Once the streak reaches the
# configured threshold the complete standby takes over - both families, no route
# deleted, nothing torn down.
test_sustained_loss_fails_over_to_the_complete_exit() {
	base_scenario
	uci_set h5000m_netmode.settings.mode modem_first
	uci_set h5000m_netmode.settings.health_check 1
	uci_set h5000m_netmode.settings.health_probe_interval 0
	uci_set h5000m_netmode.settings.probe_fail_streak 2
	uci_set h5000m_netmode.settings.switch_cooldown 0
	routes 'default via 10.13.35.1 dev eth2 metric 10
default via 192.168.88.1 dev eth1 metric 50' \
		'default via fe80::1 dev eth1 metric 50
default via fe80::2 dev eth2 metric 10' \
		'1.1.1.1 via 10.13.35.1 dev eth2 src 10.13.35.40 uid 0' \
		'2606:4700:4700::1111 via fe80::2 dev eth2 src 2409:8a00::40 uid 0'
	ping_fail 'dev=eth2 fam=6'

	run reconcile
	check_eq 'first failing round is not acted on' '1' "$(health_of modem6_fail)"
	check_eq 'no switch after one round' '0' "$(route_ops)"

	run reconcile
	check_ok 'the streak reached the threshold' \
		"$([ "$(health_of modem6_fail)" -ge 2 ] && echo 0 || echo 1)"
	check_eq 'both families now leave through the standby' 'eth1 eth1' \
		"$(active_dev 4) $(active_dev 6)"
	check_eq 'the failing exit is not torn down' '0' "$(action_count)"
	check_eq 'no route is deleted' '0' "$(route_dels)"
	check_contains 'the failover is logged' 'exit_degraded' "$SC/logger.log"
	check_contains 'the degraded exit is named in the log' 'modem degraded' "$SC/logger.log"
	check_eq 'the standby is now the recorded owner' 'wan' \
		"$(uci_of h5000m_netmode.settings.ipv6_owner)"

	run status
	check_eq 'the failover leaves no split' '0' "$(sv split)"
	check_eq 'the standby is complete' '1' "$(sv group_ready_wan)"
}

# The one-click repair endpoint: an IPv4/IPv6 split is a fault, so `align` fixes it
# immediately instead of waiting out the cooldown.
test_align_command_repairs_a_split() {
	base_scenario
	routes "$WAN_ROUTE
$MODEM_ROUTE" \
		'default via fe80::1 dev eth1 metric 50
default via fe80::2 dev eth2 metric 10' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::2 dev eth2 src 2409:8a00::40 uid 0'

	run status
	check_eq 'align-before split' '1' "$(sv split)"
	check_eq 'align-before ipv4' 'wan' "$(sv active4)"
	check_eq 'align-before ipv6' 'modem' "$(sv active6)"

	run align --wait
	check_eq 'align rc' '0' "$rc"
	check_eq 'align is a committed task' 'COMMITTED' "$(state_of state)"
	check_eq 'align is labelled as an alignment' 'align' "$(state_of kind)"
	check_eq 'align put both families on one exit' "$(active_dev 4)" "$(active_dev 6)"
	check_eq 'align kept the IPv4 owner' 'eth1' "$(active_dev 4)"
	check_eq 'align deletes nothing' '0' "$(route_dels)"
	check_eq 'align touches no interface' '0' "$(action_count)"
}

# What the LuCI page reads while (and after) a switch: the state machine stage, the
# target, the reason, the phase durations and the group slots.
test_switch_progress_is_reported_to_luci() {
	base_scenario
	both_exits_up

	run switch-worker modem_first 1
	run status
	check_eq 'progress state' 'COMMITTED' "$(sv switch_state)"
	check_eq 'progress target' 'modem' "$(sv switch_target)"
	check_eq 'progress target mode' 'modem_first' "$(sv switch_target_mode)"
	check_eq 'progress result' 'ok' "$(sv switch_result)"
	check_eq 'progress is not busy' '0' "$(sv switch_busy)"
	check_eq 'progress message' '1' "$(grep -c '^switch_message=switch committed' "$SC/out.txt")"
	check_eq 'progress phase timings' '1' \
		"$(grep -c '^switch_phases=.*WAIT_IPV4:.*VERIFY_IPV6:' "$SC/out.txt")"
	check_eq 'progress elapsed' '1' "$(grep -c '^switch_elapsed=[0-9]' "$SC/out.txt")"
	check_eq 'progress active group' 'modem' "$(sv group_active)"
	check_eq 'progress primary group' 'modem' "$(sv group_primary)"
	check_eq 'progress active slot' '10' "$(sv slot4_modem)"
	check_eq 'progress standby slot' '50' "$(sv slot4_wan)"
	check_eq 'progress gateways' '10.13.35.1' "$(sv gw4_modem)"
	check_eq 'progress readiness flags' '1' "$(sv group_ready_modem)"
	check_eq 'progress address flags' '1' "$(sv addr6_wan)"
	check_eq 'progress family split flag' '0' "$(sv split)"
}

# The switch must not be gated on DNS: after a switch the resolver list usually
# still contains the previous exit's servers, and a resolver that is unreachable
# over the new exit costs one timeout, not the whole switch.
test_dns_is_reported_but_never_gates_a_switch() {
	base_scenario
	both_exits_up
	uci_set h5000m_netmode.settings.dns_check 1
	ubus_set 2_1 'up=true' 'available=true' 'pending=false' 'l3_device="eth2"' 'device="eth2"' \
		'dns-server=["223.5.5.5","2400:3200::1"]'

	run switch-worker modem_first 1
	check_eq 'dns does not block the switch' '0' "$rc"
	check_eq 'dns switch committed' 'COMMITTED' "$(state_of state)"

	run status
	check_eq 'dns check flag' '1' "$(sv dns_check)"
	check_eq 'dns verdict is present' '1' "$(grep -c '^dns_ok=' "$SC/out.txt")"
	check_eq 'dns lists the live exit resolvers' '223.5.5.5 2400:3200::1' \
		"$(sv dns_servers_active | tr -s ' ' | sed 's/ *$//')"
	check_eq 'dns does not touch the exit' 'modem' "$(sv group_active)"
}

# Every wait is bounded and the production defaults are what the documentation
# promises.  A hostile configuration cannot make a wait unbounded or a failure
# instant.
# The clock is the arithmetic the whole switch budget hangs on.  It is parsed out
# of /proc/uptime, whose fraction is a two-digit centisecond string - and a value
# like "08" is an invalid OCTAL literal.  dash aborts the expression, the clock
# prints nothing, and the budget that consumed it became `$(( + 3000 ))`: an
# already-expired budget, in ~10% of the switches, because the fraction happened
# to start with a zero.  Every probe then stopped after its first attempt, a
# healthy target was declared unreachable and the switch rolled back.
test_clock_is_decimal_under_any_fraction() {
	base_scenario

	run now-cs
	check_eq 'clock reads a plain value' '1' "$(grep -cE '^[1-9][0-9]+$' "$SC/out.txt")"

	for pair in '1234.08:123408' '1234.09:123409' '100.00:10000' '7.5:750' \
		'12:1200' '0.01:1' '1234.99:123499' '1234.10:123410'; do
		set_clock "${pair%%:*}"
		run now-cs
		check_eq "clock parse ${pair%%:*}" "${pair#*:}" "$(sed -n 1p "$SC/out.txt")"
	done

	# A clock that cannot be read at all must not invent a value.
	set_clock 'garbage'
	run now-cs
	check_eq 'clock falls back to the wall clock' '1' "$(grep -cE '^[1-9][0-9]+$' "$SC/out.txt")"
	clear_clock
}

# The same bug, seen from the outcome the user cares about: with a leading-zero
# fraction on the clock the switch must still commit.  This is the regression
# test for the flapping switch - it failed about one run in ten before the parse
# was fixed, and the failure looked like a dead 5G uplink.
test_switch_survives_a_leading_zero_clock() {
	base_scenario
	both_exits_up
	set_clock '1234.08'

	run switch-worker modem_first 1
	clear_clock
	check_eq 'leading-zero clock rc' '0' "$rc"
	check_eq 'leading-zero clock committed' 'COMMITTED' "$(state_of state)"
	check_eq 'leading-zero clock moved the exit' 'eth2' "$(active_dev 4)"
	check_eq 'leading-zero clock rolled nothing back' '0' "$(grep -c 'rollback' "$SC/logger.log")"
}

test_switch_limits_are_bounded() {
	base_scenario
	# base_scenario pins short waits so a timeout can be observed quickly; this
	# test is about the shipped defaults, so the pins are removed first.
	uci_del h5000m_netmode.settings.switch_wait_ipv4
	uci_del h5000m_netmode.settings.switch_wait_ipv6
	uci_del h5000m_netmode.settings.switch_budget

	run status
	check_eq 'default IPv4 wait' '15' "$(sv switch_wait_ipv4)"
	check_eq 'default IPv6 wait' '20' "$(sv switch_wait_ipv6)"
	check_eq 'default budget' '60' "$(sv switch_budget)"
	check_eq 'probe threshold' '2/3' "$(sv probe_ok_threshold)"
	check_eq 'fail streak starts at zero' '0' "$(sv modem6_fail_streak)"
	check_eq 'default cooldown' '20' "$(sv switch_cooldown)"
	check_eq 'default debounce' '2' "$(sv hotplug_debounce)"
	check_eq 'default confirmation rounds' '2' "$(sv align_confirm)"

	uci_set h5000m_netmode.settings.switch_wait_ipv4 0
	uci_set h5000m_netmode.settings.switch_wait_ipv6 0
	uci_set h5000m_netmode.settings.switch_budget 1
	uci_set h5000m_netmode.settings.probe_attempts 1
	uci_set h5000m_netmode.settings.probe_ok 9
	run status
	check_eq 'a zero wait is floored' '1' "$(sv switch_wait_ipv4)"
	check_eq 'a zero budget is floored' '5' "$(sv switch_budget)"
	check_eq 'the probe threshold cannot exceed the attempts' '1/1' "$(sv probe_ok_threshold)"
}

# A source audit, because these are the mechanisms that must not come back: the
# switch works by moving default-route priorities, so nothing may restart the
# network stack, take an interface down, or switch IPv6 off to make a
# single-family target acceptable.
test_no_restart_or_ipv6_disable_mechanisms() {
	base_scenario

	local f code token
	for f in "$script" "$tests_dir/../root/etc/hotplug.d/iface/95-h5000m-netmode" \
		"$tests_dir/../root/etc/init.d/h5000m-netmode"; do
		[ -f "$f" ] || continue
		# Comments are stripped first: the files explain at length why these
		# mechanisms are not used, and that prose must not read as a violation.
		code="$(grep -v '^[[:space:]]*#' "$f")"
		for token in 'ifdown ' 'init.d/network' 'network restart' 'network reload' \
			'firewall restart' 'reload_config' 'disable_ipv6' 'ip6assign' 'ip6prefix'; do
			check_eq "$(basename "$f") has no $token" '0' \
				"$(printf '%s' "$code" | grep -c -- "$token")"
		done
	done
}

# ---------------------------------------------------------------------------
# runner
# ---------------------------------------------------------------------------

test_names="
test_fib_oracle_beats_main_table
test_metric_order_is_not_dump_order
test_symbolic_device_reference_is_resolved
test_proxy_tunnel_is_not_a_family_split
test_proxy_tunnel_attributes_to_the_proxy_exit
test_real_family_split_is_still_reported
test_tunnel_without_proxy_state_follows_ipv4_physical_exit
test_split_egress_is_repaired_without_dropping_a_family
test_failover_aligns_both_families_without_shutting_anything_down
test_no_ipv4_default_keeps_ipv6_untouched
test_dual_stack_target_is_required
test_stable_state_produces_no_churn
test_status_is_read_only_and_complete
test_wireless_state_is_reported
test_health_probe_defaults_on
test_iface_role_classification
test_eth_fallback_sections_are_not_modem
test_manual_mapping_is_serialised
test_shared_device_is_reported
test_usage_errors
test_reconcile_lock_behaviour
test_switch_moves_both_families_to_the_target
test_switch_back_to_the_wired_wan
test_broken_target_ipv6_keeps_the_old_exit
test_target_without_ipv4_never_touches_the_old_exit
test_ipv4_only_target_is_refused
test_half_committed_switch_is_rolled_back
test_switch_requests_are_serialised
test_dead_worker_does_not_block_the_next_switch
test_hotplug_burst_is_coalesced
test_one_failed_probe_does_not_fail_over
test_sustained_loss_fails_over_to_the_complete_exit
test_align_command_repairs_a_split
test_switch_progress_is_reported_to_luci
test_dns_is_reported_but_never_gates_a_switch
test_switch_limits_are_bounded
test_no_restart_or_ipv6_disable_mechanisms
test_clock_is_decimal_under_any_fraction
test_switch_survives_a_leading_zero_clock
"

printf 'backend: %s\n\n' "$script"
only="${2:-}"
for case_name in $test_names; do
	if [ -n "$only" ]; then
		case "$case_name" in
			*"$only"*) ;;
			*) continue ;;
		esac
	fi
	before=$failures
	"$case_name"
	if [ "${H5_KEEP:-0}" = "1" ]; then
		printf 'scenario(%s) %s\n' "$case_name" "$SC"
	fi
	if [ "$failures" = "$before" ]; then
		printf 'ok   %s\n' "$case_name"
	else
		printf 'FAIL %s\n' "$case_name"
		if [ "${H5_KEEP:-0}" = "1" ]; then
			printf '  scenario: %s\n' "$SC"
			[ -f "$SC/state" ] && sed 's/^/  state: /' "$SC/state"
			[ -f "$SC/logger.log" ] && tail -n 6 "$SC/logger.log" | sed 's/^/  log: /'
		fi
	fi
done

if [ -n "$SC" ] && [ "${H5_KEEP:-0}" != "1" ]; then
	rm -rf "$SC"
fi
if [ "${H5_KEEP:-0}" = "1" ] && [ -n "$SC" ]; then
	printf 'scenario kept: %s\n' "$SC"
fi

printf '\n%d checks, %d failure(s)\n' "$checks" "$failures"
[ "$failures" = "0" ] || exit 1
exit 0
