# Third-party license boundary

The root MIT License applies to Toolkit's own source, documentation and
synthetic fixtures. It does not replace dependency or asset licenses.

This repository is a source distribution. It contains no Makepad vendor
checkout, third-party binary, font bundle, captured screenshot or generated
container image. Cargo dependencies and Makepad are obtained separately.

The publication audit checked 98 local third-party package declarations and
all 70 distinct registry packages referenced by the two lockfiles. Registry
expressions were checked in cached manifests or exact-version crates.io
metadata. They use permissive license choices (MIT, Apache-2.0, ISC, Zlib,
Unlicense or 0BSD), with the additional Unicode-3.0 requirement noted below.
No copyleft, proprietary or noncommercial registry declaration was found.
This declaration audit is not approval to redistribute an asset/binary bundle.

## Makepad

The default bootstrap selects public upstream revision
`105158e08c89d47f57af4621420ed68eb3a147ce`. Preserve Makepad's
[own license and copyright notice](https://github.com/makepad/makepad/blob/105158e08c89d47f57af4621420ed68eb3a147ce/LICENSE)
when redistributing its code. Its nested libraries and assets may have
additional terms; their parent directory's MIT notice does not override them.

## Dependency notices for compiled distributions

Review the exact resolved package versions and their source notices before
shipping an executable or container. In particular:

- `ab_glyph_rasterizer` retains Google Apache-2.0 source notices; preserve them
  and supply [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0) even if a
  local copy has a conflicting generic MIT LICENSE file.
- `spirv` and `unicode-linebreak` declare Apache-2.0.
- `foldhash` uses Zlib, `libloading` uses ISC, and `cfg_aliases` has an
  additional NOTICES.md. Preserve their matching notices.
- `unicode-ident` declares `(MIT OR Apache-2.0) AND Unicode-3.0`; preserve the
  [Unicode notice](https://www.unicode.org/license.txt) as well as the selected
  permissive license.

## Fonts and other assets

Fonts in the separately obtained Makepad resources are not Toolkit MIT assets.
IBM Plex, JetBrains Mono, LXGW WenKai, Noto and Font Awesome fonts have
[OFL terms](https://openfontlicense.org/open-font-license-official-text/).
Font Awesome's [versioned license](https://github.com/FortAwesome/Font-Awesome/blob/6.7.2/LICENSE.txt)
also distinguishes fonts, icons and code. Preserve copyright, reserved-name
and full license notices when including them in a distribution.

The inspected legacy Liberation Mono 1.04 and New Computer Modern Math assets
require matching version-specific notices; the latter identifies the GUST
Font License. They are excluded from this repository. Binary or asset bundles
that include these fonts require a separate completed redistribution audit.

Generic demo data and the small synthetic PNG fixture are first-party test
inputs. Local app captures, customer documents, reports and archives are not
public fixtures and are excluded by .gitignore.
