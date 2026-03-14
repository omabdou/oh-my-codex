import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { TEAM_API_OPERATIONS } from '../../team/api-interop.js';
import { generateInitialInbox } from '../../team/worker-bootstrap.js';
import {
  buildPhase1HudWatchCommand,
  buildRuntimeCapturePaneCommand,
} from '../../cli/runtime-native.js';

function readSource(...parts: string[]): string {
  return readFileSync(join(process.cwd(), ...parts), 'utf8');
}

describe('phase-1 runtime surface parity contracts', () => {
  it('keeps the team state/runtime lane mapped from TS MCP entrypoints onto native runtime-run ownership', () => {
    const teamServerSource = readSource('src', 'mcp', 'team-server.ts');
    const runtimeRunSource = readSource('crates', 'omx-runtime', 'src', 'runtime_run.rs');
    const runtimeMainSource = readSource('crates', 'omx-runtime', 'src', 'main.rs');

    assert.match(teamServerSource, /spawn\(runtimeBinaryPath, \['runtime-run'\]/);
    assert.match(runtimeMainSource, /Some\("runtime-run"\) => runtime_run::run_runtime\(&args\[1\.\.\]\)/);

    for (const marker of [
      /fn start_team\(/,
      /fn initialize_team_state\(/,
      /fn create_team_session\(/,
      /fn finalize_team_state\(/,
      /fn send_worker_bootstrap_prompts\(/,
      /fn monitor_team\(/,
      /fn shutdown_team\(/,
    ]) {
      assert.match(runtimeRunSource, marker);
    }
  });

  it('keeps the tmux/control-plane lane aligned between TS command builders and native capture-pane/hud-watch entrypoints', () => {
    const runtimeMainSource = readSource('crates', 'omx-runtime', 'src', 'main.rs');

    assert.equal(
      buildRuntimeCapturePaneCommand('%21', 400),
      'omx-runtime capture-pane --pane-id %21 --tail-lines 400',
    );
    assert.equal(
      buildPhase1HudWatchCommand('/tmp/bin/omx.js', {
        env: { OMX_RUNTIME_HUD_NATIVE: '1', OMX_RUNTIME_BIN: '/tmp/rust/omx-runtime' },
        preset: 'focused',
      }),
      "'/tmp/rust/omx-runtime' hud-watch --preset=focused",
    );

    assert.match(runtimeMainSource, /Some\("capture-pane"\) => run_capture_pane\(&args\[1\.\.\]\)/);
    assert.match(runtimeMainSource, /Some\("hud-watch"\) => hud::run_hud_watch\(&args\[1\.\.\]\)/);
    assert.match(runtimeMainSource, /omx-runtime capture-pane --pane-id <pane-id>/);
    assert.match(runtimeMainSource, /omx-runtime hud-watch \[--once\]/);
  });

  it('keeps the watcher/notification lane mapped onto native notify-fallback, hook-derived, and reply-listener subcommands', () => {
    const cliIndexSource = readSource('src', 'cli', 'index.ts');
    const replyListenerSource = readSource('src', 'notifications', 'reply-listener.ts');
    const watchersSource = readSource('crates', 'omx-runtime', 'src', 'watchers.rs');
    const runtimeMainSource = readSource('crates', 'omx-runtime', 'src', 'main.rs');

    assert.match(cliIndexSource, /spawn\(\s*resolveRuntimeBinaryPath\(\{ cwd, env: process\.env \}\),\s*\[\s*'notify-fallback'/m);
    assert.match(cliIndexSource, /spawn\(\s*resolveRuntimeBinaryPath\(\{ cwd, env: process\.env \}\),\s*\[\s*'hook-derived'/m);
    assert.match(cliIndexSource, /spawnSync\(runtimeBinaryPath, \['notify-fallback', '--once', '--cwd', cwd, '--notify-script', notifyScript\]/);
    assert.match(cliIndexSource, /spawnSync\(runtimeBinaryPath, \['hook-derived', '--once', '--cwd', cwd\]/);

    assert.match(replyListenerSource, /resolveRuntimeBinaryPath/);
    assert.match(replyListenerSource, /native reply-listener runtime unavailable/);

    assert.match(watchersSource, /pub fn run_notify_fallback/);
    assert.match(watchersSource, /pub fn run_hook_derived/);
    assert.match(watchersSource, /failed writing pid-file/);

    assert.match(runtimeMainSource, /Some\("notify-fallback"\) => watchers::run_notify_fallback\(&args\[1\.\.\]\)/);
    assert.match(runtimeMainSource, /Some\("hook-derived"\) => watchers::run_hook_derived\(&args\[1\.\.\]\)/);
    assert.match(runtimeMainSource, /Some\("reply-listener"\) => reply_listener::run_reply_listener\(&args\[1\.\.\]\)/);
  });

  it('keeps the MCP/CLI worker boundary mapped to claim-safe team api operations without worker-side workingDirectory usage', () => {
    const workerSkill = readSource('skills', 'worker', 'SKILL.md');
    const inbox = generateInitialInbox('worker-2', 'parity-team', 'executor', [{
      id: '2',
      subject: 'Verify parity',
      description: 'Check lifecycle boundaries',
      status: 'pending',
      created_at: new Date().toISOString(),
    }]);

    for (const operation of [
      'send-message',
      'mailbox-list',
      'mailbox-mark-delivered',
      'claim-task',
      'transition-task-status',
      'release-task-claim',
    ] as const) {
      assert.ok(TEAM_API_OPERATIONS.includes(operation), `missing team api operation: ${operation}`);
    }

    assert.match(workerSkill, /omx team api claim-task/);
    assert.match(workerSkill, /omx team api transition-task-status/);
    assert.match(workerSkill, /omx team api mailbox-list/);
    assert.match(workerSkill, /omx team api mailbox-mark-delivered/);
    assert.match(inbox, /do not pass `workingDirectory` unless the lead explicitly asks/i);
    assert.doesNotMatch(inbox, /workingDirectory.*claim-task/i);
  });
});
