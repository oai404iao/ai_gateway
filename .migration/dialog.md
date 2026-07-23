# dialog

2026-07-23 — golden pair via CLI plus consumer and transition rewrites; migrated Dialog to Base UI while preserving the project's Lucide close control.

## Changed

- `web/console/src/components/ui/dialog.tsx:2` now imports Base UI Dialog.
- `web/console/src/components/ui/dialog.tsx:27` maps Overlay to Backdrop, and `web/console/src/components/ui/dialog.tsx:48` maps Content to the centered Popup.
- `web/console/src/components/ui/dialog.tsx:54` replaces Radix open/closed keyframes with Base UI starting/ending transitions.
- `web/console/src/features/admin/routing/channels/channel-batch-edit-dialog.tsx:311`, `channel-model-picker-dialog.tsx:187`, `model-rule-quick-add-dialog.tsx:331`, and `web/console/src/features/admin/system/session-affinity-card.tsx:733` replace `DialogClose asChild` with `render`.
- Controlled channel batch, channel model-picker, and model-rule quick-add dialogs now pass the opener's `triggerId` to Base UI so dismissal and focus restoration remain associated with the external trigger.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/dialog.tsx` returned no matches.

## Left alone

- Dialog call sites did not use Radix-specific focus or outside-interaction callbacks, so no `onOpenChange` reason handling was added.

## Behavior changes

- Base UI Portal renders a wrapper element and manages dismissal through `onOpenChange` event details rather than separate Radix outside/escape callbacks.
- Controlled dialogs opened without a colocated `DialogTrigger` must provide `triggerId`; the affected repository call sites now do so.

## Verify by hand

- Open each channel, model-rule, and session-affinity dialog; verify focus enters the dialog and returns to the opener.
- Close with the top-right button, Cancel, Escape, and an outside click where allowed.
