# progress

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI; migrated Progress to Base UI's Root/Track/Indicator anatomy.

## Changed

- `web/console/src/components/ui/progress.tsx:1` now imports the Base UI progress primitive.
- `web/console/src/components/ui/progress.tsx:11` renders a Base UI root and forwards the current value.
- `web/console/src/components/ui/progress.tsx:18` adds the required track around the indicator; Base UI now computes indicator fill instead of a manual transform.
- `web/console/src/components/ui/progress.tsx:48` exposes Base UI label and value wrappers for accessible composed progress displays.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/progress.tsx` returned no matches.

## Left alone

- `web/console/src/features/statistics/system-load-panel.tsx` continues to use the simple `<Progress value={...} />` API and required no change.

## Behavior changes

- Progress fill is now sized by Base UI's indicator logic rather than a wrapper-authored `translateX` transform.

## Verify by hand

- Open system statistics and verify 0%, partial, and complete progress values render at the expected widths.
- Confirm indeterminate and accessible value text behavior if those states are introduced.
