# Third-party notices for the ai-gateway Console UI

This file records Console-specific attribution notes. Every third-party
dependency, including MIT/Apache-2.0/BSD/ISC packages, has redistribution
conditions. Release archives and container images therefore include generated
`THIRD_PARTY_NOTICES.md` plus the complete per-package `LICENSES/` tree.

## Bundled into the embedded binary (shipped in `dist/`)

### Geist Variable Font — SIL Open Font License 1.1 (OFL-1.1)

Source: `@fontsource-variable/geist` (npm). The Geist typeface is © Vercel,
licensed under the SIL Open Font License 1.1. The font files are bundled into
`dist/` by Vite and embedded into the Rust binary via `rust-embed`.

The OFL-1.1 permits free use, study, modification, and redistribution provided
the license text and copyright notice are included. The exact license and
font copyright notice shipped by the dependency are committed in the
repository as `LICENSES/OFL-1.1.txt` and included with binary distributions.

When redistributing the binary, retain this notice and the committed OFL-1.1
text for the Geist font.

## Build-time only (not shipped)

### lightningcss — Mozilla Public License 2.0 (MPL-2.0)

Source: `lightningcss` (npm, a dev dependency used by Vite/Tailwind for CSS
minification at build time). MPL-2.0 is a weak, file-level copyleft license.
`lightningcss` processes stylesheets during the build and is not included in
`dist/` or the embedded binary, so no MPL obligations extend to the shipped
artifact. See <https://www.mozilla.org/en-US/MPL/2.0/>.

## UI component and icon provenance

The source files in `src/components/ui/` were initially added through the
shadcn/ui CLI and are maintained as project source. Their upstream MIT
attribution is preserved in the generated third-party materials.

`public/favicon.svg` and the custom symbols in `public/icons.svg` are
project-owned artwork. Brand names and marks depicted by social-media symbols
remain the property of their respective owners; use is nominative and does not
grant trademark rights.
