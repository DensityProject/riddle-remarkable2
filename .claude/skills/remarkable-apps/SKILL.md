---
name: remarkable-apps
description: Use when building, deploying, debugging or testing apps for a reMarkable 2 tablet (AppLoad/xovi apps like Inkwell or the Tom Riddle diary), or the Mac Claude bridge they use. Covers the platform, toolchain, qtfb/AppLoad protocol, input, rendering, testing without the tablet, deploying, networking/AI bridge, and every problem hit so far with its fix.
---

# Building apps for the reMarkable 2

Written after building Inkwell (an AI notebook app) and porting this Tom Riddle diary to the rM2 (PRs #2 and #3 in this repo). Both are Rust. Read the whole file before touching the tablet. Most traps here cost an hour the first time.

## 1. The device

- **reMarkable 2.** 32-bit ARMv7 (armhf), about 1 GB RAM, OS 3.28 at the time of writing (it auto-updates).
- **Screen.** 1404 x 1872 portrait, e-ink. The stock UI is 1-bit black/white with grey only for ink.
- **Main app.** The stock UI is `xochitl` (Qt 6, QML compiled into the binary).
- **Userland is BusyBox.** These are missing or limited:
  - no `pkill`: use `killall` or `kill $(pidof x)`
  - no `timeout`
  - `ps` takes no `-o`
  - `head` needs `-n N`
  - `nc` is minimal
  - no `curl`; `wget` exists but does no TLS validation
  - `openssl` exists
- **Persistence.**
  - `/home` survives OS updates.
  - `/etc` is writable but OS updates wipe your changes there: systemd units, SSH settings.
  - Put binaries and data in `/home/root`.
  - Put units in `/etc/systemd/system`, and have something that reinstalls them (see section 9).

## 2. Getting in (SSH)

- **USB.** `ssh root@10.11.99.1`. The Mac is `10.11.99.2` on that link.
- **Root password.** It's in Settings > Help > About > Copyrights and licenses, at the bottom of the "GPLv3 Compliance" section. The user types it; never enter passwords yourself. Then install the Mac key with `ssh-copy-id`.
- **"No route to host" from Terminal.app.** That's macOS Local Network privacy. Run the command in the Claude app's terminal panel (`run_in_terminal`) instead. launchd agents and the Claude Bash tool can reach the LAN.
- **Host key changes after every OS update.** Check that it's the same device before re-trusting. Use `-o HostKeyAlias=10.11.99.1` so the Wi-Fi IP shares the USB known_hosts entry.
- **Key-only Wi-Fi SSH.**
  - A socket unit `sshkey-wlan.socket` runs `dropbear -i -s -g` (key only).
  - It has stopped silently before, so a 1-minute timer `sshkey-wlan-watch.timer` restarts it.
  - `mac/tablet_link.py` reinstalls both on every plug-in.
- **The tablet sleeps and drops USB constantly.** If both addresses are down, ask the user to wake it. Don't assume it's broken.
- **Finding it on Wi-Fi.** The IP changes per network.
  - Check `~/diary-bridge/link.json`.
  - Or scan with `nc -z` and confirm by host key (the tablet runs dropbear; OpenSSH means it's another machine).

## 3. xovi + AppLoad (how apps get on screen)

- **xovi** injects extensions into xochitl. It is tethered: after every reboot, run `/home/root/xovi/start` over SSH. After an OS update, also run its `rebuild_hashtable`.
- **AppLoad** is the xovi extension that launches apps in a window. The AppLoad version must match the OS:
  - 0.4.x: OS 3.22 to 3.25
  - 0.5.x: OS 3.26 to 3.27
  - 0.6.0: OS 3.28 (needs Qt 6.10)
  - The wrong version crashes xochitl, and the tablet auto-reboots to stock. That's safe but alarming.
  - Keep old builds in `xovi/inactive-extensions`, never delete them.
- **AppLoad 0.4.2 had a repaint bug** (180 to 500 ms per update, so the pen lagged). 0.6.0 fixed it (about 14 ms). If ink is laggy, check the AppLoad version before optimising code.
- **App layout.** `/home/root/xovi/exthome/appload/<app>/` contains `external.manifest.json`, a launch script (`inkwell-launch.sh`), the binary and assets. The launch script sources an env file (`inkwell.env` or `oracle.env`, mode 0600) and `exec`s the binary.
- **Closing.**
  - When the user closes the window, AppLoad sends **SIGTERM**. Handle it: set a flag and break the loop so the app saves.
  - Some apps (the diary) linger after close. Kill them over SSH after updating their config.
- **Native integration** (adding buttons to real xochitl notebooks) is possible via xovi plus qt-resource-rebuilder. It's deferred research, not done.

## 4. qtfb: the window protocol (the part that bit hardest)

- AppLoad sets `QTFB_KEY`. You connect to `/tmp/qtfb.sock`, the server creates shared memory, and you draw RGB565 into it.
- **The ServerMessage is a native C struct with `size_t` alignment.**
  - On 32-bit, the shm key is at offset 4 and the size (u32) at offset 8.
  - On 64-bit, the key is at 8 and the size (u64) at 16.
  - Use `const SRV_PAYLOAD: usize = size_of::<usize>()`. Code written for the 64-bit Paper Pro breaks on the rM2.
- **Format byte 0** (`FBFMT_RM2FB`) means 1404 x 1872 RGB565. The server sizes the window from the format byte only.
- **Updates.**
  - `update_partial(x, y, w, h)` and `update_all()`. Each one costs a xochitl repaint.
  - Coalesce dirty rects and flush about every 12 ms. Never union a small stroke rect with a full-height toolbar rect: that turns into a full-screen repaint. That was the "fast writing lags" bug.
  - Refresh mode UFAST for ink.
- **Input from qtfb.**
  - Pen events carry x, y and pressure, with **no eraser flag**.
  - To know if the eraser end is down, open the Wacom evdev device **read-only, not grabbed** and watch `BTN_TOOL_RUBBER` and `BTN_TOOL_PEN`. That also gives palm rejection.
  - Touch events carry a devId. Fingers should only drive the chrome (page swipe, undo/redo gestures).

## 5. Input (raw evdev, for takeover-style apps)

- **On 32-bit, `struct input_event` is 16 bytes** (24 on 64-bit). Use a size constant per arch.
- There are no sysfs per-axis ranges. Use `ioctl EVIOCGABS(code)` (0x80184540 + code).
- **The Wacom digitizer ("Wacom I2C Digitizer") is rotated:** screen x = raw Y, screen y = max_x - raw_x. Don't detect the pen by the name "marker" (that's the Paper Pro).

## 6. Toolchain and build

- **Rust:** `cargo-zigbuild --target armv7-unknown-linux-gnueabihf.2.27`. The glibc 2.27 suffix matters. Use `zig cc -target arm-linux-gnueabihf.2.27` for C helpers.
- **PATH:** non-login shells don't have cargo. Always `export PATH=$HOME/.cargo/bin:$PATH` in the same command. Once a build failed silently, a stale binary got deployed, and it was only caught by comparing md5. **Always compare `md5` of the local build with `md5sum` on the tablet after deploying.**
- **Dependencies:** keep them light. ureq for HTTP (no TLS needed for the LAN bridge), ab_glyph for fonts, png, serde.

## 7. Making it look like stock

- **The real assets are inside xochitl.**
  - Extract the Qt rcc resource trees from the stripped binary: find the trees by Qt's name hash, and the data is zstd, zlib or raw.
  - The extractor lived in a session scratchpad (`rcc_extract.py`). It's lost now, so rebuild it if needed.
  - The output is Inkwell's `assets/rm` (QML, icon SVGs, fonts, pen textures, keyboard layouts).
  - **These are proprietary: keep them gitignored and never publish or commit them.**
- **Fonts:** reMarkable Sans/Serif files have a **10-byte junk prefix**: strip it with `tail -c +11`.
- **Font sizes:** Qt sizes are em sizes, and ab_glyph's PxScale is ascent-descent. Convert with a `px_scale(font, em)` helper, or all text comes out too small.
- **Icons:** they're SVGs. Rasterise with resvg (Inkwell's `scripts/bake-assets.sh`) and threshold to 1-bit.
- **Behaviour spec:** read the QML.
  - Inkwell's `UI-SPEC.md` has geometry.
  - Inkwell's `STOCK-FEATURES.md` has the behaviour of layers, tags, search, typing, capture, landscape and so on.
  - Tokens are under `ark-imports/ark/tokens`. Foldout rows are 112 px, the label is Medium 25, the icon 48.
