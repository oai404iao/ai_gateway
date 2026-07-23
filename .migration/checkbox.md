# checkbox

2026-07-23 — golden pair via CLI plus a state-selector fix; migrated Checkbox to Base UI and preserved the project's Lucide check icon.

## Changed

- `web/console/src/components/ui/checkbox.tsx:1` now imports the Base UI checkbox primitive.
- `web/console/src/components/ui/checkbox.tsx:11` styles disabled state with `data-disabled`, which remains live after the root element changes from a button to a span.
- `web/console/src/components/ui/checkbox.tsx:16` retains the existing indicator composition and Lucide `CheckIcon`.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/checkbox.tsx` returned no matches.

## Left alone

- Checkbox consumers were unchanged because they use boolean checked values and single-argument change handlers compatible with Base UI.

## Behavior changes

- The visible checkbox root is now a `<span>` backed by a hidden native input rather than Radix's button-shaped root. Base UI preserves form and keyboard behavior, but DOM-sensitive CSS or tests should account for the new element.

## Verify by hand

- Toggle a checkbox with pointer and Space key, then verify its check mark and focus ring.
- Confirm disabled, required, invalid, and form-submission behavior on an admin form.
