#!/usr/bin/env python3
"""Pin the netlink layout against a real kernel.

`cargo test` can assert the bytes `build_request` produces but not what the
kernel does with them - and both bugs this module shipped with produced
plausible-looking buffers plus a silent -EINVAL:

  * `struct rtmsg` written as 9 bytes instead of 12 (rtm_flags is a u32, and
    the attribute array must start 4-byte aligned at nlmsghdr + 12);
  * `rtm_type` = RTN_UNSPEC(0) instead of RTN_UNICAST(1).

Neither is visible without a kernel, so this script replays the exact wire
image the Rust backend builds inside a throwaway network namespace and checks
that the route actually lands.

Usage:
    python3 tests/netlink_netns.py

Exits 0 when the checks pass, or when `unshare -n` is unavailable (no
privileges, no namespaces) - a skipped check says nothing either way.
"""

import os
import socket
import struct
import subprocess
import sys

AF_INET = 2
AF_INET6 = 10
RTM_NEWROUTE = 24
RTM_DELROUTE = 25
RTM_GETROUTE = 26
NLMSG_ERROR = 2
NLMSG_DONE = 3
NLM_F_REQUEST = 0x0001
NLM_F_ACK = 0x0004
NLM_F_REPLACE = 0x0100
NLM_F_CREATE = 0x0400
NLM_F_DUMP = 0x0300
NLMSG_HDRLEN = 16
RTMSG_LEN = 12
RT_TABLE_MAIN = 254
RTN_UNICAST = 1
RTPROT_BOOT = 3
RTA_OIF = 4
RTA_GATEWAY = 5
RTA_PRIORITY = 6

failures = 0


def check(label, ok, extra=""):
    global failures
    print(("ok   " if ok else "FAIL ") + label + ("  " + extra if extra else ""))
    if not ok:
        failures += 1


def attr(rta_type, payload):
    length = 4 + len(payload)
    aligned = (length + 3) & ~3
    out = struct.pack("HH", aligned, rta_type) + payload
    return out + b"\x00" * (aligned - length)


def build_request(family, msg_type, flags, oif=None, via=None, metric=None):
    """The wire image src/network/netlink.rs::build_request produces."""
    buf = bytearray()
    buf += struct.pack("<I", 0)  # nlmsg_len, patched below
    buf += struct.pack("<H", msg_type)
    buf += struct.pack("<H", flags)
    buf += struct.pack("<I", 0)  # seq
    buf += struct.pack("<I", 0)  # pid
    # struct rtmsg - 12 bytes
    buf += bytes([family, 0, 0, 0, RT_TABLE_MAIN, RTPROT_BOOT, 0, RTN_UNICAST])
    buf += struct.pack("<I", 0)  # rtm_flags
    if oif is not None:
        buf += attr(RTA_OIF, struct.pack("<I", oif))
    if via is not None:
        buf += attr(RTA_GATEWAY, via)
    if metric is not None:
        buf += attr(RTA_PRIORITY, struct.pack("<I", metric))
    struct.pack_into("<I", buf, 0, len(buf))
    return bytes(buf)


def send(msg, dump=False, timeout=5.0):
    sock = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW, 0)
    sock.bind((0, 0))
    sock.settimeout(timeout)
    sock.send(msg)
    raw = b""
    try:
        while True:
            chunk = sock.recv(65536)
            if not chunk:
                break
            raw += chunk
            if not dump or done_present(raw):
                break
    except socket.timeout:
        pass
    sock.close()
    return raw


def done_present(raw):
    off = 0
    while off + NLMSG_HDRLEN <= len(raw):
        length, mtype = struct.unpack("<IH", raw[off:off + 6])
        if length < NLMSG_HDRLEN:
            return False
        if mtype == NLMSG_DONE:
            return True
        off += (length + 3) & ~3
    return False


def request(family, msg_type, flags, oif=None, via=None, metric=None):
    raw = send(build_request(family, msg_type, flags, oif, via, metric))
    if len(raw) < NLMSG_HDRLEN:
        return None, -1
    length, mtype = struct.unpack("<IH", raw[:6])
    if mtype == NLMSG_ERROR:
        return raw[NLMSG_HDRLEN:length], struct.unpack("<i", raw[16:20])[0]
    return raw[NLMSG_HDRLEN:length], 0


def dump_payloads(family):
    """RTM_GETROUTE + NLM_F_DUMP, the way route_proto() sees the FIB."""
    msg = build_request(family, RTM_GETROUTE, NLM_F_REQUEST | NLM_F_DUMP)
    # A dump request carries struct rtgenmsg (one `family` byte), not rtmsg.
    msg = msg[:NLMSG_HDRLEN] + bytes([family])
    struct.unpack("<I", msg[:4])  # keep the length word where the parser wants it
    msg = struct.pack("<I", NLMSG_HDRLEN + 1) + msg[4:]
    raw = send(msg, dump=True)
    out = []
    off = 0
    while off + NLMSG_HDRLEN <= len(raw):
        length, mtype = struct.unpack("<IH", raw[off:off + 6])
        if length < NLMSG_HDRLEN or off + length > len(raw):
            break
        if mtype == NLMSG_DONE:
            break
        if mtype == RTM_NEWROUTE:
            out.append(raw[off + NLMSG_HDRLEN:off + length])
        off += (length + 3) & ~3
    return out


