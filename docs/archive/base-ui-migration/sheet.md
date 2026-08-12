# sheet

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI; migrated Sheet from the Radix Dialog namespace to Base UI Dialog while preserving side-specific slide behavior.

## Changed

- `web/console/src/components/ui/sheet.tsx:2` now imports Base UI Dialog as the Sheet primitive family.
- `web/console/src/components/ui/sheet.tsx:24` maps the overlay to Base UI Backdrop.
- `web/console/src/components/ui/sheet.tsx:54` uses Base UI starting/ending style hooks for each sheet side.
- `web/console/src/components/ui/sheet.tsx:63` composes the close control through `render` and retains the Lucide close icon.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/sheet.tsx` returned no matches.

## Left alone

- Existing request-log and audit-log sheet consumers use the root/content API without Radix-only props, so no call-site edits were needed.

## Behavior changes

- Sheet dismissal and focus lifecycle now follow Base UI Dialog semantics and event-detail callbacks.

## Verify by hand

- Open request-log and audit-log sheets, then close them with the close button, Escape, and outside click.
- Confirm right-side entry/exit motion, focus trapping, scroll locking, and focus return.