- **Templates:** `/usr/share/remarkable/templates`, a JSON vector format with expressions.
- **Stock features that need reMarkable's cloud:**
  - Convert to text: POSTs MyScript strokes to `/convert/v1/handwriting` on the reMarkable cloud, gets JIIX back, and needs a paired device plus internet.
  - Handwriting search.
  - Mimicking them means reverse engineering that API, or using Claude through the bridge.

## 8. Testing without the tablet (do this before every deploy)

- **Headless app mode.** Inkwell has `inkwell --script steps.txt outdir`. It runs the real app against an in-memory surface:
  - Steps are `tap x y`, `down`, `move`, `up`, `wait ms` and `shot name`.
  - Script coordinates are **physical**. Screenshots are **logical** (upright).
  - Fresh process means fresh UI state (the toolbar starts expanded), so don't send a "re-expand" tap.
  - Test on a **copy** of the real data: `ssh ... 'cd /home/root/inkwell && tar cz .' | tar xz`.
  - Required env: `INKWELL_HOME`, `INKWELL_ASSETS`, `INKWELL_TEMPLATES`, `INKWELL_FONT` and `INKWELL_HAND_FONT`.
- **Preview PNG tests.** Write a PNG when `INKWELL_PREVIEW_DIR` is set, then Read the PNG and look at it. Pixel-hash tests keep refactors honest (the keyboard extraction kept 8 states identical).
- **Data compatibility.**
  - New model fields use `#[serde(default, skip_serializing_if = ...)]` so old builds still read new files.
  - Round-trip the real user files in a test (`INKWELL_REALDATA`).
