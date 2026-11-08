# Older configuration spellings

Configs written for earlier Halley releases keep loading. Halley still reads
the older spellings below, but the generated config and these docs use the
current ones only, and new spellings are never added to this list. The full
list lives in `halley-config/src/aliases.rs`, and keybind actions have their
own table under [Older action spellings](keybinds.md#older-action-spellings).

Two spellings are always accepted and are not "older": `snake_case` for any
key (`bg_colour` is `bg-colour`), and the American `color` for any key that
contains `colour`.

## Renamed sections

An older section name works for every key in the section it became.

| Older section | Current section |
|---|---|
| `background:`, `gesso:` | `wallpaper:` |
| `overlay:` | `overlays:` |
| `tile:` | `clusters:` → `tiling:` |
| `stacking:` | `clusters:` → `stacking:` |

## Renamed keys

| Older key | Current key |
|---|---|
| `clusters.cluster-dwell-ms`, `clusters.dwell-ms` | `clusters.join-dwell-ms` |
| `clusters.distance-px` | `clusters.join-distance-px` |
| `tile.queue-show-icons` | `clusters.tiling.overflow-show-icons` |
| `field.pins.badge-corner` | `field.pins.corner` |
| `field.pins.pin-colour` | `field.pins.colour` |
| `field.pins.bg-colour` | `field.pins.background-colour` |
| `debug.show-focus-ring` | `debug.show-decay-zone` |
| `view.output` → `focus-ring:` | `view.output` → `decay-zone:` |
