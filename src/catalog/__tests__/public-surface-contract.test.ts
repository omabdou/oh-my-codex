import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { readCatalogManifest } from '../reader.js';

async function readRepoFile(relativePath: string): Promise<string> {
  return readFile(join(process.cwd(), relativePath), 'utf8');
}

describe('catalog public-surface contract', () => {
  it('marks task-intent review skills and internal experts consistently in the manifest', () => {
    const manifest = readCatalogManifest();

    const analyze = manifest.skills.find((entry) => entry.name === 'analyze');
    const codeReview = manifest.skills.find((entry) => entry.name === 'code-review');
    const securityReview = manifest.skills.find((entry) => entry.name === 'security-review');
    const review = manifest.skills.find((entry) => entry.name === 'review');

    assert.equal(analyze?.status, 'active');
    assert.equal(analyze?.surface, 'public_task_intent');
    assert.equal(codeReview?.status, 'active');
    assert.equal(codeReview?.surface, 'public_task_intent');
    assert.equal(securityReview?.status, 'deprecated');
    assert.equal(securityReview?.surface, 'public_compatibility');
    assert.equal(securityReview?.canonical, 'code-review');
    assert.equal(review?.status, 'deprecated');
    assert.equal(review?.surface, 'public_compatibility');
    assert.equal(review?.canonical, 'plan --review');

    const architect = manifest.agents.find((entry) => entry.name === 'architect');
    const debuggerPrompt = manifest.agents.find((entry) => entry.name === 'debugger');
    const codeReviewer = manifest.agents.find((entry) => entry.name === 'code-reviewer');
    const securityReviewer = manifest.agents.find((entry) => entry.name === 'security-reviewer');
    const critic = manifest.agents.find((entry) => entry.name === 'critic');

    assert.equal(architect?.status, 'internal');
    assert.equal(architect?.surface, 'internal_expert');
    assert.equal(debuggerPrompt?.status, 'internal');
    assert.equal(debuggerPrompt?.surface, 'internal_expert');
    assert.equal(codeReviewer?.status, 'internal');
    assert.equal(codeReviewer?.surface, 'internal_expert');
    assert.equal(securityReviewer?.status, 'internal');
    assert.equal(securityReviewer?.surface, 'internal_expert');
    assert.equal(critic?.status, 'active');
    assert.equal(critic?.surface, 'public_agent');
  });

  it('rejects invalid surface metadata in the manifest schema', async () => {
    const schema = await import('../schema.js');
    const manifest = readCatalogManifest();
    const broken = JSON.parse(JSON.stringify(manifest));
    const analyze = broken.skills.find((entry: { name: string }) => entry.name === 'analyze');
    analyze.surface = 'totally_invalid';
    assert.throws(() => schema.validateCatalogManifest(broken), /skills\[\d+\]\.surface/);
  });



  it('marks reviewer-family specialist aliases as hidden compatibility rather than public compatibility', () => {
    const manifest = readCatalogManifest();
    for (const name of ['style-reviewer', 'quality-reviewer', 'api-reviewer', 'performance-reviewer']) {
      const entry = manifest.agents.find((agent) => agent.name === name);
      assert.equal(entry?.surface, 'hidden_compatibility');
    }
  });

  it('documents the three-entry public review/analysis surface in docs/skills.html', async () => {
    const skillsDoc = await readRepoFile('docs/skills.html');

    assert.match(skillsDoc, /Public Review\/Analysis Entry Points/i);
    assert.match(skillsDoc, /\$analyze/);
    assert.match(skillsDoc, /\$code-review/);
    assert.match(skillsDoc, /\/prompts:critic/);
    assert.match(skillsDoc, /\$security-review<\/code>[\s\S]*?<code>\$review<\/code> stay available only as[\s\S]*?compatibility\/deprecated shims/i);
  });

  it('keeps internal experts out of the primary docs/agents.html surface', async () => {
    const agentsDoc = await readRepoFile('docs/agents.html');

    assert.match(agentsDoc, /Public Review\/Analysis Entry Points/i);
    assert.match(agentsDoc, /\/prompts:architect <strong>\(internal expert\)<\/strong>/i);
    assert.match(agentsDoc, /\/prompts:debugger <strong>\(internal expert\)<\/strong>/i);
    assert.match(agentsDoc, /\/prompts:code-reviewer <strong>\(internal expert\)<\/strong>/i);
    assert.match(agentsDoc, /\/prompts:security-reviewer <strong>\(internal expert\)<\/strong>/i);
    assert.match(agentsDoc, /first-class public critique agent/i);
  });
});
