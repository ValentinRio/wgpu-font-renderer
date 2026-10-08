# Font fixture

Cantarell-VF.otf is the Cantarell variable font, version 0.303, distributed under the
SIL Open Font License 1.1. Its embedded name table includes the copyright,
license notice, and license URL (http://scripts.sil.org/OFL). The full license
text is in [OFL.txt](OFL.txt).
Copyright 2019 The Cantarell Project Authors
(https://gitlab.gnome.org/GNOME/cantarell-fonts).

Copied unchanged from `owned_ttf_parser` 0.21.0's `fonts/Cantarell-VF.otf`
in the Cargo registry. Upstream source:
https://github.com/alexheretic/owned-ttf-parser/tree/v0.21.0/fonts

This fixture uses CFF2 outlines. Tests check every outlined glyph's segment
chaining and padded bounds. The GPU regression test renders "Hi" and requires
ink in the expected box, a dark H stem, a white H gap, and a bounded ink count.
Contours are closed and winding is selected from each glyph's total signed area.
Variable fonts are loaded at their default variation instance.
