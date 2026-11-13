termwall 2.4.1 closes a hole in `termwall share` and fixes two smaller bugs. If you use
`termwall share`, update; until you can, `termwall share off`.

## Security: `termwall share`

- The share server answered `/fonts/NAME.woff2` by joining NAME onto the page's folder, and it
  did so before checking the share key. A drive or network path in NAME
  (`fonts/C:\...\x.woff2`, `fonts/\\host\...\x.woff2`) left the folder: anyone who could reach
  the share port (your network, your tailnet) could read any `.woff2` file on the disk, and make
  Windows open an SMB connection to a host of their choosing, which can hand that host the
  sign-in's NTLM hash.
- Fonts are now served by plain file name only (letters, digits, `-`, `_`, `.`). The selftest
  checks it with a real file outside the folder.
- Without `termwall share` turned on, the LAN server doesn't run and none of this applied.

## Fixes

- `fetch`: a long track title under the clock made the spectrum above it as wide as the title.
  The bars now stay as wide as the clock's digits; the title still reaches further left.
- `termwall --stop`, which the installer runs before an update, compared path strings, so it
  missed a server started through another name for the same folder (a junction, a renamed
  folder with a link back, another letter case), and the update could not replace it. It now
  compares the files.