def parse(payload):
    """parse_route_info(): proto lives at rtmsg offset 5, attrs at offset 12."""
    proto = payload[5] if len(payload) > 5 else 0
    attrs = payload[RTMSG_LEN:] if len(payload) > RTMSG_LEN else b""
    oif, gw, metric = 0, None, 0
    off = 0
    while off + 4 <= len(attrs):
        alen, atype = struct.unpack("<HH", attrs[off:off + 4])
        if alen < 4 or off + alen > len(attrs):
            break
        value = attrs[off + 4:off + alen]
        if atype == RTA_OIF and len(value) >= 4:
            oif = struct.unpack("<I", value[:4])[0]
        elif atype == RTA_GATEWAY:
            gw = value
        elif atype == RTA_PRIORITY and len(value) >= 4:
            metric = struct.unpack("<I", value[:4])[0]
        off += alen
    return oif, gw, proto, metric


def route_proto(family, via, oif, metric):
    for payload in dump_payloads(family):
        if len(payload) > 1 and payload[1] == 0:  # rtm_dst_len == 0
            o, g, proto, m = parse(payload)
            if m == metric and (oif is None or o == oif) and (via is None or g == via):
                return proto
    return None


def sh(*args):
    return subprocess.run(args, capture_output=True, text=True)


def main():
    sh("ip", "link", "set", "lo", "up")
    sh("ip", "link", "add", "h5test0", "type", "dummy")
    sh("ip", "link", "set", "h5test0", "up")
    sh("ip", "addr", "add", "10.44.0.1/24", "dev", "h5test0")
    # The ifindex must come from inside this namespace: /sys/class/net is not
    # guaranteed to follow a netns change.
    out = sh("ip", "-o", "link", "show", "h5test0").stdout
    if not out:
        print("FAIL could not create a test interface")
        return 1
    oif = int(out.split(":")[0])
    gw = socket.inet_aton("10.44.0.254")

    flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_REPLACE
    _, err = request(AF_INET, RTM_NEWROUTE, flags, oif=oif, via=gw, metric=10)
    check("RTM_NEWROUTE is accepted by the kernel", err == 0, "err=%d" % err)
    routes = sh("ip", "route", "show", "default").stdout
    check("the default route landed", "10.44.0.254" in routes and "metric 10" in routes,
          routes.strip())

    proto = route_proto(AF_INET, gw, oif, 10)
    check("route_proto() reads RTPROT_BOOT back", proto == RTPROT_BOOT, "proto=%r" % proto)
    check("is_route_boot() therefore holds", proto == RTPROT_BOOT)

    # A route this program did not install must not be claimed as boot.
    sh("ip", "route", "replace", "default", "via", "10.44.0.253", "dev", "h5test0",
       "metric", "20", "proto", "static")
    other = route_proto(AF_INET, socket.inet_aton("10.44.0.253"), oif, 20)
    check("a foreign route is not reported as boot", other != RTPROT_BOOT, "proto=%r" % other)

    # Same metric, new next hop: the atomic "move the slot" semantics.
    gw2 = socket.inet_aton("127.0.0.9")
    _, err = request(AF_INET, RTM_NEWROUTE, flags, oif=1, via=gw2, metric=10)
    check("replace moves the slot at the same metric", err == 0, "err=%d" % err)
    routes = sh("ip", "route", "show", "default").stdout
    check("the slot holds exactly one next hop",
          "10.44.0.254" not in routes and "127.0.0.9" in routes, routes.strip())

    _, err = request(AF_INET, RTM_DELROUTE, NLM_F_REQUEST | NLM_F_ACK, oif=1, via=gw2,
                     metric=10)
    check("RTM_DELROUTE is accepted by the kernel", err == 0, "err=%d" % err)

    print("")
    if failures:
        print("%d check(s) failed" % failures)
        return 1
    print("netlink layout verified against %s" % os.uname().release)
    return 0


if __name__ == "__main__":
    if os.environ.get("H5_NETNS_CHILD") == "1":
        sys.exit(main())
    env = dict(os.environ, H5_NETNS_CHILD="1")
    try:
        proc = subprocess.run(["unshare", "-n", sys.executable, __file__], env=env,
                              capture_output=True, text=True)
    except (FileNotFoundError, PermissionError, OSError) as exc:
        print("skipped: no usable network namespace (%s)" % exc)
        sys.exit(0)
    # unshare(1) itself failing (no privileges, userns disabled) is not a check
    # failure: the util-linux binary prints its own diagnostic and exits
    # non-zero instead of raising, so catch that here. A non-zero exit without
    # an unshare diagnostic means the child checks genuinely failed.
    if proc.returncode != 0 and "unshare" in (proc.stderr or "").lower():
        print("skipped: no usable network namespace (%s)" % proc.stderr.strip())
        sys.exit(0)
    sys.stdout.write(proc.stdout)
    sys.stderr.write(proc.stderr)
    sys.exit(proc.returncode)
