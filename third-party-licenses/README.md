# Supplemental third-party license texts

These files supplement the license files copied from the Cargo package cache by
the release packaging process. Some published crates omit their workspace's root
licenses, and some bundled fonts/data use license filenames that an ordinary
`LICENSE*`/`NOTICE*` scan does not detect. Keep this entire directory with the
distributed application, alongside the automatically collected dependency
licenses and inventory. This is a version-specific supplement, not a complete
inventory of every dependency.

The upstream license files are reproduced byte-for-byte. `sources.json` records
the exact upstream source URLs, commits, Git blob SHA-1 values, and SHA-256 values
of the local copies. The two extracted notice files identify their source files
and the extraction method instead. No license text was replaced with a generic
template. Retrieval and verification date: 2026-10-04.

## Coverage

| Local directory | Cargo packages covered | License texts |
| --- | --- | --- |
| `clipboard-win-5.4.1/` | `clipboard-win` 5.4.1 | BSL-1.0 |
| `egui-0.31.1/` | `ecolor`, `eframe`, `egui`, `egui-winit`, `egui_glow`, `emath`, `epaint`, and the Rust library code in `epaint_default_fonts`, all 0.31.1 | MIT and Apache-2.0 (upstream alternatives) |
| `epaint_default_fonts-0.31.1/` | The four bundled fonts in `epaint_default_fonts` 0.31.1 | Hack's MIT/Bitstream Vera notices; Noto Emoji's OFL-1.1; Ubuntu Font Licence 1.0; emoji-icon-font's MIT notice; embedded font copyright/trademark metadata |
| `gl_generator-0.14.0/` | `gl_generator` 0.14.0 | Apache-2.0 |
| `khronos_api-3.1.0/` | `khronos_api` 3.1.0 and its bundled registry data | Apache-2.0; ANGLE's BSD-style license; embedded Khronos/ANGLE notices, including the EGL/WebGL permission notices |
| `profiling-1.0.18/` | `profiling` 1.0.18 | MIT and Apache-2.0 (upstream alternatives) |
| `winapi-x86_64-pc-windows-gnu-0.4.0/` | `winapi-x86_64-pc-windows-gnu` 0.4.0 | MIT and Apache-2.0 (upstream alternatives) |

## Exact upstream sources

Except for the old winapi import-library package described below, the source
commit was taken from the published crate's `.cargo_vcs_info.json`.

