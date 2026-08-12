# button

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI plus consumer prop migration; replaced the Radix Slot-based button with the real Base UI Button primitive.

## Changed

- `web/console/src/components/ui/button.tsx:1` now imports `Button` from `@base-ui/react/button`.
- `web/console/src/components/ui/button.tsx:43` accepts Base UI Button props, including `render`, while preserving all local variants and sizes.
- `web/console/src/components/ui/pagination.tsx:53` renders pagination buttons as anchors through `render`.
- `web/console/src/app/router.tsx:150`, `web/console/src/features/auth/login-page.tsx:93`, and `web/console/src/features/auth/activate-invitation-page.tsx:92` replace `asChild` with `render` and set `nativeButton={false}` for router links.
- `web/console/src/components/shared/resource-table.test.tsx` now asserts the Base UI button role exposed by interactive pagination controls.
- Channel token-list tests query their remove controls by accessible button name instead of relying on duplicated text nodes.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/button.tsx` returned no matches.

## Left alone

- Badge and Sidebar polymorphism was handled in their separate migration reports.

## Behavior changes

- Elements rendered through the Base UI Button with `nativeButton={false}` retain their anchor DOM and `href`, but Base UI exposes button semantics (`role="button"`) and button keyboard handling.

## Verify by hand

- Activate normal, disabled, destructive, icon, and submit buttons with pointer and keyboard.
- Follow login, invitation, not-found, and pagination controls and confirm navigation, focus styling, Enter, and Space behavior.
