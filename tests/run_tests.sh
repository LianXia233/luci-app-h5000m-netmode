#!/bin/sh
#
# Deterministic unit tests for the h5000m-netmode backend.
#
# The backend decides which uplink is live and keeps IPv4 and IPv6 on the same
# exit, so the interesting cases are exactly the ones that are unsafe to provoke
# on a production router: policy routing, a foreign IPv6 default, a modem with no
# IPv6 interface, a lost hotplug event.  Those cases are constructed here instead.
#
# The script under test runs unmodified.  Only uci/ubus/ip/jsonfilter/pgrep/
# logger/ifup/ifdown/ping and the sysfs root are mocked, through the seams the
# script exposes; the interpreter and every other utility are the real ones.  All
# writes land inside the throwaway scenario directory, so running this on a live
# router cannot touch its configuration.
#
# The suite is POSIX sh on purpose: run it on the router to validate against
# BusyBox ash, and in CI to validate against dash.
#
# Usage:
#   sh tests/run_tests.sh [path/to/h5000m-netmode]
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
	[ -n "$SC" ] && rm -rf "$SC"
	SC="$(mktemp -d)"
	mkdir -p "$SC/uci" "$SC/ubus" "$SC/iwinfo" "$SC/routes" "$SC/sysfs"
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

netdev() { # <dev> [carrier]
	mkdir -p "$SC/sysfs/$1"
	if [ -n "${2:-}" ]; then
		printf '%s' "$2" > "$SC/sysfs/$1/carrier"
	fi
}

# ---------------------------------------------------------------------------
# running the backend
# ---------------------------------------------------------------------------

run() {
	H5_SCEN="$SC" \
	H5_MOCKBIN="$mockbin" \
	H5000M_LOCK_DIR="$SC/lock" \
	H5000M_DAED_STATE="$SC/daed-exit" \
	H5000M_HEALTH_STATE="$SC/health" \
	H5000M_SYSFS_NET="$SC/sysfs" \
	H5000M_SELF="$script" \
	PATH="$mockbin:$PATH" \
	sh "$script" "$@" > "$SC/out.txt" 2> "$SC/err.txt"
	rc=$?
}

sv() { # status value of a key from the last run
	sed -n "s/^$1=//p" "$SC/out.txt" | head -n 1
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

# Does any route line in <text> egress through <dev>?  Used to reproduce what the
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

log_has() {
	[ -f "$SC/logger.log" ] && grep -q -- "$1" "$SC/logger.log"
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
	uci_set network.2_1v6.auto 0
	uci_set network.wan6.defaultroute 1
	uci_set network.wan6.auto 1

	uci_set h5000m_netmode.settings settings
	uci_set h5000m_netmode.settings.mode wan_first
	uci_set h5000m_netmode.settings.ipv6_owner wan
	# Health probing is opt-out, so every other case pins it off: their behaviour
	# must not depend on a probe verdict, and the default itself is covered by
	# test_health_probe_defaults_on.
	uci_set h5000m_netmode.settings.health_check 0

	netdev eth0 0
	netdev eth1 1
	netdev eth2 1
	netdev br-lan 1

	ubus_set wan 'up=true' 'available=true' 'pending=false' 'l3_device="eth1"' 'device="eth1"'
	ubus_set wan6 'up=true' 'available=true' 'pending=false' 'l3_device="eth1"' 'device="eth1"'
	ubus_set 2_1 'up=true' 'available=true' 'pending=false' 'l3_device="eth2"' 'device="eth2"'
	ubus_set 2_1v6 'up=false' 'available=true' 'pending=false' 'device="eth2"'
}

# ---------------------------------------------------------------------------
# tests
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
$WAN_ROUTE" '' '' ''

	run status
	check_eq 'metric-order rc' '0' "$rc"
	check_eq 'metric-order egress4' 'eth1' "$(sv egress4)"
	check_eq 'metric-order active4' 'wan' "$(sv active4)"
	check_eq 'metric-order old oracle disagrees' 'modem' "$(old_owner 'eth2' 'eth1' 'eth2')"
}

# netifd reports symbolic references such as "@2_1" for child interfaces, and
# nothing at all for a section it never started.  A route's dev is never "@2_1",
# so without expansion every modem IPv6 route looks foreign and the IPv6
# alignment fights the live IPv4 exit.
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

