export type SessionDeviceKind = "desktop" | "mobile" | "tablet" | "terminal" | "unknown";

export interface SessionDevice {
  label: string;
  kind: SessionDeviceKind;
}

function versionLabel(
  userAgent: string,
  expression: RegExp,
  name: string,
): string | null {
  const match = userAgent.match(expression);
  return match ? `${name} ${match[1]}` : null;
}

function browserLabel(userAgent: string): string | null {
  return (
    versionLabel(userAgent, /\bEdg(?:A|iOS)?\/(\d+)/, "Edge") ??
    versionLabel(userAgent, /\bOPR\/(\d+)/, "Opera") ??
    versionLabel(userAgent, /\bCriOS\/(\d+)/, "Chrome") ??
    versionLabel(userAgent, /\bChrome\/(\d+)/, "Chrome") ??
    versionLabel(userAgent, /\bFxiOS\/(\d+)/, "Firefox") ??
    versionLabel(userAgent, /\bFirefox\/(\d+)/, "Firefox") ??
    versionLabel(userAgent, /\bVersion\/(\d+).*\bSafari\//, "Safari") ??
    versionLabel(userAgent, /\bElectron\/(\d+)/, "Electron") ??
    versionLabel(userAgent, /\bPostmanRuntime\/(\d+)/, "Postman") ??
    versionLabel(userAgent, /\bcurl\/(\d+)/, "curl")
  );
}

function platformLabel(userAgent: string): string | null {
  if (/\biPad\b/.test(userAgent)) return "iPadOS";
  if (/\biPhone\b/.test(userAgent)) return "iOS";
  if (/\bAndroid\b/.test(userAgent)) return "Android";
  if (/\bWindows NT\b/.test(userAgent)) return "Windows";
  if (/\bMacintosh\b|\bMac OS X\b/.test(userAgent)) return "macOS";
  if (/\bCrOS\b/.test(userAgent)) return "ChromeOS";
  if (/\bLinux\b/.test(userAgent)) return "Linux";
  return null;
}

function deviceKind(userAgent: string): SessionDeviceKind {
  if (/\bPostmanRuntime\/|\bcurl\//.test(userAgent)) return "terminal";
  if (/\biPad\b|\bAndroid\b(?!.*\bMobile\b)/.test(userAgent)) return "tablet";
  if (/\biPhone\b|\bAndroid\b.*\bMobile\b/.test(userAgent)) return "mobile";
  if (platformLabel(userAgent)) return "desktop";
  return "unknown";
}

export function describeSessionDevice(
  userAgent: string | null | undefined,
  unknownLabel: string,
): SessionDevice {
  if (!userAgent) {
    return { label: unknownLabel, kind: "unknown" };
  }
  const browser = browserLabel(userAgent);
  const platform = platformLabel(userAgent);
  const label =
    browser && platform
      ? `${browser} · ${platform}`
      : browser ?? platform ?? unknownLabel;
  return { label, kind: deviceKind(userAgent) };
}

export function shortSessionId(id: string): string {
  return id.slice(-8);
}
