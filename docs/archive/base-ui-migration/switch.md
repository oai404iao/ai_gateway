# switch

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI; migrated the switch wrapper to Base UI with Base UI presence-state selectors.

## Changed

- `web/console/src/components/ui/switch.tsx:1` now imports `Switch` from `@base-ui/react/switch`.
- `web/console/src/components/ui/switch.tsx:16` uses `data-checked`, `data-unchecked`, and `data-disabled` for track styling.
- `web/console/src/components/ui/switch.tsx:23` uses the same Base UI state hooks to position and color the thumb.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/switch.tsx` returned no matches.

## Left alone

- Switch consumers were unchanged because checked values and existing one-argument `onCheckedChange` handlers remain compatible.

## Behavior changes

- The visible switch root is now a `<span>` backed by a hidden native input rather than a button-shaped Radix root. Form semantics remain managed by Base UI.

## Verify by hand

- Toggle a switch with pointer and Space key and confirm the thumb moves correctly.
- Verify disabled, invalid, focus-ring, and form-submission behavior.
