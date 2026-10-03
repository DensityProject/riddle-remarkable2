#!/usr/bin/env python3
"""Plug the reMarkable into the Mac once and the AI works over Wi-Fi after.

Runs on the Mac under launchd and watches for the tablet's USB network
(10.11.99.1). Each time the cable goes in, over SSH it:
  - reads the tablet's Wi-Fi address and puts it on the bridge's allowlist
    (the `allow` file next to bridge.py, which the bridge re-reads)
  - writes the Mac's current addresses and the bridge token into
    inkwell.env and the diary's oracle.env (only the bridge lines)
  - makes sure key-only SSH over Wi-Fi is on (OS updates remove it)
If the Mac later changes networks while the tablet is reachable over Wi-Fi,
it re-syncs over Wi-Fi without the cable.

Nothing is sent anywhere but the tablet, and the token goes over SSH stdin,
never on a command line or in the log.
"""
import ipaddress, json, os, re, shlex, socket, subprocess, sys, time

HERE = os.path.dirname(os.path.abspath(__file__))
USB_TABLET = "10.11.99.1"
USB_MAC = "10.11.99.2"
PORT = int(os.environ.get("BRIDGE_PORT", "8788"))
STATE = os.path.join(HERE, "link.json")
ALLOW_FILE = os.path.join(HERE, "allow")
TOKEN_FILE = os.path.join(HERE, "token")
APPS = "/home/root/xovi/exthome/appload"
POLL = 4
MAX_ALLOW = 8
# The tablet's host key is the same on USB and Wi-Fi; pin it under the USB
# name so a changed Wi-Fi address never needs a new known_hosts entry.
SSH = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=6",
       "-o", f"HostKeyAlias={USB_TABLET}", "-o", "StrictHostKeyChecking=yes"]

# Key-only SSH on wlan0, same units as installed by hand on 2026-10-03.
WIFI_SSH = r"""
if [ ! -f /etc/systemd/system/sshkey-wlan.socket ]; then
cat > /etc/systemd/system/sshkey-wlan.socket <<'EOF'
[Unit]
Description=Key-only SSH over Wi-Fi (no passwords)
Conflicts=dropbear-wlan.socket dropbear.service
Requires=home.mount
After=home.mount

[Socket]
ListenStream=22
BindToDevice=wlan0
Accept=yes

[Install]
WantedBy=sockets.target sys-subsystem-net-devices-wlan0.device
EOF
cat > /etc/systemd/system/sshkey-wlan@.service <<'EOF'
[Unit]
Description=Key-only SSH per-connection server (Wi-Fi)
Wants=dropbearkey.service
After=syslog.target dropbearkey.service

[Service]
ExecStart=-/usr/sbin/dropbear -i -s -g -G root -r /etc/dropbear/dropbear_ed25519_host_key
StandardInput=socket
KillMode=process
EOF
rm-ssh-over-wlan off >/dev/null 2>&1 || true
systemctl daemon-reload
echo wifi_ssh=installed
fi
# The socket has been seen stopped with Wi-Fi up (cause unknown, logs
# rotated): a 1-minute timer starts it again if it is ever down.
if [ ! -f /etc/systemd/system/sshkey-wlan-watch.timer ]; then
cat > /etc/systemd/system/sshkey-wlan-watch.service <<'EOF'
[Unit]
Description=Restart key-only Wi-Fi SSH if it stopped

[Service]
Type=oneshot
ExecStart=/bin/systemctl start sshkey-wlan.socket
EOF
cat > /etc/systemd/system/sshkey-wlan-watch.timer <<'EOF'
[Unit]
Description=Check key-only Wi-Fi SSH every minute

[Timer]
OnBootSec=1min
OnUnitActiveSec=1min

[Install]
WantedBy=timers.target
EOF
systemctl daemon-reload
systemctl enable --now sshkey-wlan-watch.timer >/dev/null 2>&1
echo wifi_ssh_watch=installed
fi
systemctl is-active -q sshkey-wlan.socket || systemctl enable --now sshkey-wlan.socket >/dev/null 2>&1
echo wifi_ssh=$(systemctl is-active sshkey-wlan.socket)
"""

