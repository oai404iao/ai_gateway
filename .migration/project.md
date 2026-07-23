# project

2026-07-23 — whole-project golden-pair migration plus consumer sweep; all Console Radix wrappers and direct usages now use Base UI or the documented native replacement.

## Changed

- `web/console/components.json` now uses the `base-nova` shadcn style.
- `web/console/package.json` and `web/console/pnpm-lock.yaml` add `@base-ui/react@1.6.0` and remove the direct `radix-ui` dependency after the final wrapper migration.
- Twenty wrapper reports in `.migration/` record the component-by-component golden-pair, merge, and consumer work for Label, Separator, Toggle, Checkbox, Switch, Avatar, Progress, Collapsible, Tabs, ScrollArea, Tooltip, DropdownMenu, Select, Dialog, Sheet, AlertDialog, Button, Badge, ToggleGroup, and Sidebar.
- The application-wide consumer sweep replaced `asChild`, Radix value shapes, popup anatomy, state hooks, and positioning props. Controlled external dialogs now identify their trigger, clickable tables ignore interactive descendants, and test setup supplies the browser animation API used by Base UI.
- Final dependency and source scans found no `radix-ui` or `@radix-ui` matches in `web/console/package.json`, `web/console/pnpm-lock.yaml`, `web/console/components.json`, `web/console/src`, or `web/console/e2e`.
- Verification passed: TypeScript typecheck, oxlint with 0 errors and the 5 existing Fast Refresh warnings, 76 Vitest tests in 26 files, production build, OpenAPI generated-type drift check, and 5 Playwright Chromium smoke tests.

## Left alone

- `sonner` and `recharts`, including `web/console/src/components/ui/sonner.tsx` and `chart.tsx`, are independent libraries rather than Radix wrappers and were intentionally untouched.
- Backend Rust code, the Console OpenAPI contract, and generated API types did not require contract changes.

## Behavior changes

- Base UI Tabs use manual keyboard activation.
- Base UI ToggleGroup values are arrays and items expose toggle-button semantics.
- Base UI portals and popups add Positioner/wrapper elements and use starting/ending transition state.
- Base UI Checkbox uses a hidden native input and automatic label association.
- Anchors rendered through Base UI Button keep navigation attributes but expose button semantics.

## Verify by hand

- Exercise keyboard and pointer navigation for Select, DropdownMenu, Tabs, ToggleGroup, Checkbox, Switch, and Sidebar.
- Open every Dialog, AlertDialog, and Sheet; close with buttons, Escape, and allowed outside clicks, then confirm focus returns to the opener.
- Check popup placement and collision behavior near each viewport edge, including RTL if enabled later.
- Confirm the Catalog and Channels selection checkboxes do not activate their clickable rows.
- Confirm the login, invitation, not-found, pagination, and sidebar navigation controls retain the intended navigation behavior.
