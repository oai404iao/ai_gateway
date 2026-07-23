# sidebar

2026-07-23 — golden pair via CLI plus consumer and state-hook cleanup; removed the final Sidebar Slot usages in favor of Base UI useRender.

## Changed

- `web/console/src/components/ui/sidebar.tsx:4` now imports Base UI `mergeProps` and `useRender` instead of Radix Slot.
- `web/console/src/components/ui/sidebar.tsx:393`, `web/console/src/components/ui/sidebar.tsx:499`, and `web/console/src/components/ui/sidebar.tsx:666` migrate polymorphic group labels, menu buttons, actions, and sub-buttons to `render`.
- `web/console/src/components/ui/sidebar.tsx:478` uses Base UI's `data-popup-open` hook for popup-trigger styling.
- `web/console/src/components/ui/sidebar.tsx:521` composes tooltip triggers through `render` without nesting interactive elements.
- `web/console/src/app/layouts/console-layout.tsx:156` renders navigation links through `SidebarMenuButton.render`.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/sidebar.tsx` returned no matches.

## Left alone

- Sidebar collapse state attributes such as `data-state="collapsed"` are application-owned layout state, not Radix primitive state, and were intentionally retained.

## Behavior changes


## Verify by hand

- Navigate through every sidebar link and verify active styling, focus rings, and anchor semantics.
- Collapse and expand with the rail, trigger button, and Ctrl/Cmd+B.
- On mobile, open and close the sheet-backed sidebar; on desktop, confirm collapsed-menu tooltips.
