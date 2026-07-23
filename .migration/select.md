# select

2026-07-23 — golden pair via CLI plus customization replay; migrated Select to Base UI while preserving string-only values and selected-item labels for existing consumers.

## Changed

- `web/console/src/components/ui/select.tsx:17` infers an `items` label map from nested `SelectItem` elements so `SelectValue` continues to show human labels instead of raw stored values.
- `web/console/src/components/ui/select.tsx:41` preserves the project's string-only change contract and filters Base UI's `null` empty value, replacing the former Radix empty-string guard.
- `web/console/src/components/ui/select.tsx:136` introduces `Portal > Positioner > Popup > List`, forwards all exposed positioning props, and replaces `position` with `alignItemWithTrigger`.
- `web/console/src/components/ui/select.tsx:189` uses Base UI's ItemText-first anatomy and rendered ItemIndicator.
- `web/console/src/components/ui/select.test.tsx:14` now pins selected-label rendering and verifies no empty change is emitted.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/select.tsx` returned no matches.

## Left alone

- Existing application Select call sites were unchanged because the wrapper intentionally retains string values and single-argument handlers. No consumer used the removed Radix `position` prop.

## Behavior changes

- Popup positioning now uses Base UI collision handling and defaults to a 4px side offset.
- The wrapper remains single-select/string-specific; Base UI's object-value and multiple-select capabilities are not exposed because the existing repository contract did not support them.

## Verify by hand

- Open representative selects in user, channel, request-log, and statistics forms; confirm selected labels rather than raw enum or ID values appear.
- Verify keyboard navigation, typeahead, scroll arrows, selection, focus return, and sentinel options such as `__none__`.
