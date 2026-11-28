import { useMemo, useState } from "react";
import { useI18n } from "@/app/i18n";
import {
  RemoteEntityCombobox,
  type RemoteEntityOption,
} from "@/components/shared/remote-entity-combobox";
import {
  useAdminApiKey,
  useAdminApiKeys,
  useUser,
  useUsers,
} from "@/features/admin/api";
import { useOwnApiKey, useOwnApiKeys } from "@/features/api-keys/api";
import { useDebouncedValue } from "@/hooks/use-debounced-value";

const REMOTE_SEARCH_LIMIT = 20;

interface UserFilterComboboxProps {
  id: string;
  ariaLabel?: string;
  value: string;
  onValueChange: (userId: string) => void;
}

export function UserFilterCombobox({
  id,
  ariaLabel,
  value,
  onValueChange,
}: UserFilterComboboxProps) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const debouncedSearch = useDebouncedValue(search, 300);

  const users = useUsers(
    {
      q: debouncedSearch.trim() || undefined,
      limit: REMOTE_SEARCH_LIMIT,
    },
    { enabled: open },
  );
  const selectedUser = useUser(value);

  const options = useMemo<RemoteEntityOption[]>(
    () =>
      (users.data?.items ?? []).map((user) => ({
        id: user.id,
        label: user.email ? `${user.display_name} · ${user.email}` : user.display_name,
      })),
    [users.data?.items],
  );

  const selectedUserRecord = selectedUser.data?.data;
  const selectedLabel = selectedUserRecord
    ? selectedUserRecord.email
      ? `${selectedUserRecord.display_name} · ${selectedUserRecord.email}`
      : selectedUserRecord.display_name
    : undefined;

  return (
    <RemoteEntityCombobox
      id={id}
      ariaLabel={ariaLabel}
      allLabel={t("All users")}
      value={value}
      selectedLabel={selectedLabel}
      onValueChange={onValueChange}
      options={options}
      isLoading={users.isLoading}
      isError={users.isError}
      open={open}
      onOpenChange={setOpen}
      search={search}
      onSearchChange={setSearch}
    />
  );
}

interface ApiKeyFilterComboboxProps {
  id: string;
  ariaLabel?: string;
  scope: "own" | "system";
  userId?: string;
  value: string;
  onValueChange: (apiKeyId: string) => void;
}

export function ApiKeyFilterCombobox({
  id,
  ariaLabel,
  scope,
  userId,
  value,
  onValueChange,
}: ApiKeyFilterComboboxProps) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");
  const debouncedSearch = useDebouncedValue(search, 300);
  const trimmed = debouncedSearch.trim() || undefined;
  const isSystem = scope === "system";

  const adminKeys = useAdminApiKeys(
    {
      q: trimmed,
      user_id: userId || undefined,
      limit: REMOTE_SEARCH_LIMIT,
    },
    { enabled: isSystem && open },
  );
  const ownKeys = useOwnApiKeys(
    {
      q: trimmed,
      limit: REMOTE_SEARCH_LIMIT,
    },
    { enabled: !isSystem && open },
  );
  const selectedAdminKey = useAdminApiKey(isSystem ? value : "");
  const selectedOwnKey = useOwnApiKey(!isSystem ? value : "");

  const options = useMemo<RemoteEntityOption[]>(() => {
    const items = isSystem ? (adminKeys.data?.items ?? []) : (ownKeys.data?.items ?? []);
    return items.map((key) => ({ id: key.id, label: key.name }));
  }, [adminKeys.data?.items, isSystem, ownKeys.data?.items]);

  const selectedLabel = isSystem
    ? selectedAdminKey.data?.data.name
    : selectedOwnKey.data?.data.name;

  return (
    <RemoteEntityCombobox
      id={id}
      ariaLabel={ariaLabel}
      allLabel={t("All API keys")}
      value={value}
      selectedLabel={selectedLabel}
      onValueChange={onValueChange}
      options={options}
      isLoading={isSystem ? adminKeys.isLoading : ownKeys.isLoading}
      isError={isSystem ? adminKeys.isError : ownKeys.isError}
      open={open}
      onOpenChange={setOpen}
      search={search}
      onSearchChange={setSearch}
    />
  );
}
