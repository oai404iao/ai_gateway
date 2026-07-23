# button

2026-07-23 — golden pair via CLI plus consumer prop migration; replaced the Radix Slot-based button with the real Base UI Button primitive.

## Changed

- `web/console/src/components/ui/button.tsx:1` now imports `Button` from `@base-ui/react/button`.
- `web/console/src/components/ui/button.tsx:43` accepts Base UI Button props, including `render`, while preserving all local variants and sizes.
- `web/console/src/components/ui/pagination.tsx:53` renders pagination buttons as anchors through `render`.
- `web/console/src/app/router.tsx:150`, `web/console/src/features/auth/login-page.tsx:93`, and `web/console/src/features/auth/activate-invitation-page.tsx:92` replace `asChild` with `render` and set `nativeButton={false}` for router links.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/button.tsx` returned no matches.

## Left alone

- Slot-based polymorphism still present in Badge and Sidebar belongs to their separate migrations.

## Behavior changes


## Verify by hand

- Activate normal, disabled, destructive, icon, and submit buttons with pointer and keyboard.
- Follow login, invitation, not-found, and pagination links and confirm they remain anchors with correct navigation and focus styling.
