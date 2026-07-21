import { useState } from "react";
import { Copy, Check, ShieldAlert } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from "@/components/ui/input-group";
import { useI18n } from "@/app/i18n";

interface SecretOnceDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  secret: string | null;
}

/**
 * Displays a one-time secret (new API key, invitation token) with copy
 * support and an explicit warning that it will not be shown again. The secret
 * is never persisted by the SPA; closing clears the local copy.
 */
export function SecretOnceDialog({
  open,
  onOpenChange,
  title,
  description,
  secret,
}: SecretOnceDialogProps) {
  const [copied, setCopied] = useState(false);
  const { t } = useI18n();

  const copy = async () => {
    if (!secret) return;
    await navigator.clipboard.writeText(secret);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 2000);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setCopied(false);
        onOpenChange(next);
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t(title)}</DialogTitle>
          {description ? <DialogDescription>{t(description)}</DialogDescription> : null}
        </DialogHeader>
        <Alert variant="destructive">
          <ShieldAlert data-icon="inline-start" />
          <AlertTitle>{t("Save it now")}</AlertTitle>
          <AlertDescription>
            {t(
              "This value is shown only once. The gateway does not store it in a retrievable form, and the console will not display it again.",
            )}
          </AlertDescription>
        </Alert>
        <InputGroup>
          <InputGroupInput
            readOnly
            value={secret ?? ""}
            placeholder="—"
            className="font-mono text-xs"
            aria-label={t(title)}
          />
          <InputGroupAddon align="inline-end">
            <InputGroupButton onClick={copy} disabled={!secret}>
              {copied ? <Check data-icon="inline-start" /> : <Copy data-icon="inline-start" />}
              {copied ? t("Copied") : t("Copy")}
            </InputGroupButton>
          </InputGroupAddon>
        </InputGroup>
        <DialogFooter>
          <Button onClick={() => onOpenChange(false)}>{t("I have saved it")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
