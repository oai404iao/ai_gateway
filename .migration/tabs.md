# tabs

2026-07-23 — golden pair via CLI; migrated Tabs to Base UI's Root/List/Tab/Panel parts.

## Changed

- `web/console/src/components/ui/tabs.tsx:1` now imports the Base UI tabs primitive.
- `web/console/src/components/ui/tabs.tsx:50` maps `TabsTrigger` to `Tabs.Tab` and styles active state with `data-active`.
- `web/console/src/components/ui/tabs.tsx:70` maps `TabsContent` to `Tabs.Panel`.
- `web/console/src/features/admin/transforms/transform-document-editor.test.tsx` asserts Base UI's `data-active` state hook instead of Radix's `data-state="active"`.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/tabs.tsx` returned no matches.

## Left alone

- Existing statistics and transform-editor consumers use string values without Radix-only props, so their call sites were unchanged.

## Behavior changes

- Base UI defaults to manual tab activation: arrow keys move focus and Enter/Space activates the focused tab. Radix defaulted to automatic activation while focus moved. This migration intentionally follows the Base UI registry behavior.

## Verify by hand

- On the statistics page, click each tab and confirm the correct panel appears.
- With keyboard only, move focus between tabs with arrow keys, then activate with Enter or Space.
