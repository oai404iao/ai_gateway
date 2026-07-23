# alert-dialog

2026-07-23 — golden pair via CLI plus transition cleanup; migrated AlertDialog to Base UI and preserved the console's customized heading class.

## Changed

- `web/console/src/components/ui/alert-dialog.tsx:2` now imports Base UI AlertDialog.
- `web/console/src/components/ui/alert-dialog.tsx:26` maps the overlay to Backdrop, and the content wrapper maps to Popup.
- `web/console/src/components/ui/alert-dialog.tsx:53` uses Base UI starting/ending transitions.
- `web/console/src/components/ui/alert-dialog.tsx:142` maps Action to a plain Button because Base UI has no Action primitive.
- `web/console/src/components/ui/alert-dialog.tsx:163` maps Cancel to Base UI Close rendered as the existing Button.
- Leftover scan clean: `grep -n "radix-ui\|@radix-ui" web/console/src/components/ui/alert-dialog.tsx` returned no matches.

## Left alone

- `web/console/src/components/shared/confirm-dialog.tsx` required no edit: its current confirm handlers already close controlled dialog state before starting mutations.

## Behavior changes

- `AlertDialogAction` no longer closes automatically; it is a plain Button. Existing consumers explicitly close state, but future consumers must do the same.
- Base UI focuses the first tabbable element by default rather than specifically preferring Cancel. A confirmation dialog containing an input may initially focus that input.

## Verify by hand

- Revoke a session and an API key; confirm Cancel, confirm action, Escape, focus trapping, and focus return.
- In the API-key dialog, verify the initial focus target and reason input behavior are acceptable.
