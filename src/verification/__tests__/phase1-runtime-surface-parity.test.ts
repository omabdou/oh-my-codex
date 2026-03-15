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


  it('keeps the HUD lane truthfully scoped to native launch ownership rather than full behavioral parity', () => {
    const hudIndexSource = readSource('src', 'hud', 'index.ts');
    const hudStateSource = readSource('src', 'hud', 'state.ts');
    const hudRenderSource = readSource('src', 'hud', 'render.ts');
    const hudNativeSource = readSource('crates', 'omx-runtime', 'src', 'hud.rs');
    const cutoverDoc = readSource('docs', 'reference', 'rust-runtime-phase1-cutover-order.md');
    const parityDoc = readSource('docs', 'reference', 'ts-rust-parity-lanes.md');

    assert.match(hudIndexSource, /readAllState/);
    assert.match(hudIndexSource, /renderHud/);
    assert.match(hudIndexSource, /buildPhase1HudWatchCommand/);
    assert.match(hudStateSource, /export async function readAllState/);
    assert.match(hudRenderSource, /export function renderHud/);

    assert.match(hudNativeSource, /pub fn run_hud_watch/);
    assert.match(hudNativeSource, /native-hud/);

    assert.match(cutoverDoc, /What live HUD\/runtime owner is now replaced under native mode/i);
    assert.match(cutoverDoc, /remaining risk is now parity, not a live Node startup dependency/i);
    assert.match(parityDoc, /## Lane 3 — HUD behavior parity/);
    assert.match(parityDoc, /HUD launch ownership is native on the guarded path, but behavior is still parity-incomplete/i);
    assert.match(parityDoc, /Unsafe statements[\s\S]*HUD parity is complete/);
  });

  it('keeps the watcher/notification lane mapped onto the exact TS watcher SSOT touchpoints and native operator subcommands', () => {
    const cliIndexSource = readSource('src', 'cli', 'index.ts');
    const replyListenerSource = readSource('src', 'notifications', 'reply-listener.ts');
    const watchersSource = readSource('crates', 'omx-runtime', 'src', 'watchers.rs');
    const replyListenerNativeSource = readSource('crates', 'omx-runtime', 'src', 'reply_listener.rs');
    const runtimeMainSource = readSource('crates', 'omx-runtime', 'src', 'main.rs');

    assert.match(
      cliIndexSource,
      /spawn\(\s*resolveRuntimeBinaryPath\(\{ cwd, env: process\.env \}\),\s*\[\s*'notify-fallback',\s*'--cwd',\s*cwd,\s*'--notify-script',\s*notifyScript,\s*'--pid-file',\s*pidPath,\s*'--parent-pid',\s*String\(process\.pid\)/m,
    );
    assert.match(
      cliIndexSource,
      /process\.env\.OMX_NOTIFY_FALLBACK_MAX_LIFETIME_MS\s*\?\s*\['--max-lifetime-ms',\s*process\.env\.OMX_NOTIFY_FALLBACK_MAX_LIFETIME_MS\]/m,
    );
    assert.match(
      cliIndexSource,
      /spawn\(\s*resolveRuntimeBinaryPath\(\{ cwd, env: process\.env \}\),\s*\[\s*'hook-derived',\s*'--cwd',\s*cwd,\s*'--pid-file',\s*pidPath/m,
    );
    assert.match(cliIndexSource, /spawnSync\(runtimeBinaryPath, \['notify-fallback', '--once', '--cwd', cwd, '--notify-script', notifyScript\]/);
    assert.match(cliIndexSource, /spawnSync\(runtimeBinaryPath, \['hook-derived', '--once', '--cwd', cwd\]/);

    assert.match(replyListenerSource, /resolveRuntimeBinaryPath\(\{ cwd: process\.cwd\(\), env: process\.env \}\)/);
    assert.match(replyListenerSource, /\["reply-listener", \.\.\.args\]/);
    assert.match(replyListenerSource, /\["reply-listener"\]/);
    assert.match(replyListenerSource, /createMinimalDaemonEnv\(\)/);
    assert.match(replyListenerSource, /native reply-listener runtime unavailable/);

    assert.match(watchersSource, /pub fn run_notify_fallback/);
    assert.match(watchersSource, /pub fn run_hook_derived/);
    assert.match(watchersSource, /"--notify-script" if allow_notify_script/);
    assert.match(watchersSource, /"--parent-pid"/);
    assert.match(watchersSource, /"--max-lifetime-ms"/);
    assert.match(watchersSource, /failed writing pid-file/);

    for (const marker of [
      /Some\("reply-listener"\) => reply_listener::run_reply_listener\(&args\[1\.\.\]\)/,
      /Some\("notify-fallback"\) => watchers::run_notify_fallback\(&args\[1\.\.\]\)/,
      /Some\("hook-derived"\) => watchers::run_hook_derived\(&args\[1\.\.\]\)/,
      /omx-runtime reply-listener/,
      /omx-runtime notify-fallback \[--once\] --cwd <path> \[--notify-script <path>\] \[--pid-file <path>\]/,
      /omx-runtime hook-derived \[--once\] --cwd <path> \[--pid-file <path>\]/,
    ]) {
      assert.match(runtimeMainSource, marker);
    }

    for (const marker of [
      /Some\("--once"\) => start_reply_listener\(true\)/,
      /Some\("status"\) => status_reply_listener\(\)/,
      /Some\("stop"\) => stop_reply_listener\(\)/,
      /Some\("discord-fetch"\) => discord_fetch_command\(\)/,
      /Some\("lookup-message"\) => lookup_message_command\(&args\[1\.\.\]\)/,
      /Some\("inject-reply"\) => inject_reply_command\(&args\[1\.\.\]\)/,
    ]) {
      assert.match(replyListenerNativeSource, marker);
    }
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
