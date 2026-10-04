# Mac bridge + plug-in auto-link

Runs the diary (and Inkwell) on your Claude subscription through a Mac, over
USB or home Wi-Fi, with no addresses to type in.

- `bridge.py`: OpenAI-compatible `/v1/chat/completions` that answers with
  `claude -p` (all tools, MCP, plugins and settings disabled). Only the
  tablet's addresses get in (`BRIDGE_ALLOW` plus the `allow` file), bearer
  token required, one request at a time.
- `tablet_link.py`: watches for the tablet on USB (10.11.99.1). Every time
  the cable goes in it reads the tablet's Wi-Fi address, allowlists it,
  writes the Mac's current addresses + token into `inkwell.env` and
  `oracle.env`, and turns key-only Wi-Fi SSH back on if an OS update removed
  it. If the Mac changes networks later it re-syncs over Wi-Fi. A macOS
  notification says whether Wi-Fi will work or the two are on different
  networks.
- `install.sh`: installs both as launchd agents into `~/diary-bridge`.

Setup, once:

```sh
mac/install.sh
```

Needs: Claude Code logged in (`~/.local/bin/claude`), and the Mac's SSH key
in the tablet's `authorized_keys` with its host key in `~/.ssh/known_hosts`
under `10.11.99.1`. Then plug the tablet in; unplug after the notification.
Logs: `~/diary-bridge/link.log`, `~/diary-bridge/bridge.log`.

Plain HTTP + token: meant for trusted home Wi-Fi only.