- clipboard-win: [LICENSE](https://github.com/DoumanAsh/clipboard-win/blob/3b27cf2bfd1adcfa6e0264eb51c1025ddaf0f342/LICENSE)
  at commit `3b27cf2bfd1adcfa6e0264eb51c1025ddaf0f342`.
- egui workspace: [LICENSE-MIT](https://github.com/emilk/egui/blob/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/LICENSE-MIT)
  and [LICENSE-APACHE](https://github.com/emilk/egui/blob/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/LICENSE-APACHE)
  at commit `1669e52a7ccfc3489c1b0999b9ed48894a0b3887`.
  All eight packages above record this same commit. Their workspace licenses are
  stored once in `egui-0.31.1/`.
- egui fonts: [Hack-Regular.txt](https://github.com/emilk/egui/blob/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/crates/epaint_default_fonts/fonts/Hack-Regular.txt),
  [OFL.txt](https://github.com/emilk/egui/blob/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/crates/epaint_default_fonts/fonts/OFL.txt),
  [UFL.txt](https://github.com/emilk/egui/blob/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/crates/epaint_default_fonts/fonts/UFL.txt),
  and [emoji-icon-font-mit-license.txt](https://github.com/emilk/egui/blob/1669e52a7ccfc3489c1b0999b9ed48894a0b3887/crates/epaint_default_fonts/fonts/emoji-icon-font-mit-license.txt)
  at that same commit. The cached copies and source font bytes were verified
  against the official repository's Git blob hashes. `FONT-NOTICES.txt` preserves
  actual copyright/trademark/license name-table records from those fonts,
  including Noto Emoji's Google and Ubuntu's Canonical copyright notices.
- gl_generator: [LICENSE](https://github.com/rust-windowing/gl-rs/blob/ea503e8d5fb6d73c6030e6191ce738cd3bf3433e/LICENSE)
  at commit `ea503e8d5fb6d73c6030e6191ce738cd3bf3433e`.
- khronos_api: [LICENSE](https://github.com/rust-windowing/gl-rs/blob/f150967b1c44ae888e6676f93f639ebc82771bdc/LICENSE)
  at commit `f150967b1c44ae888e6676f93f639ebc82771bdc`.
  The former `brendanzab/gl-rs` repository redirects to `rust-windowing/gl-rs`.
  Both version-specific root licenses have Git blob SHA-1
  `d645695673349e3947e8e5ae42332d0ac3164cd7`.
- profiling: [LICENSE-MIT](https://github.com/aclysma/profiling/blob/8271551172eb6fa4cba47369aedd93790c623df9/LICENSE-MIT)
  and [LICENSE-APACHE](https://github.com/aclysma/profiling/blob/8271551172eb6fa4cba47369aedd93790c623df9/LICENSE-APACHE)
  at commit `8271551172eb6fa4cba47369aedd93790c623df9`.
- winapi import libraries: [LICENSE-MIT](https://github.com/retep998/winapi-rs/blob/9497609ef44cc9bcd16cd2411c0ee6ccaf5483aa/LICENSE-MIT)
  and [LICENSE-APACHE](https://github.com/retep998/winapi-rs/blob/9497609ef44cc9bcd16cd2411c0ee6ccaf5483aa/LICENSE-APACHE)
  at commit `9497609ef44cc9bcd16cd2411c0ee6ccaf5483aa`.
  This old crate has no `.cargo_vcs_info.json`. The selected commit's
  [x86_64/Cargo.toml](https://github.com/retep998/winapi-rs/blob/9497609ef44cc9bcd16cd2411c0ee6ccaf5483aa/x86_64/Cargo.toml)
  declares version 0.4.0 and exactly matches the published `Cargo.toml.orig`
  (Git blob SHA-1 `fe5a8d0a3eec54cab7b8afe7d81ef8d8c742fd49`). All 1,419 published
  original manifest, source, build-script, and import-library files were also
  verified against this commit. Cargo-generated packaging metadata was excluded
  from that comparison.

## Bundled Khronos registry data

The khronos_api source commit records these exact submodule revisions. Its
`.gitmodules` identifies their official repositories. `REGISTRY-NOTICES.txt`
retains the copyright/license comments from all 11 bundled OpenGL, EGL, ANGLE,
and WebGL XML/IDL files that contain them. Every full source file was matched to
its upstream Git blob hash before extracting the comments.

- [KhronosGroup/OpenGL-Registry](https://github.com/KhronosGroup/OpenGL-Registry/tree/85d924a5165e7206f4c73c8c10307658dc2bbe10),
  commit `85d924a5165e7206f4c73c8c10307658dc2bbe10`.
- [KhronosGroup/EGL-Registry](https://github.com/KhronosGroup/EGL-Registry/tree/b6b558e7042d67fc7543383bb0e13a075f21d8ab),
  commit `b6b558e7042d67fc7543383bb0e13a075f21d8ab`.
- [google/angle](https://github.com/google/angle/tree/7403dd2cd3764fe96660fe09892e764e9ae1dbca),
  commit `7403dd2cd3764fe96660fe09892e764e9ae1dbca`.
  [Its LICENSE](https://github.com/google/angle/blob/7403dd2cd3764fe96660fe09892e764e9ae1dbca/LICENSE)
  is reproduced separately as `ANGLE-LICENSE` because the ANGLE XML headers refer
  to that file and it is absent from the published khronos_api crate.
- [KhronosGroup/WebGL](https://github.com/KhronosGroup/WebGL/tree/c987f075bfdca44119175c41f547d849c08983a3),
  commit `c987f075bfdca44119175c41f547d849c08983a3`.

When dependency versions change, re-check this supplement against the new
published source archives and upstream revisions instead of carrying the old
version coverage forward automatically.
