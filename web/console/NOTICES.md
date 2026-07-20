# Third-party notices for the ai-gateway Console UI

This file records license attribution for dependencies of `web/console/` that
are not MIT/Apache-2.0/BSD/ISC and may carry redistribution obligations. The
full license inventory can be regenerated with `pnpm licenses list`.

## Bundled into the embedded binary (shipped in `dist/`)

### Geist Variable Font — SIL Open Font License 1.1 (OFL-1.1)

Source: `@fontsource-variable/geist` (npm). The Geist typeface is © Vercel,
licensed under the SIL Open Font License 1.1. The font files are bundled into
`dist/` by Vite and embedded into the Rust binary via `rust-embed`.

The OFL-1.1 permits free use, study, modification, and redistribution provided
the license text and copyright notice are included. The full OFL-1.1 text is
available at <https://openfontlicense.org> and in the
`@fontsource-variable/geist` package (`node_modules/@fontsource-variable/geist/LICENSE`).

When redistributing the binary, retain the OFL-1.1 notice for the Geist font.

## Build-time only (not shipped)

### lightningcss — Mozilla Public License 2.0 (MPL-2.0)

Source: `lightningcss` (npm, a dev dependency used by Vite/Tailwind for CSS
minification at build time). MPL-2.0 is a weak, file-level copyleft license.
`lightningcss` processes stylesheets during the build and is not included in
`dist/` or the embedded binary, so no MPL obligations extend to the shipped
artifact. See <https://www.mozilla.org/en-US/MPL/2.0/>.

## Other permissive licenses in the tree

BlueOak-1.0.0 (`isexe`, `lru-cache`, `minimatch`), Python-2.0 (`argparse`),
CC-BY-4.0 (`caniuse-lite`), CC0-1.0 (`mdn-data`), 0BSD (`tslib`), MIT-0. All are
permissive and compatible with embedding and redistribution.
