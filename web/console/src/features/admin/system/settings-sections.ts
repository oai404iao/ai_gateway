export const transportSections = [
  { id: "general", label: "API hosts" },
  { id: "upstream", label: "Upstream timeouts" },
  { id: "websocket", label: "Responses WebSocket" },
] as const;

export const healthSections = [
  { id: "reliability", label: "Retries and health" },
  { id: "testing", label: "Scheduled channel tests" },
  { id: "affinity", label: "Session affinity" },
] as const;

export const maintenance = { id: "maintenance", label: "Runtime maintenance" } as const;
export const SETTINGS_SECTIONS = [...transportSections, ...healthSections, maintenance];
export type SettingsSection = (typeof SETTINGS_SECTIONS)[number]["id"];
