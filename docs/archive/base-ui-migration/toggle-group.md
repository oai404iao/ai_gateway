# toggle-group

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI plus consumer value-shape migration; migrated ToggleGroup to Base UI and reused the migrated Toggle primitive for items.

## Changed

- `web/console/src/components/ui/toggle-group.tsx:4` imports Base UI Toggle and ToggleGroup.
- `web/console/src/components/ui/toggle-group.tsx:23` renders the callable Base UI group, while `web/console/src/components/ui/toggle-group.tsx:59` renders each item as a Toggle.
- `web/console/src/features/statistics/channel-group-status-panel.tsx` and `web/console/src/features/statistics/cost-statistics-panel.tsx` migrate single selections to Base UI's always-array value shape.
- `web/console/src/features/admin/system/session-affinity-card.tsx:547` replaces `type="multiple"` with the `multiple` boolean.
- `web/console/src/features/statistics/statistics-page.test.tsx` asserts Base UI toggle buttons through `aria-pressed` instead of Radix radio roles.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/toggle-group.tsx` returned no matches.

## Left alone

- No consumer used Radix's `rovingFocus`, `loop`, or direction props, so no additional call-site changes were required.

## Behavior changes

- ToggleGroup values and callbacks are arrays in both single and multiple modes. Existing controlled single selections continue to ignore an empty array, preserving their non-empty UI behavior.
- ToggleGroup items expose toggle-button semantics (`button` with `aria-pressed`) rather than Radix single-group radio semantics.

## Verify by hand

- Change status windows, quick ranges, and cost granularity with pointer and arrow keys.
- Select and clear multiple API formats in the session-affinity rule editor.