- **`--ask-test` CLI.** Runs an AI request against a saved page through the bridge. Temporarily add `127.0.0.1` to `~/diary-bridge/allow` and restore it after.
- **Full OS emulation.**
  - qemu-system-arm with an Ubuntu 22.04 armhf image, a fake qtfb server built from AppLoad's `common.h`, a uinput fake Wacom and a mock oracle.
  - **qemu-user is NOT valid**: a 64-bit host kernel gives 24-byte evdev events.
- **Screenshots of the real tablet.** Copy the `framebuffer-spy` xovi extension into `extensions.d`. It logs the framebuffer address in the xochitl journal. Then read `/proc/<xochitl pid>/mem` with `/home/root/fbgrab`.

## 9. Deploying

- **Pattern:** `scp` to `app.new`, then `cp app app.prev`, then `mv app.new app`. Keep a frozen known-good build in Inkwell's `dist/` and a source tarball before big changes. **Never delete user files or old copies; archive or rename them.**
- **macOS `tar` adds `._*` files:** use `COPYFILE_DISABLE=1`.
- **Don't `killall` a running app that might have unsaved work.**
  - If the binary has the SIGTERM-save handler, `kill -TERM` and confirm "window closed, saved" in `journalctl`.
  - Otherwise replace the binary and tell the user to close and reopen.
- **Saves are debounced (1.5 s)** for speed. Every page change, close, undo and Ask still saves immediately.
- **Logs:** `journalctl -f -o cat | grep inkwell` (apps log to stderr, which goes to the xochitl journal).

## 10. AI on the tablet (the Mac Claude bridge)

- **Bridge.** `~/diary-bridge/bridge.py` runs as launchd agent `com.nate.diary-bridge` on port 8788. The source is `mac/` in this repo, and `mac/install.sh` installs both agents.
  - It speaks OpenAI-style SSE `/v1/chat/completions`.
  - It answers with `claude -p --input-format stream-json --output-format stream-json --include-partial-messages --tools "" --setting-sources "" --strict-mcp-config --mcp-config '{"mcpServers":{}}' --disable-slash-commands --no-session-persistence --model <ID>`.
  - `--bare` can't use the subscription OAuth.