# Tailscale (userspace, no /dev/net/tun on the rM2) lets the tablet reach
# the Mac from any network, including ones that isolate Wi-Fi clients. Its
# binaries live in /home (kept across OS updates); the unit in /etc isn't.
TAILSCALE = r"""
TS=/home/root/tailscale
if [ -x $TS/tailscaled ]; then
if [ ! -f /etc/systemd/system/tailscaled.service ]; then
cat > /etc/systemd/system/tailscaled.service <<'EOF'
[Unit]
Description=Tailscale (userspace) so Inkwell can reach the Mac bridge from any network
Requires=home.mount
After=home.mount network-online.target

[Service]
ExecStart=/home/root/tailscale/tailscaled --tun=userspace-networking --statedir=/home/root/.tailscale --socket=/run/tailscale/tailscaled.sock --socks5-server=127.0.0.1:1055 --outbound-http-proxy-listen=127.0.0.1:1055
Restart=on-failure
RestartSec=5
RuntimeDirectory=tailscale

[Install]
WantedBy=multi-user.target
EOF
systemctl daemon-reload
fi
systemctl is-active -q tailscaled || systemctl enable --now tailscaled >/dev/null 2>&1
echo tailnet=$($TS/tailscale --socket=/run/tailscale/tailscaled.sock ip -4 2>/dev/null | head -n 1)
fi
"""

PROBE = WIFI_SSH + TAILSCALE + r"""
echo wifi=$(ip -4 -o addr show wlan0 2>/dev/null | awk '{print $4}' | head -n 1)
"""

# Reads the token from stdin, then sets KEY=VALUE lines in place (keeping
# every other line). $1 = inkwell bridges, $2 = diary bases.
WRITE = r"""
umask 077
read -r TOKEN
setkv() {
    [ -d "$(dirname "$1")" ] || return 0
    touch "$1"
    grep -v "^$2=" "$1" > "$1.tmp"; echo "$2=$3" >> "$1.tmp"
    chmod 600 "$1.tmp"; mv "$1.tmp" "$1"
}
setkv APPS/inkwell/inkwell.env INKWELL_BRIDGES "$1"
setkv APPS/inkwell/inkwell.env INKWELL_TOKEN "$TOKEN"
setkv APPS/riddle/oracle.env RIDDLE_OPENAI_BASE "$2"
setkv APPS/riddle/oracle.env RIDDLE_OPENAI_KEY "$TOKEN"
# The diary reads oracle.env only at launch and lingers after AppLoad closes
# it; stop it so the next open picks up the new address. Inkwell re-reads
# inkwell.env on every Ask, so it keeps running.
killall riddle 2>/dev/null && echo diary=restarted
echo written
""".replace("APPS", APPS)


def log(msg):
    print(time.strftime("%Y-%m-%d %H:%M:%S ") + msg, flush=True)


def notify(msg):
    subprocess.run(["osascript", "-e", f'display notification {json.dumps(msg)} with title "reMarkable"'],
                   capture_output=True)


def reachable(host, port=22, timeout=1.5):
    try:
        socket.create_connection((host, port), timeout).close()
        return True
    except OSError:
        return False


def mac_networks():
    """The Mac's LAN IPv4 interfaces (not loopback, USB, link-local or VPN)."""
    out = subprocess.run(["ifconfig"], capture_output=True, text=True).stdout
    nets, iface = [], ""
    for line in out.splitlines():
        m = re.match(r"^(\w+):", line)
        if m:
            iface = m.group(1)
        m = re.search(r"inet (\d+\.\d+\.\d+\.\d+) netmask (0x[0-9a-f]+)", line)
        if not m or not iface.startswith("en"):
            continue
        ip = ipaddress.ip_address(m.group(1))
        if ip.is_loopback or ip.is_link_local or str(ip).startswith("10.11.99."):
            continue
        nets.append(ipaddress.ip_interface(f"{ip}/{bin(int(m.group(2), 16)).count('1')}"))
    return nets


def mac_tailnet_ip():
    """The Mac's Tailscale address: a 100.64.0.0/10 address on a utun
    interface. (The Tailscale CLI returns nothing when run from launchd.)"""
    out = subprocess.run(["ifconfig"], capture_output=True, text=True).stdout
    iface = ""
    for line in out.splitlines():
        m = re.match(r"^(\w+):", line)
        if m:
            iface = m.group(1)
        m = re.search(r"inet (100\.\d+\.\d+\.\d+)", line)
        if m and iface.startswith("utun") and ipaddress.ip_address(m.group(1)) in ipaddress.ip_network("100.64.0.0/10"):
            return m.group(1)
    return ""


def ssh(host, script, args=(), stdin=""):
    """Run `script` with `args` on the tablet; secrets go only via stdin."""
    cmd = " ".join(shlex.quote(x) for x in ["sh", "-c", script, "sh", *args])
    r = subprocess.run(SSH + [f"root@{host}", cmd], input=stdin, capture_output=True, text=True, timeout=40)
    if r.returncode != 0:
        raise RuntimeError(f"ssh {host} exit {r.returncode}: {r.stderr.strip()[-300:]}")
    return dict(l.split("=", 1) for l in r.stdout.splitlines() if "=" in l), r.stdout


