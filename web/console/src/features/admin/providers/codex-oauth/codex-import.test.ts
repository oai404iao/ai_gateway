import { describe, expect, it } from "vitest";
import { parseCodexImportDocuments } from "./codex-import";

function jwt(payload: Record<string, unknown>): string {
  const encode = (value: Record<string, unknown>) =>
    btoa(JSON.stringify(value))
      .replace(/\+/g, "-")
      .replace(/\//g, "_")
      .replace(/=+$/g, "");
  return `${encode({ alg: "none" })}.${encode(payload)}.signature`;
}

describe("Codex import parser", () => {
  it("normalizes an ai-gateway bundle with portable proxies", () => {
    const parsed = parseCodexImportDocuments([
      {
        name: "gateway-export.json",
        content: JSON.stringify({
          type: "ai-gateway-codex-credentials",
          version: 1,
          proxies: [
            {
              proxy_key: "00000000-0000-0000-0000-000000000001",
              name: "US egress",
              proxy_url: "socks5h://proxy.example:1080",
              username: "proxy-user",
              password: "proxy-password",
              no_proxy_hosts: [],
              enabled: true,
            },
          ],
          credentials: [
            {
              label: "Personal Plus",
              account_id: "account-1",
              id_token: jwt({
                email: "codex@example.test",
                "https://api.openai.com/auth": {
                  chatgpt_account_id: "account-1",
                  chatgpt_user_id: "user-1",
                },
              }),
              access_token: jwt({ exp: 1_900_000_000 }),
              refresh_token: "refresh-1",
              proxy_key: "00000000-0000-0000-0000-000000000001",
              weight: 80,
              quota_threshold_percent: 90,
              enabled: true,
            },
          ],
        }),
      },
    ]);

    expect(parsed.errors).toEqual([]);
    expect(parsed.proxies).toHaveLength(1);
    expect(parsed.proxies[0]).toMatchObject({
      name: "US egress",
      proxy_url: "socks5h://proxy.example:1080",
      username: "proxy-user",
    });
    expect(parsed.credentials[0]).toMatchObject({
      source: "ai_gateway",
      label: "Personal Plus",
      account_id: "account-1",
      user_id: "user-1",
      weight: "80",
      quota_threshold_percent: "90",
      source_proxy_key: parsed.proxies[0]?.source_key,
      selected: true,
    });
  });

  it("accepts a CLIProxyAPI auth file and separates embedded proxy credentials", () => {
    const parsed = parseCodexImportDocuments([
      {
        name: "codex-user.json",
        content: JSON.stringify({
          type: "codex",
          email: "cpa@example.test",
          account_id: "account-cpa",
          id_token: jwt({ email: "cpa@example.test" }),
          access_token: jwt({ exp: 1_900_000_000 }),
          refresh_token: "refresh-cpa",
          proxy_url: "socks5://proxy-user:proxy-password@127.0.0.1:1080",
        }),
      },
    ]);

    expect(parsed.errors).toEqual([]);
    expect(parsed.credentials[0]).toMatchObject({
      source: "cpa",
      label: "cpa@example.test",
      account_id: "account-cpa",
      selected: true,
    });
    expect(parsed.proxies[0]).toMatchObject({
      proxy_url: "socks5://127.0.0.1:1080",
      username: "proxy-user",
      password: "proxy-password",
    });
  });

  it("reads a Sub2API export envelope and allows access-token identity fallback", () => {
    const accessToken = jwt({
      email: "sub2api@example.test",
      "https://api.openai.com/auth": {
        chatgpt_account_id: "account-sub2api",
      },
    });
    const parsed = parseCodexImportDocuments([
      {
        name: "sub2api-export.json",
        content: JSON.stringify({
          code: 0,
          data: {
            type: "sub2api-data",
            version: 1,
            proxies: [
              {
                proxy_key: "socks5|10.0.0.1|1080|user|pass",
                name: "Sub2API proxy",
                protocol: "socks5",
                host: "10.0.0.1",
                port: 1080,
                username: "user",
                password: "pass",
                status: "active",
              },
            ],
            accounts: [
              {
                name: "Sub2API account",
                platform: "openai",
                type: "oauth",
                proxy_key: "socks5|10.0.0.1|1080|user|pass",
                credentials: {
                  access_token: accessToken,
                  refresh_token: "refresh-sub2api",
                },
              },
              {
                name: "Ignored Claude account",
                platform: "anthropic",
                type: "oauth",
                credentials: { access_token: "ignored" },
              },
            ],
          },
        }),
      },
    ]);

    expect(parsed.errors).toEqual([]);
    expect(parsed.credentials).toHaveLength(1);
    expect(parsed.credentials[0]).toMatchObject({
      source: "sub2api",
      label: "Sub2API account",
      email: "sub2api@example.test",
      account_id: "account-sub2api",
      id_token: "",
      selected: true,
    });
    expect(parsed.credentials[0]?.warnings).toContain(
      "ID token is missing; identity will be read from the access token.",
    );
    expect(parsed.proxies[0]?.proxy_url).toBe("socks5://10.0.0.1:1080");
  });

  it("marks duplicate accounts and unsupported JSON before import", () => {
    const credential = {
      type: "codex",
      account_id: "duplicate-account",
      id_token: jwt({}),
      access_token: jwt({}),
      refresh_token: "duplicate-refresh",
    };
    const parsed = parseCodexImportDocuments([
      {
        name: "duplicates.json",
        content: JSON.stringify([credential, credential]),
      },
      {
        name: "unsupported.json",
        content: JSON.stringify({ hello: "world" }),
      },
    ]);

    expect(parsed.credentials).toHaveLength(2);
    expect(parsed.credentials[0]?.selected).toBe(true);
    expect(parsed.credentials[1]?.selected).toBe(false);
    expect(parsed.credentials[1]?.errors).toContain("Duplicate of import row 1.");
    expect(parsed.errors).toContain(
      "unsupported.json: No Codex token fields were found.",
    );
  });

  it("keeps different Business members in the same workspace importable", () => {
    const credential = (email: string, userId: string) => ({
      type: "codex",
      account_id: "business-workspace",
      id_token: jwt({
        email,
        "https://api.openai.com/auth": {
          chatgpt_account_id: "business-workspace",
          chatgpt_user_id: userId,
          chatgpt_plan_type: "business",
        },
      }),
      access_token: jwt({ exp: 1_900_000_000 }),
      refresh_token: `refresh-${userId}`,
    });
    const parsed = parseCodexImportDocuments([
      {
        name: "business-members.json",
        content: JSON.stringify([
          credential("shared@example.test", "user-a"),
          credential("shared@example.test", "user-b"),
        ]),
      },
    ]);

    expect(parsed.credentials).toHaveLength(2);
    expect(parsed.credentials).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          account_id: "business-workspace",
          user_id: "user-a",
          selected: true,
        }),
        expect.objectContaining({
          account_id: "business-workspace",
          user_id: "user-b",
          selected: true,
        }),
      ]),
    );
  });
});
