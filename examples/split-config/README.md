# Split Config Example

This directory shows how to split a Halley config with `gather`.

`halley.rune` is the entry point. The gathered files are loaded relative to it,
so keep the files together when copying this example into your config directory.

Values in `halley.rune` can override values from gathered files. In this example,
`field.rune` defines the pin badge colours and `halley.rune` overrides only the
pin glyph colour.

Keep `keybinds`, `autostart`, and `rules` in the main config file. Those sections
use Halley's inline/raw parsing and are safest there.
