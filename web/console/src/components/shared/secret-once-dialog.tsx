import { useState } from "react";
import { Copy, Check, ShieldAlert } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

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
          <DialogTitle>{title}</DialogTitle>
          {description ? <DialogDescription>{description}</DialogDescription> : null}
        </DialogHeader>
        <Alert variant="destructive">
          <ShieldAlert data-icon="inline-start" />
          <AlertTitle>Save it now</AlertTitle>
          <AlertDescription>
            This value is shown only once. The gateway does not store it in a
            retrievable form, and the console will not display it again.
          </AlertDescription>
        </Alert>
        <div className="flex items-center gap-2 rounded-md border bg-muted/40 p-3">
          <code className="flex-1 break-all font-mono text-sm">{secret ?? "—"}</code>
          <Button size="sm" variant="outline" onClick={copy} disabled={!secret}>
            {copied ? <Check data-icon="inline-start" /> : <Copy data-icon="inline-start" />}
            {copied ? "Copied" : "Copy"}
          </Button>
        </div>
        <div className="flex justify-end">
          <Button onClick={() => onOpenChange(false)}>I have saved it</Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
