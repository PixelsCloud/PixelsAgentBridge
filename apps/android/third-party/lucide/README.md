# Lucide icons

Functional icons throughout the Android app use Lucide 0.468.0, licensed under ISC
(with the upstream Feather MIT notice retained in LICENSE). The Pixels brand image
remains a brand asset; it is not approximated using a hand-drawn functional icon.

Official sources: https://github.com/lucide-icons/lucide/tree/0.468.0/icons

`icons/` contains untouched upstream SVG files. `sources.toml` pins their hashes.
Run `python scripts/android_icons.py` to generate Android VectorDrawables offline,
or `python scripts/android_icons.py --check` to verify the generated files. The
converter retains the original geometry, 24-unit viewport, round caps and joins.
Normal stroke width is 2; selected navigation icons use width 2.5, supported by
Lucide's stroke-width customization. Only the icons used by this app are bundled.
The upstream license is also packaged in the APK at `assets/licenses/lucide.txt`.

Choose future icons from this same upstream set. To deliberately refresh the
pinned release, update VERSION and run `python scripts/android_icons.py --download`.
