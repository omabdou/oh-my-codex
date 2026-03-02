import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { ralphCommand } from '../ralph.js';

describe('ralphCommand CLI boundary behavior', () => {
  it('strips --no-prd before launchWithHud command invocation', async () => {
    let launchedArgs: string[] | null = null;

    await ralphCommand(['--no-prd', '--model', 'gpt-5', 'ship', 'it'], {
      ensureArtifacts: async () => ({
        canonicalPrdPath: undefined,
        canonicalProgressPath: '/tmp/ralph-progress.json',
        migratedPrd: false,
        migratedProgress: false,
        scaffoldedPrd: false,
      }),
      startModeFn: async () => ({
        active: true,
        mode: 'ralph',
        iteration: 0,
        max_iterations: 50,
        current_phase: 'starting',
        started_at: new Date().toISOString(),
      }),
      updateModeStateFn: async () => ({
        active: true,
        mode: 'ralph',
        iteration: 0,
        max_iterations: 50,
        current_phase: 'starting',
        started_at: new Date().toISOString(),
      }),
      launchWithHud: async (args) => {
        launchedArgs = args;
      },
      logger: () => {},
    });

    assert.deepEqual(launchedArgs, ['--model', 'gpt-5', 'ship', 'it']);
  });

  it('persists prd_policy=opt_out when --no-prd is provided', async () => {
    const updates: Array<Record<string, unknown>> = [];
    const artifactCalls: Array<Record<string, unknown>> = [];

    await ralphCommand(['--no-prd', 'plan', 'carefully'], {
      ensureArtifacts: async (_cwd, _sessionId, options) => {
        artifactCalls.push(options as Record<string, unknown>);
        return {
          canonicalPrdPath: undefined,
          canonicalProgressPath: '/tmp/ralph-progress.json',
          migratedPrd: false,
          migratedProgress: false,
          scaffoldedPrd: false,
        };
      },
      startModeFn: async () => ({
        active: true,
        mode: 'ralph',
        iteration: 0,
        max_iterations: 50,
        current_phase: 'starting',
        started_at: new Date().toISOString(),
      }),
      updateModeStateFn: async (_mode, update) => {
        updates.push(update as Record<string, unknown>);
        return {
          active: true,
          mode: 'ralph',
          iteration: 0,
          max_iterations: 50,
          current_phase: 'starting',
          started_at: new Date().toISOString(),
          ...update,
        };
      },
      launchWithHud: async () => {},
      logger: () => {},
    });

    assert.equal(artifactCalls.length, 1);
    assert.equal(artifactCalls[0]?.prdPolicy, 'opt_out');
    assert.equal(artifactCalls[0]?.ensurePrd, false);
    assert.equal(artifactCalls[0]?.taskDescription, 'plan carefully');

    assert.equal(updates.length, 1);
    assert.equal(updates[0]?.prd_policy, 'opt_out');
    assert.equal(updates[0]?.canonical_progress_path, '/tmp/ralph-progress.json');
  });
});
