import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));

const configureOpenClawSkill = readFileSync(
  join(__dirname, "../../../skills/configure-openclaw/SKILL.md"),
  "utf-8",
);
const openclawIntegrationDoc = readFileSync(
  join(__dirname, "../../../docs/openclaw-integration.md"),
  "utf-8",
);
const configureNotificationsSkill = readFileSync(
  join(__dirname, "../../../skills/configure-notifications/SKILL.md"),
  "utf-8",
);

function extractJsonFenceContaining(content: string, needle: string): string {
  const matches = [...content.matchAll(/```json\n([\s\S]*?)\n```/g)];
  const found = matches.map((m) => m[1]).find((block) => block.includes(needle));
  assert.ok(found, `Expected a JSON code fence containing ${needle}`);
  return found;
}

describe("OpenClaw setup workflow contracts", () => {
  it("documents explicit /hooks/agent delivery verification path", () => {
    assert.ok(
      configureOpenClawSkill.includes("/hooks/agent"),
      "configure-openclaw skill should include /hooks/agent",
    );
    assert.ok(
      openclawIntegrationDoc.includes("/hooks/agent"),
      "openclaw integration doc should include /hooks/agent",
    );
    assert.ok(
      /Delivery verification \(`\/hooks\/agent`\)/.test(configureOpenClawSkill),
      "configure-openclaw skill should include a delivery verification section",
    );
  });

  it("keeps wake smoke test guidance alongside delivery verification", () => {
    assert.ok(
      configureOpenClawSkill.includes("Wake smoke test (`/hooks/wake`)"),
      "configure-openclaw skill should include /hooks/wake smoke test",
    );
    assert.ok(
      openclawIntegrationDoc.includes("Wake smoke test (`/hooks/wake`)"),
      "openclaw integration doc should include /hooks/wake smoke test",
    );
  });

  it("includes pass/fail diagnostics guidance", () => {
    assert.ok(
      /Pass\/Fail Diagnostics Guidance/.test(configureOpenClawSkill),
      "configure-openclaw skill should include pass/fail diagnostics",
    );
    assert.ok(
      /Pass\/Fail Diagnostics/.test(openclawIntegrationDoc),
      "openclaw integration doc should include pass/fail diagnostics",
    );
  });

  it("includes token check, URL reachability check, and command dual env gate guidance", () => {
    assert.ok(
      configureOpenClawSkill.includes("Hook token present"),
      "configure-openclaw skill should require hook token validation",
    );
    assert.ok(
      configureOpenClawSkill.includes("Gateway URL format and reachability"),
      "configure-openclaw skill should require URL reachability validation",
    );
    assert.ok(
      configureOpenClawSkill.includes("OMX_OPENCLAW_COMMAND=1"),
      "configure-openclaw skill should mention command dual gate",
    );

    assert.ok(
      openclawIntegrationDoc.includes("OMX_OPENCLAW_COMMAND=1"),
      "openclaw integration doc should mention command dual gate",
    );
    assert.ok(
      openclawIntegrationDoc.includes("token present"),
      "openclaw integration doc should include token preflight check",
    );
    assert.ok(
      openclawIntegrationDoc.includes("reachability"),
      "openclaw integration doc should include URL reachability checks",
    );
  });

  it("uses runtime schema examples with notifications.openclaw.gateways + hooks", () => {
    assert.ok(
      configureOpenClawSkill.includes("notifications.openclaw.gateways"),
      "configure-openclaw skill should reference notifications.openclaw.gateways",
    );
    assert.ok(
      configureOpenClawSkill.includes("notifications.openclaw.hooks"),
      "configure-openclaw skill should reference notifications.openclaw.hooks",
    );

    const configJson = extractJsonFenceContaining(openclawIntegrationDoc, "\"notifications\"");
    const parsed = JSON.parse(configJson) as {
      notifications?: {
        openclaw?: {
          gateways?: Record<string, unknown>;
          hooks?: Record<string, unknown>;
        };
      };
    };

    assert.ok(parsed.notifications?.openclaw, "Doc example should include notifications.openclaw");
    assert.ok(parsed.notifications?.openclaw?.gateways, "Doc example should include openclaw.gateways");
    assert.ok(parsed.notifications?.openclaw?.hooks, "Doc example should include openclaw.hooks");
  });

  it("keeps configure-notifications handoff aligned to gateways + hooks language", () => {
    assert.ok(
      configureNotificationsSkill.includes("notifications.openclaw.gateways + hooks"),
      "configure-notifications skill should describe schema-aligned OpenClaw setup",
    );
  });
});
