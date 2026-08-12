# dropdown-menu

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI plus consumer and transition rewrites; migrated DropdownMenu to Base UI Menu while preserving the project's Lucide icons and visual treatment.

## Changed

- `web/console/src/components/ui/dropdown-menu.tsx:2` now uses `@base-ui/react/menu`.
- `web/console/src/components/ui/dropdown-menu.tsx:33` introduces `Portal > Positioner > Popup` and forwards align/side offsets to the Positioner.
- `web/console/src/components/ui/dropdown-menu.tsx:117` uses `data-popup-open` for submenu trigger state.
- `web/console/src/components/ui/dropdown-menu.tsx:176` and `web/console/src/components/ui/dropdown-menu.tsx:216` use the split Base UI checkbox/radio indicators.
- `web/console/src/components/shared/locale-toggle.tsx:18` and `web/console/src/app/layouts/console-layout.tsx:56` replace trigger `asChild` composition with `render`.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/dropdown-menu.tsx` returned no matches.

## Left alone

- `web/console/src/app/layouts/console-layout.tsx:156` still has `SidebarMenuButton asChild`; that belongs to the separate sidebar migration.

## Behavior changes

- Base UI checkbox and radio menu items default to staying open after selection. Theme and locale radio menus therefore remain open until dismissed, unlike Radix's default close-on-select behavior. This is intentionally flagged rather than silently overridden.

## Verify by hand

- Open theme, locale, and user menus with pointer and keyboard; verify focus, arrow navigation, and typeahead.
- Select theme and locale radio items, observe the intentional stay-open behavior, then dismiss with Escape or outside click.
- Confirm Profile and Sign out items still close the menu when clicked.
