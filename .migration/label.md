# label

2026-07-23 — golden pair via CLI; migrated the Radix label wrapper to a native label without changing its public styling contract.

## Changed

- `web/console/src/components/ui/label.tsx:5` now renders a native `<label>` and uses native label props.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/label.tsx` returned no matches.

## Left alone

- `web/console/src/components/ui/field.tsx` still consumes the public `Label` wrapper; no call-site change was needed because `htmlFor`, children, and DOM label props remain compatible.

## Behavior changes


## Verify by hand

- Open a form, click a field label, and confirm focus moves to the associated input.
- Confirm disabled field labels retain the expected cursor and opacity styling.
