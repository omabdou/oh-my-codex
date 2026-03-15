import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { mkdtemp, readFile, rm, writeFile, chmod } from 'node:fs/promises';
import { join } from 'node:path';
import { tmpdir, homedir } from 'node:os';

type PackageJson = {
  dependencies?: Record<string, string>;
};

describe('team MCP runtime dependency contract', () => {
  it('declares zod as a top-level runtime dependency because team-server imports it directly', () => {
    const teamServerSource = readFileSync(join(process.cwd(), 'src', 'mcp', 'team-server.ts'), 'utf8');
    assert.match(teamServerSource, /from ['\"]zod['\"]/);

    const pkg = JSON.parse(readFileSync(join(process.cwd(), 'package.json'), 'utf8')) as PackageJson;
    assert.equal(
      typeof pkg.dependencies?.zod,
      'string',
      'package.json must declare zod in top-level runtime dependencies when shipped JS imports it directly',
    );
  });

  it('launches the native runtime-run seam instead of the legacy node runtime-cli entrypoint', () => {
    const teamServerSource = readFileSync(join(process.cwd(), 'src', 'mcp', 'team-server.ts'), 'utf8');

    assert.match(teamServerSource, /resolveRuntimeBinaryPath\(\{ cwd: inputCwd, env: process\.env \}\)/);
    assert.match(teamServerSource, /spawn\(runtimeBinaryPath, \['runtime-run'\]/);
    assert.doesNotMatch(teamServerSource, /runtime-cli\.js/);
    assert.doesNotMatch(teamServerSource, /spawn\('node', \[runtimeCliPath\]/);
  });

  it('keeps the runtime-run seam off legacy Node bridges while documenting native startup ownership', () => {
    const runtimeRunSource = readFileSync(join(process.cwd(), 'crates', 'omx-runtime', 'src', 'runtime_run.rs'), 'utf8');

    assert.match(runtimeRunSource, /fn start_team\(/);
    assert.match(runtimeRunSource, /fn initialize_team_state\(/);
    assert.match(runtimeRunSource, /fn create_team_session\(/);
    assert.match(runtimeRunSource, /fn send_worker_bootstrap_prompts\(/);
    assert.match(runtimeRunSource, /fn monitor_team\(/);
    assert.match(runtimeRunSource, /fn shutdown_team\(/);
    assert.doesNotMatch(runtimeRunSource, /START_TEAM_SCRIPT/);
    assert.doesNotMatch(runtimeRunSource, /import \{ startTeam \} from '\.\/dist\/team\/runtime\.js'/);
    assert.doesNotMatch(runtimeRunSource, /runtime-cli\.js/);
    assert.doesNotMatch(runtimeRunSource, /execute_node_json\(/);
    assert.doesNotMatch(runtimeRunSource, /resolve_runtime_cli_command/);
  });
});
const OMX_JOBS_DIR = join(homedir(), '.omx', 'team-jobs');

async function loadTeamServer() {
  process.env.OMX_TEAM_SERVER_DISABLE_AUTO_START = '1';
  return await import('../team-server.js');
}

describe('team MCP native runtime seam', () => {
  it('omx_run_team_start launches the native runtime binary with runtime-run and JSON stdin', async () => {
    const cwd = await mkdtemp(join(tmpdir(), 'omx-team-start-native-'));
    const fakeBinDir = await mkdtemp(join(tmpdir(), 'omx-team-start-native-bin-'));
    const runtimeStubPath = join(fakeBinDir, 'omx-runtime');
    const argsLogPath = join(fakeBinDir, 'runtime-args.log');
    const stdinLogPath = join(fakeBinDir, 'runtime-stdin.json');
    const prevRuntimeBin = process.env.OMX_RUNTIME_BIN;
    let startPayload: { jobId?: string } | undefined;

    try {
      await writeFile(
        runtimeStubPath,
        `#!/bin/sh
set -eu
printf '%s\n' "$*" > "${argsLogPath}"
cat > "${stdinLogPath}"
printf '{"status":"completed","teamName":"stub-team","taskResults":[],"duration":0,"workerCount":1}\n'
`,
      );
      await chmod(runtimeStubPath, 0o755);
      process.env.OMX_RUNTIME_BIN = runtimeStubPath;

      const { handleTeamToolCall } = await loadTeamServer();
      const startResponse = await handleTeamToolCall({
        params: {
          name: 'omx_run_team_start',
          arguments: {
            teamName: 'stub-team',
            agentTypes: ['codex'],
            tasks: [{ subject: 'one', description: 'desc' }],
            cwd,
          },
        },
      });

      startPayload = JSON.parse(startResponse.content[0]?.text ?? '{}') as { jobId?: string };
      assert.equal(typeof startPayload.jobId, 'string');

      await new Promise((resolve) => setTimeout(resolve, 100));

      const argsLog = await readFile(argsLogPath, 'utf8');
      const stdinLog = await readFile(stdinLogPath, 'utf8');
      assert.match(argsLog, /^runtime-run$/m);
      assert.match(stdinLog, /"teamName":"stub-team"/);
      assert.match(stdinLog, /"agentTypes":\["codex"\]/);
      assert.match(stdinLog, /"subject":"one"/);
      assert.match(stdinLog, new RegExp(`"cwd":${JSON.stringify(JSON.stringify(cwd)).slice(1, -1)}`));

      const statusResponse = await handleTeamToolCall({
        params: {
          name: 'omx_run_team_status',
          arguments: { job_id: startPayload.jobId },
        },
      });
      const statusPayload = JSON.parse(statusResponse.content[0]?.text ?? '{}') as { status?: string; result?: { status?: string } };
      assert.equal(statusPayload.status, 'completed');
      assert.equal(statusPayload.result?.status, 'completed');
    } finally {
      if (typeof prevRuntimeBin === 'string') process.env.OMX_RUNTIME_BIN = prevRuntimeBin;
      else delete process.env.OMX_RUNTIME_BIN;
      await rm(cwd, { recursive: true, force: true });
      await rm(fakeBinDir, { recursive: true, force: true });
      await rm(join(OMX_JOBS_DIR, `${startPayload?.jobId ?? 'missing'}.json`), { force: true }).catch(() => null);
      await rm(join(OMX_JOBS_DIR, `${startPayload?.jobId ?? 'missing'}-panes.json`), { force: true }).catch(() => null);
    }
  });

  it('passes extended startup parity fields through to runtime-run stdin', async () => {
    const cwd = await mkdtemp(join(tmpdir(), 'omx-team-start-parity-'));
    const fakeBinDir = await mkdtemp(join(tmpdir(), 'omx-team-start-parity-bin-'));
    const runtimeStubPath = join(fakeBinDir, 'omx-runtime');
    const stdinLogPath = join(fakeBinDir, 'runtime-stdin.json');
    const prevRuntimeBin = process.env.OMX_RUNTIME_BIN;
    let startPayload: { jobId?: string } | undefined;

    try {
      await writeFile(
        runtimeStubPath,
        `#!/bin/sh
set -eu
cat > "${stdinLogPath}"
printf '{"status":"completed","teamName":"stub-team","taskResults":[],"duration":0,"workerCount":2}\n'
`,
      );
      await chmod(runtimeStubPath, 0o755);
      process.env.OMX_RUNTIME_BIN = runtimeStubPath;

      const { handleTeamToolCall } = await loadTeamServer();
      const startResponse = await handleTeamToolCall({
        params: {
          name: 'omx_run_team_start',
          arguments: {
            teamName: 'stub-team',
            agentTypes: ['codex'],
            workerCount: 2,
            pollIntervalMs: 1500,
            tasks: [{
              subject: 'one',
              description: 'desc',
              owner: 'worker-1',
              blocked_by: ['2'],
              role: 'executor',
            }],
            cwd,
          },
        },
      });

      startPayload = JSON.parse(startResponse.content[0]?.text ?? '{}') as { jobId?: string };
      await new Promise((resolve) => setTimeout(resolve, 100));

      const stdinLog = await readFile(stdinLogPath, 'utf8');
      assert.match(stdinLog, /"workerCount":2/);
      assert.match(stdinLog, /"pollIntervalMs":1500/);
      assert.match(stdinLog, /"owner":"worker-1"/);
      assert.match(stdinLog, /"blocked_by":\["2"\]/);
      assert.match(stdinLog, /"role":"executor"/);
    } finally {
      if (typeof prevRuntimeBin === 'string') process.env.OMX_RUNTIME_BIN = prevRuntimeBin;
      else delete process.env.OMX_RUNTIME_BIN;
      await rm(cwd, { recursive: true, force: true });
      await rm(fakeBinDir, { recursive: true, force: true });
      await rm(join(OMX_JOBS_DIR, `${startPayload?.jobId ?? 'missing'}.json`), { force: true }).catch(() => null);
      await rm(join(OMX_JOBS_DIR, `${startPayload?.jobId ?? 'missing'}-panes.json`), { force: true }).catch(() => null);
    }
  });
});
