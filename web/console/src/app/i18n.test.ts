/// <reference types="node" />
import fs from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { translateFor, zhCN } from "@/app/i18n";

function collectStaticI18nKeys(srcRoot: string): string[] {
  const files = fs
    .readdirSync(srcRoot, { recursive: true, withFileTypes: true })
    .filter(
      (entry) =>
        entry.isFile() &&
        /\.(ts|tsx)$/.test(entry.name) &&
        !/\.test\.(ts|tsx)$/.test(entry.name) &&
        !entry.name.endsWith(".d.ts"),
    )
    .map((entry) => path.join(entry.parentPath, entry.name));

  const callPattern = /\b(?:t|translate)\(\s*"((?:[^"\\]|\\.)*)"/g;
  const translateForPattern = /\btranslateFor\(\s*[^,]+,\s*"((?:[^"\\]|\\.)*)"/g;

  const keys = new Set<string>();
  for (const filePath of files) {
    const content = fs.readFileSync(filePath, "utf8");
    for (const pattern of [callPattern, translateForPattern]) {
      for (const match of content.matchAll(pattern)) {
        keys.add(JSON.parse(`"${match[1]}"`) as string);
      }
    }
  }
  return [...keys].sort();
}

describe("Console i18n", () => {
  it("uses the English message when a translation is missing", () => {
    expect(translateFor("zh-CN", "Unknown message {id}", { id: 0 })).toBe("Unknown message 0");
  });

  it("interpolates values in the translated message", () => {
    expect(translateFor("zh-CN", "Session {id}", { id: "000000a1" })).toBe("会话 000000a1");
  });

  it("covers every static t() and translate() key with non-empty en-US and zh-CN messages", () => {
    const srcRoot = path.resolve(import.meta.dirname, "..");
    const staticKeys = collectStaticI18nKeys(srcRoot);
    const zhLookup: Record<string, string> = zhCN;

    expect(staticKeys.length).toBeGreaterThan(0);
    expect(staticKeys.filter((key) => !zhLookup[key]?.trim())).toEqual([]);
    expect(staticKeys.filter((key) => !translateFor("en-US", key).trim())).toEqual([]);
  });
});
