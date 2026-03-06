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
    assert.equal(codeReview?.status, 'active');
    assert.equal(securityReview?.status, 'deprecated');
    assert.equal(securityReview?.canonical, 'code-review');
    assert.equal(review?.status, 'deprecated');
    assert.equal(review?.canonical, 'plan --review');

    const architect = manifest.agents.find((entry) => entry.name === 'architect');
    const debuggerPrompt = manifest.agents.find((entry) => entry.name === 'debugger');
    const codeReviewer = manifest.agents.find((entry) => entry.name === 'code-reviewer');
    const securityReviewer = manifest.agents.find((entry) => entry.name === 'security-reviewer');
    const critic = manifest.agents.find((entry) => entry.name === 'critic');

    assert.equal(architect?.status, 'internal');
    assert.equal(debuggerPrompt?.status, 'internal');
    assert.equal(codeReviewer?.status, 'internal');
    assert.equal(securityReviewer?.status, 'internal');
    assert.equal(critic?.status, 'active');
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
  });
});
