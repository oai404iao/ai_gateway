import { z } from "zod";
import { USER_STATUSES } from "@/lib/permissions";

export const accountSchema = z.object({
  display_name: z.string().min(1, "Display name is required.").max(200),
  email: z.union([z.literal(""), z.string().email("Enter a valid email.")]),
  role: z.enum(["user", "admin"]),
  user_group_id: z.string().min(1, "Pick a user group."),
  default_api_key_policy_id: z.string(),
});

export const balanceSchema = z.object({
  balance_amount: z.string().regex(/^-?\d+(?:\.\d+)?$/, "Enter a valid balance."),
});

export const statusSchema = z.object({
  status: z.enum(["active", "suspended", "disabled"]),
});

export type AccountValues = z.infer<typeof accountSchema>;
export type BalanceValues = z.infer<typeof balanceSchema>;
export type StatusValues = z.infer<typeof statusSchema>;
export type ManageableStatus = StatusValues["status"];
export type SubmittingAction = "account" | "balance" | "status" | "settings";

export const manageableStatuses = USER_STATUSES.filter(
  (status): status is ManageableStatus => status !== "invited",
);

export const emptyAccountValues: AccountValues = {
  display_name: "",
  email: "",
  role: "user",
  user_group_id: "",
  default_api_key_policy_id: "",
};

export const emptyBalanceValues: BalanceValues = {
  balance_amount: "0",
};

export const emptyStatusValues: StatusValues = {
  status: "active",
};

export function isManageableStatus(value: string): value is ManageableStatus {
  return manageableStatuses.some((status) => status === value);
}