# Invariant 2: IPv4 on the wired WAN but IPv6 leaked onto the modem must be
# repaired towards the IPv4 exit.
test_split_egress_is_repaired_towards_ipv4() {
	base_scenario
	routes "$WAN_ROUTE
$MODEM_ROUTE" \
		'default via fe80::1 dev eth2 metric 50' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::1 dev eth2 src 2409::1 uid 0'
	ubus_set 2_1v6 'up=true' 'available=true' 'pending=false' 'l3_device="eth2"' 'device="eth2"'

	run status
	check_eq 'split-before split' '1' "$(sv split)"
	check_eq 'split-before active4' 'wan' "$(sv active4)"
	check_eq 'split-before active6' 'modem' "$(sv active6)"

	run reconcile
	check_eq 'split-rc' '0' "$rc"
	check_eq 'split 2_1v6.defaultroute' '0' "$(uci_of network.2_1v6.defaultroute)"
	check_eq 'split 2_1v6.auto' '0' "$(uci_of network.2_1v6.auto)"
	check_eq 'split wan6.defaultroute' '1' "$(uci_of network.wan6.defaultroute)"
	check_eq 'split wan6.auto' '1' "$(uci_of network.wan6.auto)"
	check_eq 'split ipv6_owner' 'wan' "$(uci_of h5000m_netmode.settings.ipv6_owner)"
	check_contains 'split tears the modem IPv6 down' '^ifdown 2_1v6$' "$SC/iface-actions.log"
	check_absent 'split leaves wan6 alone' '^ifdown wan6$' "$SC/iface-actions.log"
}

# After a failover IPv6 must follow IPv4, and the stale family must be removed
# before the new owner is raised so the two are never live at once.
test_failover_moves_ipv6_and_drops_old_family_first() {
	base_scenario
	uci_set h5000m_netmode.settings.mode modem_first
	routes "$MODEM_ROUTE" \
		'default via fe80::942b:33ff:fecd:8306 dev eth1 metric 512' \
		'1.1.1.1 via 10.13.35.1 dev eth2 src 10.13.35.40 uid 0' \
		'2606:4700:4700::1111 via fe80::942b:33ff:fecd:8306 dev eth1 src 2409::1 uid 0'

	run reconcile
	check_eq 'failover rc' '0' "$rc"
	check_eq 'failover wan6.defaultroute' '0' "$(uci_of network.wan6.defaultroute)"
	check_eq 'failover wan6.auto' '0' "$(uci_of network.wan6.auto)"
	check_eq 'failover 2_1v6.defaultroute' '1' "$(uci_of network.2_1v6.defaultroute)"
	check_eq 'failover 2_1v6.auto' '1' "$(uci_of network.2_1v6.auto)"
	check_eq 'failover ipv6_owner' 'modem' "$(uci_of h5000m_netmode.settings.ipv6_owner)"

	check_contains 'failover tears wan6 down' '^ifdown wan6$' "$SC/iface-actions.log"
	check_contains 'failover raises the modem IPv6' '^ifup 2_1v6$' "$SC/iface-actions.log"
	if [ -n "$(action_index 'ifdown wan6')" ] && [ -n "$(action_index 'ifup 2_1v6')" ]; then
		check_ok 'failover removes the old family before raising the new one' \
			"$([ "$(action_index 'ifdown wan6')" -lt "$(action_index 'ifup 2_1v6')" ] && echo 0 || echo 1)"
	fi
}

# With no IPv4 default there is nothing to stay consistent with, so IPv6 must be
# left alone instead of being torn down.
test_no_ipv4_default_keeps_ipv6_untouched() {
	base_scenario
	routes '' 'default via fe80::942b:33ff:fecd:8306 dev eth1 metric 512' '' \
		'2606:4700:4700::1111 via fe80::942b:33ff:fecd:8306 dev eth1 src 2409::1 uid 0'

	run reconcile
	check_eq 'no-v4 rc' '0' "$rc"
	check_eq 'no-v4 wan6.defaultroute untouched' '1' "$(uci_of network.wan6.defaultroute)"
	check_eq 'no-v4 wan6.auto untouched' '1' "$(uci_of network.wan6.auto)"
	check_eq 'no-v4 no iface churn' '0' "$(action_count)"
	check_eq 'no-v4 no network commit' '0' "$(writes_count 'commit network')"

	run status
	check_eq 'no-v4 active4' 'none' "$(sv active4)"
}

# IPv4 on the modem while no modem IPv6 interface exists: IPv6 cannot follow, so
# managed IPv6 is switched off rather than leaking out of the wired uplink.
test_missing_modem_ipv6_disables_ipv6() {
	base_scenario
	uci_del network.2_1v6
	uci_del network.2_1v6.device
	uci_del network.2_1v6.modem_config
	uci_del network.2_1v6.defaultroute
	uci_del network.2_1v6.auto
	ubus_del 2_1v6
	routes "$MODEM_ROUTE" '' \
		'1.1.1.1 via 10.13.35.1 dev eth2 src 10.13.35.40 uid 0' ''

	run reconcile
	check_eq 'no-modem6 rc' '0' "$rc"

	run status
	check_eq 'no-modem6 capable' '0' "$(sv ipv6_capable_modem)"
	check_eq 'no-modem6 desired' 'off' "$(sv ipv6_desired)"
	check_eq 'no-modem6 ipv6_owner' 'off' "$(uci_of h5000m_netmode.settings.ipv6_owner)"
	check_eq 'no-modem6 wan6.defaultroute' '0' "$(uci_of network.wan6.defaultroute)"
	check_eq 'no-modem6 wan6.auto' '0' "$(uci_of network.wan6.auto)"
	check_contains 'no-modem6 tears wan6 down' '^ifdown wan6$' "$SC/iface-actions.log"
}

