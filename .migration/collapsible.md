# collapsible

2026-07-23 — golden pair via CLI with a consumer prop migration; migrated Collapsible to Base UI and updated the JSON viewer trigger composition.

## Changed

- `web/console/src/components/ui/collapsible.tsx:1` now imports the Base UI collapsible primitive.
- `web/console/src/components/ui/collapsible.tsx:13` maps the public content wrapper to `Collapsible.Panel`.
- `web/console/src/components/shared/json-viewer.tsx:37` replaces Radix `asChild` with Base UI `render` while retaining the existing Button styling and children.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/collapsible.tsx web/console/src/components/shared/json-viewer.tsx` returned no matches.

## Left alone

- Sidebar's unrelated `collapsible="icon"` layout prop is application state, not the Radix Collapsible primitive, so it was intentionally untouched.

## Behavior changes


## Verify by hand

- Expand and collapse a JSON payload with mouse and keyboard.
- Confirm focus stays on the trigger and the chevron rotation tracks the open state.