- **Use explicit model IDs.** CLI aliases lag. Haiku is `claude-haiku-4-5-20251001`, Sonnet `claude-sonnet-5-5`, Opus `claude-opus-5-5`; Opus 5.5 needs CLI 2.1.280 or later.
- **Claude Code thinks before every answer by default.** On a page with long instructions that was **40 to 90 s before the first word**.
  - The bridge passes `--settings '{"alwaysThinkingEnabled":false}'` unless the request has `"thinking": true`. `reasoning_effort` maps to `--effort` only when thinking is on.
  - Inkwell shows a thinking switch for Haiku and effort levels for Sonnet and Opus.
- **OAuth expires.** Symptom: the bridge log says "OAuth session expired", the apps say "didn't add anything". Fix: the user runs `~/.local/bin/claude auth login` (open it in their terminal panel). The bridge now emits a clear message for this case.
- **Security.** Every request needs the bearer token (`~/diary-bridge/token`, 0600, sent to the tablet only over SSH stdin). Only addresses in the `allow` file get in. Never print the token.
- **The bridge log shows timing:** `first text Xs, N chars out, images, KB in`. Use it to separate model time from network time.

## 11. Networking (why "the AI can't reach the Mac")

- **The Mac's IP changes when it changes networks.** Nothing should be typed into env files by hand. `tablet_link.py` (launchd `com.nate.tablet-link`) does it all on every USB plug-in:
  - writes the Mac's addresses and the token into `inkwell.env` and `oracle.env`
  - allowlists the tablet's Wi-Fi and Tailscale IPs (keeping the last 8)
  - restores Wi-Fi SSH and Tailscale
  - posts a notification
  - `tablet_link.py --once <ip>` syncs over Wi-Fi.
- **Route order (important):** same-subnet Wi-Fi, then USB, then other LAN, then **Tailscale last**. Apps also try whichever route answered last first.
- **USB unplugged:** `10.11.99.2` gets "connection refused" immediately. That's fine.
- **A home router can block the tablet's internet** (a parental or security firewall). That makes Tailscale useless there. Log Tailscale in once from a network where the tablet has internet (e.g. a phone hotspot).
- **Tailscale on the tablet.**
  - Userspace mode, because there's no `/dev/net/tun`: `/home/root/tailscale/tailscaled --tun=userspace-networking`, with an HTTP proxy on `127.0.0.1:1055`.
  - Apps send `100.64.0.0/10` bases through that proxy.
  - **With no internet, the proxy answers 502 or just hangs.** Treat 502 to 504 as "try the next route", and keep Tailscale last.
  - Under launchd, the Mac's Tailscale CLI prints nothing. Read the Mac's tailnet IP from the `utun` interface instead.
  - Check the Mac has no stale exit node set (an offline one kills its internet). Clear it with `tailscale up --exit-node=`.
- **School/enterprise Wi-Fi isolates clients,** so only Tailscale or USB works there.

## 12. Rotation / landscape (if you need it)

- **Rotate only at the edges:**
  - The Surface maps logical to physical in every primitive (`put_px`, `luma`, `fill`, `copy`/`paste`, `invert`).
  - Input is converted with `logical_of` once, in the run loop.
  - Dirty rects are converted to physical rects in `flush`.
- Page size is runtime (`model::pw()/ph()`), and the UI screen size is `ui::screen()`. Only the notebook view rotates; full-screen windows stay portrait.
- 14 toolbar tiles don't fit 1404 px. Move Tags and Share into the more menu, as stock does.
- Portrait-designed bottom parts (the keyboard) draw through a Surface offset: bottom-anchored, centred.
- Switching orientation rotates the stored coordinates, so ink stays put. Round-trip test it on real data.

## 13. Working style that worked (and what went wrong)

- **Subagents.**
  - Give each agent a **new file** with a fixed API. They must not edit shared files like `app.rs`, `model.rs` or `toolbar.rs`; integrate those yourself, one at a time.
  - Agents die on API connection errors and rate limits. Resume them with SendMessage, and check whether they left partial work.
- **Check the real behaviour first.** Ask a cheap agent to extract the stock behaviour from the QML before designing a feature.
- **Freeze before big changes:** a tarball of `src`, plus a dist binary.
- **Things that were missed and later caught:**
  - finger gestures (page swipe, two-finger undo) changing the page under an open editor or panel, which can crash
  - the toolbar state updated without a redraw
  - PDF export dropping typed text
  - a UI pill keeping old coordinates after rotation
  - Test touch paths, not only pen.
