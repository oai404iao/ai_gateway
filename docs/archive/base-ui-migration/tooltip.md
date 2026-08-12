# tooltip

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI plus consumer and animation rewrites; migrated Tooltip to Base UI while preserving the console's zero-delay provider default.

## Changed

- `web/console/src/components/ui/tooltip.tsx:6` maps the former `delayDuration={0}` default to Base UI's `delay={0}`.
- `web/console/src/components/ui/tooltip.tsx:41` introduces the required `Portal > Positioner > Popup` structure and forwards all exposed positioning props to the Positioner.
- `web/console/src/components/ui/tooltip.tsx:51` replaces Radix state/keyframe hooks with Base UI starting/ending transition hooks.
- `web/console/src/components/shared/api-key-value.tsx:56` and `web/console/src/components/shared/api-key-value.tsx:72` replace `asChild` with `render` for API-key action buttons.
- `web/console/src/components/ui/sidebar.tsx:527` updates the sidebar's tooltip trigger consumer to `render`; the sidebar primitive itself remains scheduled for its own migration.
- Leftover scan clean for the migrated wrapper: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/tooltip.tsx` returned no matches.

## Left alone

- Other polymorphic `asChild` uses in `web/console/src/components/ui/sidebar.tsx` belong to the sidebar wrapper and were intentionally deferred to the sibling `sidebar.md` migration record.

## Behavior changes

- Tooltip content now defaults to a 4px side offset instead of the former 0px wrapper default, matching the Base UI shadcn variant.
- Positioning and enter/exit animation are handled by Base UI's Positioner and transition-state attributes.

## Verify by hand

- Hover and keyboard-focus both API-key action buttons and confirm tooltips appear immediately.
- Collapse the sidebar and verify menu tooltips appear on the right, track their trigger, and dismiss cleanly.
