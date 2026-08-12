# scroll-area

> 状态：历史归档。记录 2026-07-23 Console Radix → Base UI 迁移的组件级差异；当前实现以 `web/console/` 为准。

2026-07-23 — golden pair via CLI plus a strict-TypeScript cleanup; migrated ScrollArea to Base UI and restored a clean typecheck.

## Changed

- `web/console/src/components/ui/scroll-area.tsx:1` now imports the Base UI scroll-area primitive.
- `web/console/src/components/ui/scroll-area.tsx:16` uses the Base UI viewport part.
- `web/console/src/components/ui/scroll-area.tsx:34` maps the scrollbar and thumb to Base UI's renamed parts.
- Removed the registry-generated unused React import because this project enforces `noUnusedLocals`.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/scroll-area.tsx` returned no matches.

## Left alone

- Existing ScrollArea consumers use vertical scrolling only and do not pass Radix-only `type` or `scrollHideDelay` props, so no call-site edits were needed.

## Behavior changes

- Scrollbar visibility is now driven by Base UI overflow, hover, and scrolling state rather than Radix's `type` setting. The repository did not configure a non-default type.

## Verify by hand

- Scroll the model-picker and quick-add lists with mouse wheel, touchpad, and keyboard.
- Confirm the thumb appears, drags correctly, and nested JSON content remains scrollable.