# A consistent state must produce no writes and no interface churn, and a second
# run must be equally quiet.
test_stable_state_produces_no_churn() {
	base_scenario
	routes "$WAN_ROUTE
$MODEM_ROUTE" \
		'default via fe80::942b:33ff:fecd:8306 dev eth1 metric 512' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::942b:33ff:fecd:8306 dev eth1 src 2409::1 uid 0'

	run reconcile
	check_eq 'stable rc' '0' "$rc"
	check_eq 'stable no iface actions' '0' "$(action_count)"
	check_eq 'stable no network commit' '0' "$(writes_count 'commit network')"
	check_eq 'stable ipv6_owner unchanged' 'wan' "$(uci_of h5000m_netmode.settings.ipv6_owner)"

	run reconcile
	check_eq 'stable idempotent' '0' "$(action_count)"
}

# `status` is polled by LuCI every five seconds: it must be read-only and must keep
# every key the shipped frontend consumes.
test_status_is_read_only_and_complete() {
	base_scenario
	routes "$WAN_ROUTE
$MODEM_ROUTE" \
		'default via fe80::942b:33ff:fecd:8306 dev eth1 metric 512' \
		'1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' \
		'2606:4700:4700::1111 via fe80::942b:33ff:fecd:8306 dev eth1 src 2409::1 uid 0'

	run status
	check_eq 'status rc' '0' "$rc"
	check_eq 'status writes nothing' '0' "$(writes_total)"

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
	routes "$WAN_ROUTE
$MODEM_ROUTE" '' '1.1.1.1 via 192.168.88.1 dev eth1 src 192.168.88.187 uid 0' ''

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
	check_eq 'health caches wan verdict' '1' "$(sed -n 's/^wan=//p' "$SC/health" | head -n 1)"
	check_eq 'health caches modem verdict' '1' "$(sed -n 's/^modem=//p' "$SC/health" | head -n 1)"
	check_eq 'health records a timestamp' '1' "$(grep -c '^ts=' "$SC/health")"

	# A verdict younger than health_probe_interval must be reused even though a
	# fresh probe would contradict it (the ping mock always succeeds).
	printf 'wan=0\nmodem=0\nts=%s\n' "$(date +%s)" > "$SC/health"
	run reconcile
	check_eq 'health throttles a fresh verdict' '0' "$(sed -n 's/^wan=//p' "$SC/health" | head -n 1)"

	# interval=0 disables the throttle and the verdict is refreshed again.
	uci_set h5000m_netmode.settings.health_probe_interval 0
	run reconcile
	check_eq 'health interval 0 refreshes' '1' "$(sed -n 's/^wan=//p' "$SC/health" | head -n 1)"

	# A stale verdict is refreshed without touching the interval.
	printf 'wan=0\nmodem=0\nts=1\n' > "$SC/health"
	uci_del h5000m_netmode.settings.health_probe_interval
	run reconcile
	check_eq 'health refreshes a stale verdict' '1' "$(sed -n 's/^wan=//p' "$SC/health" | head -n 1)"
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

	run reconcile
	check_contains 'shared device warning' 'mapped to both exits' "$SC/logger.log"
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
}

# ---------------------------------------------------------------------------
# helpers used by the tests above
# ---------------------------------------------------------------------------

old_owner() { # <dev> <wan devices> <modem devices> — the previous oracle
	case " $2 " in
		*" $1 "*) echo wan; return 0 ;;
	esac
	case " $3 " in
		*" $1 "*) echo modem; return 0 ;;
	esac
	if [ -n "$1" ]; then echo other; else echo none; fi
}

test_names="
test_fib_oracle_beats_main_table
test_metric_order_is_not_dump_order
test_symbolic_device_reference_is_resolved
test_split_egress_is_repaired_towards_ipv4
test_failover_moves_ipv6_and_drops_old_family_first
test_no_ipv4_default_keeps_ipv6_untouched
test_missing_modem_ipv6_disables_ipv6
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
	if [ "$failures" = "$before" ]; then
		printf 'ok   %s\n' "$case_name"
	else
		printf 'FAIL %s\n' "$case_name"
	fi
done

[ -n "$SC" ] && rm -rf "$SC"

printf '\n%d checks, %d failure(s)\n' "$checks" "$failures"
[ "$failures" = "0" ] || exit 1
exit 0
