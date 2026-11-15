# Driving termwall from other programs

termwall has four ways in and out. All of them are local: files next to the page, commands, and
an HTTP server on 127.0.0.1. Nothing here needs termwall's source.

Where the files are: the install folder (`%LOCALAPPDATA%\Programs\termwall` on Windows, a clone
anywhere); for a Linux package, settings in `$XDG_CONFIG_HOME/termwall` and `token.js` in
`$XDG_DATA_HOME/termwall`. `TERMWALL_THEME` and `TERMWALL_STYLE` point `theme.json` and
`termwall.toml` somewhere else.

## 1. Colors: `theme.json`

Write this file and the wallpaper recolors within a second, without a reload. This is how a
wallpaper switcher, a Matugen or wallust template, or a script hands termwall a palette.

```json
{
  "bg": "#171717", "accent": "#b5f4e7", "secondary": "#7faca3",
  "text": "#939fa1", "dim": "#4b5252", "faint": "#242626"
}
```

- All six keys, each `#rrggbb`. Anything else and the file is ignored (the page keeps its colors).
- termwall follows it while `theme = "auto"` in `termwall.toml` (the default). A theme the user
  picked there, or their own `colors`, wins over this file.
- Optional `"style": { ... }` in the same file sets the look for this palette with the keys of
  `termwall.toml` (`layout`, `bars`, `logo`, `effects` ...). It wins over `termwall.toml`;
  `[screen.N]` tables there win over it. Unknown keys and bad values are dropped one by one.
- Write it in place or by rename, either works; termwall re-reads when the modification time
  changes.

Or let termwall write it:

```
termwall palette #171717 #b5f4e7 #7faca3 #939fa1 #4b5252 #242626    # bg accent secondary text dim faint
termwall palette accent=#ff5555 dim=#444444                          # some of them
termwall palette                                                     # show it
termwall palette off                                                 # remove it ("style" stays)
```

Exit code 0 when written, 2 for colors it doesn't accept.

## 2. The look: `termwall.toml` and `termwall --style`

Every setting with its choices is in the comments of `termwall.toml`. From a program:

```
termwall --style layout board        # one setting; exit code 1 and "error: ..." if refused
termwall --style hide gpu,procs      # lists: comma-separated, none = empty
termwall --style layout default      # back to the default
termwall --style                     # every setting: name, value, choices
termwall check                       # exit code 1 if a line of termwall.toml is wrong
```

The page follows a change within a second. `termwall --style` rewrites the file with its
comments; keys it doesn't know are kept.

## 3. Stats: the HTTP server

`http://127.0.0.1:9002`, GET only, JSON. It listens on the loopback address only.

| path | needs the token | what |
|---|---|---|
| `/health` | no | `{"termwall": "3.0.0", "ok": N, "forbidden": N}`: is it running, which version |
| `/stats?t=TOKEN` | yes | everything below, plus `theme`, `style`, `page` |
| `/theme?t=TOKEN` | yes | `theme` (the palette in effect or null), `style`, `page` only |
| `/logo-image?t=TOKEN` | yes | the user's logo picture, if `logo = "image"` |

Add `&screen=N` to get the look of `[screen.N]`.

The token is new at every start of the server. It is in `token.js` next to the page:

```
window.TERMWALL_TOKEN = "kR3...";
```

Read the file, take what is between the quotes, and read it again when a request answers 403
(the server restarted). The `Host` header has to be `127.0.0.1` or `localhost`.

`/stats` fields:

| field | |
|---|---|
| `user`, `host`, `os`, `os_family`, `distro`, `distro_like`, `kernel`, `board`, `shell` | text |
| `cpu_model`, `cores`, `physical_cores`, `cpu_freq` (MHz or null) | |
| `cpu` (percent), `cores_pct` (one per core) | sampled every second |
| `ram`, `swap` | `{used, total, percent}`, bytes |
| `ram_total`, `boot` (Unix time), `uptime` (seconds), `time` (Unix time of the sample) | |
| `gpu` | `{name, util, temp, vram_used, vram_total, power, clock}` (MiB, W, MHz; any can be null) or null |
| `disks` | `[{mount, fs, used, total, percent}]` |
| `net` | `{down, up}` in bytes per second |
| `local_ip` | `"192.168.1.20 (Ethernet)"` or `"-"` |
| `procs` | the 12 busiest programs: `[{name, cpu, mem, count}]`, one row per name |
| `procs_total` | number of processes |
| `history` | `{cpu, gpu, down, up}`: up to 60 one-second samples each, oldest first |

Without the server: `termwall stats` prints one such sample (it takes a second), `termwall
--once` the same cut to a screenful.

`termwall share` runs a second, read-only server for other devices on the network; its link
carries its own key, and it never sends the user name, computer name or IP. See the README.

## 4. The page inside another host

`index.html` is an ordinary page. It needs `token.js` beside it (or `window.TERMWALL_TOKEN` and
`window.TERMWALL_API = "http://HOST:PORT/stats"` set before its scripts), and takes `?screen=N`.
For the visualizer and the track line it calls Wallpaper Engine's web API, so any host that
defines these before the page's scripts run gets them working:

- `window.wallpaperRegisterAudioListener(callback)`: call `callback` about 30 times a second with
  128 numbers, 64 bands of the left channel from bass up, then 64 of the right, 0 to 1.
- `window.wallpaperRegisterMediaPropertiesListener(callback)`: call it with
  `{title, artist, albumTitle}` when the track changes; an empty `title` clears the line.

`termwall host` does exactly that (`src/host/we_api.js`).

## What is stable

The six palette keys, the settings' names in `termwall.toml`, the paths and fields above, and the
commands' exit codes are kept across 3.x. Fields may be added. The page's markup and CSS classes
are not an interface.
