# Third-Party Notices

XenTerm is released under the [MIT license](LICENSE). It bundles or adapts the
third-party works listed below. Where a licence requires the full text to travel
with the distribution, the text is kept in the repository next to the asset (for
example `assets/fonts/`) and is also embedded in the asset itself where the
licence allows that.

## Twemoji — colour emoji artwork

Colour emoji are rendered from Twemoji graphics embedded in the Rust core.

- Copyright 2020 Twitter, Inc and other contributors.
- Licensed under the Creative Commons Attribution 4.0 International licence:
  <https://creativecommons.org/licenses/by/4.0/>
- Project: <https://github.com/jdecked/twemoji>

The images are embedded at build time from the `twemoji-assets` crate; they are
not modified.

## Fonts

### Meatshell Mono — bundled monospace terminal font

The terminal's default monospace family, **Meatshell Mono**, is a renamed,
otherwise unmodified build of **Cascadia Mono** (version 2407.024), supplied by
Microsoft.

- Copyright 2021 Microsoft Corporation. Cascadia Code is a trademark of the
  Microsoft group of companies.
- Typeface design: Saja Typeworks (Aaron Bell), <http://sajatypeworks.com>.
- The font is distributed under Microsoft's supplied-font terms together with
  the SIL Open Font License based terms, both of which are embedded in the
  font's own `name` table and remain viewable in any font inspector.
- Upstream licence text:
  <https://github.com/microsoft/cascadia-code/blob/main/LICENSE>

The font file is shipped unmodified; only its family name was changed so the
application can refer to it as its own bundled family.

### MiSans

The proportional interface font **MiSans** is bundled under Xiaomi's *MiSans
Font Intellectual Property License*.

- Copyright © 2020–2025 Beijing Xiaomi Mobile Software Co., Ltd.
- The licence permits free use, including commercial use, redistribution and
  embedding in applications, provided the font files are not sold on their own,
  the copyright notice is kept, and the font is not used to defame or
  misrepresent Xiaomi.
- A summary of the terms is in
  [`assets/fonts/LICENSE-MiSans.txt`](assets/fonts/LICENSE-MiSans.txt); the
  official page (<https://hyperos.mi.com/font/>) governs.

The font files are shipped unmodified.

### HarmonyOS Sans SC

The proportional interface font **HarmonyOS Sans SC** is bundled under the
*HarmonyOS Sans Fonts License Agreement*.

- Copyright 2021 Huawei Device Co., Ltd.
- The agreement permits use, copy, merge, embed, bundle and redistribute of
  unmodified copies together with the software; it requires a prominent notice
  that HarmonyOS Sans Fonts are used, forbids modifications, and forbids
  standalone redistribution.
- The full agreement is in
  [`assets/fonts/LICENSE-HarmonyOS-Sans.txt`](assets/fonts/LICENSE-HarmonyOS-Sans.txt).

The font files are shipped unmodified.

## UI components and icons

### shadcn_flutter

The desktop UI is built with the [shadcn_flutter](https://pub.dev/packages/shadcn_flutter)
Flutter package for its widget set and theme system.

- Licensed under the MIT licence.
- The package also bundles the Lucide icon font used by many of those widgets.

### Lucide

Interface icons come from [Lucide](https://lucide.dev).

- Licensed under the ISC licence.
- Lucide includes icons derived from the Feather project, Copyright (c)
  2013-present Cole Bemis, licensed under the MIT licence.
- Two newer Lucide icons (`fingerprint-pattern` and `clipboard-clock`) are
  embedded as SVG path data in
  [`apps/xenterm/lib/widgets/lucide_extra_icons.dart`](apps/xenterm/lib/widgets/lucide_extra_icons.dart)
  because they post-date the icon set bundled with shadcn_flutter.

### Simple Icons

The Flutter project depends on the [Simple Icons](https://simpleicons.org)
package for brand glyphs.

- The icon data is released under CC0 1.0 Universal.
- Brand names and logos remain trademarks of their respective owners; they are
  used only to identify the corresponding products or services.

### Flutter

The desktop application is built with [Flutter](https://flutter.dev), licensed
under the BSD 3-Clause licence. The Flutter engine and framework notices are
included in the generated application bundle by the Flutter toolchain.

## Rust dependencies

The Rust core's dependency tree and exact versions are pinned in
[`Cargo.lock`](Cargo.lock); each crate declares its own licence in its package
metadata. No Rust dependency source code is vendored into this repository. To
list the resolved licences for a build:

```bash
cargo metadata --format-version 1 --locked
```

## Build and packaging tools

The Windows MSI is built with [WiX Toolset](https://wixtoolset.org) 3.14.1
(the pinned version is installed by `scripts/install-wix.ps1`). WiX Toolset is
licensed under the Microsoft Reciprocal License (MS-RL). It is a build-time tool:
the MSI contains generated installer tables and the standard WixUI dialogs it
produces, not the toolchain's source code.

Flutter and the Rust toolchain are likewise build-time dependencies; their
licences are not re-stated here because neither is redistributed in source form.

## Trademarks

XenTerm is an independent project and is not affiliated with, endorsed by, or
derived from any other terminal product. Product names used in this repository
— including Windows, macOS, Linux, PuTTY, OpenSSH and FinalShell — are the
trademarks of their respective owners and are used only for identification and
interoperability descriptions (for example, importing a FinalShell connection
export).

## Corrections

If you believe attribution is missing or incorrect, please open an issue at
<https://github.com/ixbaicn/XenTerm/issues>.
