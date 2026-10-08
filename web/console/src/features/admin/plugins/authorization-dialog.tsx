import { useState } from "react";
import { useI18n } from "@/app/i18n";
import { ErrorAlert } from "@/components/shared/async-resource";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { authorizePluginManagement } from "./api";

export type PluginAction = { title: string; run: (token: string) => Promise<unknown> };

export function PluginAuthorizationDialog({ action, onClose }: {
  action: PluginAction | null;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [password, setPassword] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const close = () => {
    if (pending) return;
    setPassword("");
    setError(null);
    onClose();
  };
  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!action || pending) return;
    setPending(true);
    setError(null);
    try {
      const authorization = await authorizePluginManagement(password);
      setPassword("");
      await action.run(authorization.token);
      onClose();
    } catch (failure) {
      setPassword("");
      setError(failure);
    } finally {
      setPending(false);
    }
  };
  return (
    <Dialog open={action !== null} onOpenChange={(open) => { if (!open) close(); }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("Authorize plugin operation")}</DialogTitle>
          <DialogDescription>
            {t("Native plugins execute with gateway privileges. Confirm your administrator password to continue.")}
          </DialogDescription>
        </DialogHeader>
        <form onSubmit={(event) => void submit(event)} className="flex flex-col gap-4">
          {error ? <ErrorAlert error={error} /> : null}
          <FieldGroup>
            <Field>
              <FieldLabel htmlFor="plugin-password">{t("Current password")}</FieldLabel>
              <Input id="plugin-password" type="password" autoComplete="current-password"
                value={password} onChange={(event) => setPassword(event.target.value)}
                required disabled={pending} />
            </Field>
          </FieldGroup>
          <DialogFooter>
            <Button type="button" variant="outline" disabled={pending} onClick={close}>{t("Cancel")}</Button>
            <Button type="submit" disabled={pending || !password}>
              {pending ? <Spinner data-icon="inline-start" /> : null}
              {t(action?.title ?? "Confirm")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
