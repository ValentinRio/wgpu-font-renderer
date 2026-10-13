# Fonts

Every font used by the examples, tests and benchmark lives here.

| File | Outlines | License |
|------|----------|---------|
| `Roboto-Regular.ttf` | TrueType | Apache License 2.0, see [`../LICENSE-APACHE`](../LICENSE-APACHE) |
| `Cantarell-VF.otf` | CFF2, variable | SIL Open Font License 1.1, see [`OFL-Cantarell.txt`](OFL-Cantarell.txt) |
| `NotoSerifSC-Subset.otf` | CFF | SIL Open Font License 1.1, see [`OFL-NotoSerifSC.txt`](OFL-NotoSerifSC.txt) |

## Roboto

Roboto Regular, version 2.137. Copyright 2011 Google Inc. The embedded name
table carries the Apache License 2.0 notice.

## Cantarell

Cantarell variable font, version 0.303. Copyright 2019 The Cantarell Project
Authors (https://gitlab.gnome.org/GNOME/cantarell-fonts). Its embedded name
table includes the copyright, license notice, and license URL.

Copied unchanged from `owned_ttf_parser` 0.21.0's `fonts/Cantarell-VF.otf`.
Upstream source:
https://github.com/alexheretic/owned-ttf-parser/tree/v0.21.0/fonts

Variable fonts are loaded at their default variation instance.

## Noto Serif SC (subset)

A subset of Noto Serif SC Regular. Copyright 2017-2024 Adobe
(http://www.adobe.com/). Noto is a trademark of Google Inc.

The full font is 11 MB, so this file keeps only the characters the example and
tests draw: `龍鬱齉直接从轮廓渲染字形文本`. It is a modified version of the
original, made with HarfBuzz 8.3.0:

```sh
hb-subset NotoSerifSC-Regular.otf --text="龍鬱齉直接从轮廓渲染字形文本" --layout-features='' -o NotoSerifSC-Subset.otf
```

Source file (sha256 `e8f396decc1f0963a016a989c3d8852e863d1350996f573860a80767c83a1cd3`):
https://github.com/notofonts/noto-cjk/blob/main/Serif/SubsetOTF/SC/NotoSerifSC-Regular.otf
