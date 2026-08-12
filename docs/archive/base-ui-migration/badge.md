# badge

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI plus customization replay; replaced Radix Slot polymorphism with Base UI useRender while preserving status variants.

## Changed

- `web/console/src/components/ui/badge.tsx:1` now uses Base UI `mergeProps` and `useRender`.
- `web/console/src/components/ui/badge.tsx:17` preserves the repository's custom `success`, `warning`, and `info` variants.
- `web/console/src/components/ui/badge.tsx:36` replaces the `asChild` boolean with Base UI's `render` contract while keeping `<span>` as the default element.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/badge.tsx` returned no matches.

## Left alone

- Existing Badge consumers do not use polymorphic rendering, so no call-site edits were needed.

## Behavior changes


## Verify by hand

- Inspect default, secondary, destructive, success, warning, and info badges across tables and detail pages.
- Verify a rendered link badge, if added, receives focus and hover styling through the `render` prop.
