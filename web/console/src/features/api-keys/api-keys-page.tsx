import { useState } from "react";
import { useNavigate } from "react-router";
import { useForm } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import { Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Field, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { PageHeader } from "@/components/shared/page-header";
import { AsyncResource } from "@/components/shared/async-resource";
import { ResourceTable, type Column } from "@/components/shared/resource-table";
import { SecretOnceDialog } from "@/components/shared/secret-once-dialog";
import { StatusBadge } from "@/components/shared/status-badge";
import { useCreateOwnApiKey, useOwnApiKeys } from "@/features/api-keys/api";
import type { ApiKeyView } from "@/api/types";
import { formatExpiry, formatRelative } from "@/lib/dates";
import { formatList } from "@/lib/formatters";
import { apiFormatLabel } from "@/lib/permissions";

const createSchema = z.object({
  name: z.string().min(1, "Name is required.").max(100),
  expires_at: z.string().optional(),
});

type CreateValues = z.infer<typeof createSchema>;

export function ApiKeysPage() {
  const navigate = useNavigate();
  const { data: keys, isLoading, error } = useOwnApiKeys();
  const create = useCreateOwnApiKey();
  const [createOpen, setCreateOpen] = useState(false);
  const [secret, setSecret] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const form = useForm<CreateValues>({ resolver: zodResolver(createSchema) });

  const onSubmit = async (values: CreateValues) => {
    setSubmitting(true);
    try {
      const result = await create.mutateAsync({
        name: values.name,
        expires_at: values.expires_at ? values.expires_at : null,
      });
      if (result.secret) setSecret(result.secret);
      setCreateOpen(false);
      form.reset();
      toast.success("API key created");
    } catch (error) {
      toast.error(error instanceof Error ? error.message : "Create failed");
    } finally {
      setSubmitting(false);
    }
  };

  const columns: Column<ApiKeyView>[] = [
    { key: "name", header: "Name", render: (key) => <span className="font-medium">{key.name}</span> },
    {
      key: "status",
      header: "Status",
      render: (key) => <StatusBadge value={key.status} />,
    },
    {
      key: "formats",
      header: "Formats",
      render: (key) => formatList(key.allowed_api_formats.map(apiFormatLabel)),
    },
    {
      key: "permissions",
      header: "Permissions",
      render: (key) => formatList(key.permissions),
    },
    {
      key: "expiry",
      header: "Expires",
      render: (key) => formatExpiry(key.expires_at),
    },
    {
      key: "updated",
      header: "Updated",
      render: (key) => formatRelative(key.updated_at),
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        title="API Keys"
        description="Your personal client keys for the OpenAI-compatible data plane."
        actions={
          <Button onClick={() => setCreateOpen(true)}>
            <Plus data-icon="inline-start" /> New API key
          </Button>
        }
      />
      <Card>
        <CardHeader>
          <CardTitle>Keys</CardTitle>
          <CardDescription>
            Permissions, formats, and limits are set by your assigned API key
            policy and cannot be raised from this page.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <AsyncResource
            isLoading={isLoading}
            error={error}
            isEmpty={keys?.length === 0}
            emptyTitle="No API keys"
            emptyDescription="Create your first API key to start using the data plane."
          >
            <ResourceTable
              columns={columns}
              rows={keys ?? []}
              rowKey={(key) => key.id}
              onRowClick={(key) => navigate(`/api-keys/${key.id}`)}
            />
          </AsyncResource>
        </CardContent>
      </Card>

      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>New API key</DialogTitle>
            <DialogDescription>
              Choose a name and optional expiry. Authorization fields are
              assigned from your default policy.
            </DialogDescription>
          </DialogHeader>
          <form onSubmit={form.handleSubmit(onSubmit)} className="flex flex-col gap-4">
            <FieldGroup>
              <Field>
                <FieldLabel htmlFor="name">Name</FieldLabel>
                <Input id="name" {...form.register("name")} />
                {form.formState.errors.name ? (
                  <FieldError>{form.formState.errors.name.message}</FieldError>
                ) : null}
              </Field>
              <Field>
                <FieldLabel htmlFor="expires_at">Expires at (optional)</FieldLabel>
                <Input
                  id="expires_at"
                  type="datetime-local"
                  {...form.register("expires_at")}
                />
              </Field>
            </FieldGroup>
            <DialogFooter>
              <Button type="submit" disabled={submitting}>
                {submitting ? <Spinner data-icon="inline-start" /> : null}
                Create key
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <SecretOnceDialog
        open={Boolean(secret)}
        onOpenChange={(open) => !open && setSecret(null)}
        title="Your new API key"
        description="Use this as the Bearer token for /v1/* requests."
        secret={secret}
      />
    </div>
  );
}
