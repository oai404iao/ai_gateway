# separator

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI; migrated the separator wrapper to Base UI and removed the unsupported Radix-only `decorative` prop.

## Changed

- `web/console/src/components/ui/separator.tsx:5` now accepts `SeparatorPrimitive.Props` and renders the callable Base UI separator.
- `web/console/src/components/ui/separator.tsx:11` keeps orientation styling through Base UI's `data-horizontal` and `data-vertical` hooks.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/separator.tsx` returned no matches.

## Left alone

- Existing separator consumers were unchanged because none passed the removed `decorative` prop.

## Behavior changes

- Base UI separators are semantic (`role="separator"`). The former wrapper defaulted Radix separators to decorative, so assistive technologies may now announce these separators.

## Verify by hand

- Check horizontal separators in detail pages and vertical separators in compact layouts.
- Inspect one separator with browser accessibility tools and confirm its semantic role is acceptable in context.