def load_state():
    try:
        with open(STATE) as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def save_state(st):
    with open(STATE + ".tmp", "w") as f:
        json.dump(st, f, indent=1)
    os.replace(STATE + ".tmp", STATE)


def sync(host, via):
    kv, _ = ssh(host, PROBE)
    wifi = kv.get("wifi", "")
    tablet_net = ipaddress.ip_interface(wifi) if wifi else None
    nets = mac_networks()
    same = [n for n in nets if tablet_net and n.ip in tablet_net.network]
    other = [n for n in nets if n not in same]
    url = lambda ip: f"http://{ip}:{PORT}/v1"
    tablet_ts, mac_ts = kv.get("tailnet", ""), mac_tailnet_ip()
    # Same-network Wi-Fi first (works without the cable), then USB, then
    # Tailscale (any network, slower first hop), then the Mac's other
    # addresses in case the tablet joins that network later. Inkwell moves
    # whichever answered last to the front.
    order = ([url(n.ip) for n in same] + [url(USB_MAC)] + ([url(mac_ts)] if mac_ts and tablet_ts else [])
             + [url(n.ip) for n in other])
    diary = ",".join(order)  # the diary also tries each in order

    # Remember the tablet's recent addresses too (home Wi-Fi, hotspot, ...)
    # so it is still let in after it moves networks without the cable. The
    # bearer token is still required on every request.
    allow = [USB_TABLET] + ([str(tablet_net.ip)] if tablet_net else []) + ([tablet_ts] if tablet_ts else [])
    try:
        with open(ALLOW_FILE) as f:
            allow += [l.strip() for l in f if l.strip()]
    except OSError:
        pass
    allow = list(dict.fromkeys(allow))[:MAX_ALLOW]
    with open(ALLOW_FILE + ".tmp", "w") as f:
        f.write("\n".join(allow) + "\n")
    os.replace(ALLOW_FILE + ".tmp", ALLOW_FILE)

    token = open(TOKEN_FILE).read().strip()
    kv2, out = ssh(host, WRITE, [",".join(order), diary], stdin=token + "\n")
    if "written" not in out:
        raise RuntimeError("tablet did not confirm the write")
    st = {"tablet_wifi": str(tablet_net.ip) if tablet_net else "", "tablet_tailnet": tablet_ts,
          "mac_tailnet": mac_ts, "mac": [str(n.ip) for n in nets],
          "bridges": order, "synced": time.strftime("%Y-%m-%d %H:%M:%S"), "via": via}
    save_state(st)
    log(f"synced via {via}: tablet wifi={st['tablet_wifi'] or 'off'} mac={st['mac']} "
        f"tailnet={tablet_ts or '-'}->{mac_ts or '-'} bridges={order} wifi_ssh={kv.get('wifi_ssh')}{' diary restarted' if kv2.get('diary') else ''}")
    if tablet_ts and mac_ts:
        notify("Linked. AI works on any network via Tailscale; you can unplug.")
    elif not tablet_net:
        notify("Linked over USB. The tablet's Wi-Fi is off, so the AI needs the cable.")
    elif not same:
        notify(f"Linked, but the tablet ({tablet_net.ip}) is on a different network from this Mac "
               f"({', '.join(str(n.ip) for n in nets) or 'offline'}). AI works over USB until they share Wi-Fi.")
    else:
        notify(f"Linked. AI works over Wi-Fi now ({same[0].ip}); you can unplug.")


def main():
    log(f"watching for the tablet on {USB_TABLET}, bridge port {PORT}")
    usb_was_up = False
    while True:
        try:
            st = load_state()
            usb_up = reachable(USB_TABLET)
            if usb_up and not usb_was_up:
                sync(USB_TABLET, "usb")
            elif not usb_up:
                mac = [str(n.ip) for n in mac_networks()]
                wifi = st.get("tablet_wifi")
                if wifi and mac and mac != st.get("mac") and reachable(wifi):
                    sync(wifi, "wifi")
            usb_was_up = usb_up
        except Exception as e:
            # Not marked as up: the next poll retries (the tablet often
            # sleeps right as it's plugged in).
            usb_was_up = False
            log(f"sync failed: {e}")
            time.sleep(POLL * 2)
        time.sleep(POLL)


if __name__ == "__main__":
    if sys.argv[1:] == ["--once"]:
        sync(USB_TABLET if reachable(USB_TABLET) else load_state().get("tablet_wifi", USB_TABLET), "manual")
    else:
        main()
