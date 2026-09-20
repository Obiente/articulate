# Bundled visual assets

## Articulate mark

`brand/articulate-mark.svg` is an original vector interpretation of the handwritten
lowercase a in the user-selected Articulate design. It uses a closed SVG path with
an even-odd counter, with no font dependency or embedded raster image. The mark is
part of this project's own visual identity and is distributed under the project's
MIT license. It is not part of Inter or Phosphor Icons.

## Inter

`fonts/Inter-Regular.ttf` and `fonts/Inter-Medium.ttf` are the unmodified static
TrueType files from [Inter 4.1](https://github.com/rsms/inter/releases/tag/v4.1),
created by Rasmus Andersson and contributors.

- Project: <https://rsms.me/inter/>
- Source archive: <https://github.com/rsms/inter/releases/download/v4.1/Inter-4.1.zip>
- Archive SHA-256: `9883fdd4a49d4fb66bd8177ba6625ef9a64aa45899767dde3d36aa425756b11e`
- Archive paths: `extras/ttf/Inter-Regular.ttf`, `extras/ttf/Inter-Medium.ttf`
- License: SIL Open Font License 1.1. The complete notice is in [fonts/OFL.txt](fonts/OFL.txt).

## Phosphor icons

The SVG files in `icons/` are the regular-weight icons from
[Phosphor Icons](https://phosphoricons.com/), copyright (c) 2023 Phosphor Icons.

- Upstream repository: <https://github.com/phosphor-icons/core>
- Pinned source revision: `2b75f3ad12b420c9504ef05df8d2564a28f8500e`
- Source directory: <https://github.com/phosphor-icons/core/tree/2b75f3ad12b420c9504ef05df8d2564a28f8500e/assets/regular>
- Included icons: `microphone`, `phone`, `book-open`, `lightning`, `gear`, `copy`,
  `arrow-counter-clockwise`, `lightbulb`, `lock`, `caret-down`, `stop`, `x`.
- Modification: the root SVG fill is explicitly white (`#ffffff`) so the native
  UI can tint each icon. Geometry and the original 256 by 256 view box are unchanged.
- License: MIT. The complete notice is in [icons/LICENSE.txt](icons/LICENSE.txt).

Keep both complete license notices with redistributed assets.
