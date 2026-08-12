# toggle

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI plus a class-hook cleanup; migrated the toggle primitive to Base UI while preserving the shared variant export.

## Changed

- `web/console/src/components/ui/toggle.tsx:1` now imports `Toggle` from `@base-ui/react/toggle`.
- `web/console/src/components/ui/toggle.tsx:7` uses Base UI's `data-pressed` state hook instead of Radix `data-state="on"`.
- `web/console/src/components/ui/toggle.tsx:43` retains the lint annotation required by the shared `toggleVariants` export used by ToggleGroup.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/toggle.tsx` returned no matches.

## Left alone

- `web/console/src/components/ui/toggle-group.tsx` still consumes `toggleVariants`; its primitive migration is tracked separately.

## Behavior changes


## Verify by hand

- Toggle an item on and off and confirm its pressed background appears.
- Tab to the toggle and verify focus-ring, disabled, and keyboard activation behavior.
