import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'fs/promises';
import { join } from 'path';
import { TEAM_EVENT_TYPES } from '../../team/contracts.js';

describe('state-server schema validation', () => {
  it('state_write schema includes prd_policy contract metadata', async () => {
    const src = await readFile(join(process.cwd(), 'src/mcp/state-server.ts'), 'utf8');
    const stateWriteBlockMatch = src.match(/name:\s*'state_write'[\s\S]*?required:\s*\['mode'\]/);
    assert.ok(stateWriteBlockMatch, 'Expected state_write schema block to exist');

    const block = stateWriteBlockMatch[0];
    assert.match(block, /prd_policy/);
    assert.match(block, /enum:\s*\[\.\.\.RALPH_PRD_POLICIES\]/);
    assert.match(block, /Defaults to required; opt_out skips PRD auto-scaffold only/i);
  });

  it('team_append_event schema enum is sourced from shared TEAM_EVENT_TYPES contract and contains expected values', async () => {
    const src = await readFile(join(process.cwd(), 'src/mcp/state-server.ts'), 'utf8');

    const enumRefMatch = src.match(/name:\s*'team_append_event'[\s\S]*?enum:\s*\[\.\.\.TEAM_EVENT_TYPES\]/);
    assert.ok(enumRefMatch, 'Expected team_append_event schema to reference TEAM_EVENT_TYPES');

    const enumValues = [...TEAM_EVENT_TYPES];
    assert.ok(
      enumValues.length > 0,
      `Expected at least one enum value, got ${enumValues.length}`,
    );

    for (const eventType of enumValues) {
      assert.equal(typeof eventType, 'string', `Expected string, got ${typeof eventType}`);
      assert.ok(eventType.length > 0, 'Expected non-empty event type string');
    }

    const unique = new Set(enumValues);
    assert.equal(
      unique.size,
      enumValues.length,
      `Found duplicate event types: ${enumValues.join(', ')}`,
    );
  });
});
