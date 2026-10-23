termwall 2.3 turns a picture into a logo you can edit by hand.

## termwall logo-from

- `termwall logo-from PICTURE` writes `logo.txt` in fastfetch's format (`$1`…`$9` switch the
  color) and sets `logo = "custom"` with the picture's colors in `logo_colors`. The text can
  be edited by hand, shared, or sent to fastfetch.
- Up to nine colors, the most used one `$1`, each the average of its own pixels.
- A plain background around the picture goes, together with the colors where it blended
  into the edges; a background-colored detail inside the picture stays. Specks of up to
  three pixels apart from the rest (sparkles, noise) go too.
- Half blocks by default, two pixels a character. A character has one color, so where the
  two pixels differ the rarer one wins and lines and highlights survive a fill. `--ascii`
  draws with characters from light to dense.
- Options: `--width N` (40), `--colors N` (1-9), `--keep-bg`, `--out FILE` (write only that
  file, settings untouched), `--force` (replace an existing `logo.txt`). At most 120 x 40
  characters, what `logo.txt` takes.
- PNG is read with the standard library, nothing to install. GIF (its first frame), JPEG,
  WebP and BMP need Pillow.
- `logo = "image"` still draws a picture or an animated GIF live, for one you don't want to
  edit.
