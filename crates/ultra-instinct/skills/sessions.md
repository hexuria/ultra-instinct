# ultra-instinct: sessions and browsers

A reference of `aui skills get core`. Read the core guide first.

Every verb takes `--session <name>` (letters, digits, `-`; default
`default`) and `--cdp <url>`. A verb that starts a new session must name
its browser: `aui open --launch [--headed] [--profile <dir>]` makes the
daemon start its own Chrome (headless at 1440×900 unless `--headed`, which
opens maximized), which ends when the daemon does; `--proxy
scheme://host:port` (http, https, socks4, socks5; no user name or password)
sends that Chrome's requests through a proxy; `--profile <dir>` runs it
on that profile folder and keeps it, so a login there lasts (`aui close`
lets that Chrome quit cleanly, up to 5 s). Use a folder of its own: your
browser's everyday profile folder, an empty value, or a profile another
Chrome has open is refused. Or start Chrome first (step 2) and pass
`--cdp http://127.0.0.1:<port>`; `--launch` with `--cdp` refuses (pick
one). With neither, the
verb refuses ("no browser named") rather than try port 9222, which could be
the user's own Chrome. A `--launch` session keeps its login across `aui
close` and reopen even without `--profile`: the daemon saves
`<state dir>/<session>.json` (0600) when it ends and restores it when the
next daemon on that session name starts; `AUI_SESSION_STATE=0` turns it off
and `AUI_STATE_DIR` moves the directory (default
`~/Library/Application Support/aui` on macOS,
`${XDG_STATE_HOME:-~/.local/state}/aui` on Linux,
`%LOCALAPPDATA%\aui` on Windows). `--cdp` and `--attach`
sessions keep none. `open` also takes `--allow-domain`, `--confirm-press`,
`--idle <secs>` (the daemon, and its Chrome, end after that long without a
verb: 1 s to 24 h, default 60 min) and `--record <file>` (a frame of the
page after each verb, secret fields hidden as in `aui shot`, lands in
`<file>.frames/`; when the daemon ends, `ffmpeg` assembles them into
`<file>` when it is on `PATH`, else the frames and `index.ffconcat` stand
alone); these and `--launch` are fixed when the daemon starts, so
`aui close` before you change them.

`aui session` prints the session name in use; `aui session list` lists live
sessions and removes stale endpoints.

## Use the user's own Chrome (macOS and Linux; not on Windows yet)

When the user wants their own logged-in Chrome driven (not a fresh one):

1. They open `chrome://inspect/#remote-debugging` in Chrome and turn
   remote debugging on (Chrome 136+ ignores `--remote-debugging-port` on its
   everyday profile; this toggle is the way in).
2. `aui open https://mail.example/ --attach --allow-domain mail.example`.
   `--allow-domain` is required: name every site the task needs. Chrome may
   show an "Allow remote debugging?" prompt; the user accepts it (aui waits
   up to 90 s). `aui doctor` says whether Chrome wrote its port file (a
   Chrome that quit can leave a stale one).
3. Work as usual (`snap`, `click`, …). aui opens its own tab in a new
   Chrome window and never lists, reads, switches to or closes the user's
   tabs.
4. `aui close` closes only that tab and leaves their Chrome running.

`--attach` does not mix with `--launch`, `--cdp`, `--profile` or
`--headed`, and `aui run` does not take it. On the user's own Chrome,
`cookies`, `state save|load`, `storage` and `download` refuse
(`tool_not_allowed`): they reach every site in the profile.
