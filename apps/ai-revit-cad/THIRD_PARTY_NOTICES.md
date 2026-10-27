# Third-party notices

The application's own code is offered under the MIT license in [LICENSE](LICENSE).
Third-party code and resources retain their original copyright notices and
license terms. The application MIT license does not relicense those materials.

## CADCraft DXF

The minimal [cadcraft-dxf source package](third_party/cadcraft-dxf/) is from
[CADCraft](https://github.com/storytold/cadcraft), revision
`43e33add44ffcf386f7ca0f06cfe1af7acae7256` (version 0.4.0).

Copyright (c) 2026 ArtCraft Team and the CADCraft contributors. The package is
available under the [MIT license](third_party/cadcraft-dxf/LICENSE-MIT) or the
[Apache License 2.0](third_party/cadcraft-dxf/LICENSE-APACHE), at your option.
Its [NOTICE](third_party/cadcraft-dxf/NOTICE) records exact source hashes and
the standalone manifest adaptation. The implementation source is unchanged.
The application uses this package as a DXF tag parser/writer.

The vendored package excludes unrelated CADCraft crates, DWG/acadrust, brand
assets, fonts, example drawings and research artifacts. The upstream author
and project names above are plain-text attribution and do not imply endorsement.

## Viewer-derived code and repository code

The application preserves attribution for Viewer-derived code in the relevant
source files. Project geometry is not covered by those source-code attributions.
Repository-owned Toolkit code retains the repository's MIT license; the public
application dependency graph does not include the operator-only Toolkit Windows
automation crate or rudra-runtime.

## Makepad SDK and resources obtained separately

The Makepad SDK is an external build input, obtained separately. It is not
vendored in this application directory. The SDK and its bundled dependencies
and resources retain their own licenses; a single application MIT file does
not cover them. In particular, font resources may use SIL Open Font License,
GUST Font License or other terms. Obtain and retain the license texts and
notices for the exact SDK revision and resources actually distributed.

This source package contains no bundled SDK font assets, FH-87 project meshes
or BRep geometry, ArtCraft logos, or compiled application binaries. Binary
packaging and its exact SDK/resource license bundle require a separate review.

## Cargo registry dependencies

Cargo registry dependencies retain their individual upstream licenses and are
not copied into this source package or collectively relicensed MIT. The
[registry license inventory](licenses/registry-dependency-licenses.json) binds
the reviewed source dependency snapshot to the SHA-256 of Cargo.lock and lists
exact package versions, declared licenses and the available original license
files. The newly resolved libc, smallvec and unicode-ident versions were
checked against their original MIT/Apache texts; unicode-ident additionally
requires its Unicode license notice.

Some downloaded packages have a permissive license declaration without a full
local license document. The inventory records that limit explicitly. It does
not clear the separately obtained SDK, font resources or a binary distribution,
and does not apply to a changed dependency lock. Preserve original upstream
license and notice files for any third-party code actually redistributed.
