#!/usr/bin/env node

import { existsSync, readFileSync } from 'node:fs';
import { readdir } from 'node:fs/promises';
import { join } from 'node:path';
import process from 'node:process';

const args = process.argv.slice(2);

const TEAM_SUBCOMMANDS = new Set(['status', 'resume', 'shutdown', 'api', 'help', '--help', '-h']);

function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, 'utf-8'));
  } catch {
    return null;
  }
}

async function isTerminalTaskSet(teamRoot) {
  try {
    const tasksDir = join(teamRoot, 'tasks');
    const entries = await readdir(tasksDir);
    if (entries.length === 0) return false;
    let hasAny = false;
    for (const entry of entries) {
      if (!entry.endsWith('.json')) continue;
      hasAny = true;
      const task = readJson(join(tasksDir, entry));
      const status = task?.status;
      if (!['completed', 'failed'].includes(status)) return false;
    }
    return hasAny;
  } catch {
    return false;
  }
}

async function keepLegacyTeamAlive(teamName) {
  const teamRoot = join(process.cwd(), '.omx', 'state', 'team', teamName);
  while (existsSync(teamRoot)) {
    const phase = readJson(join(teamRoot, 'phase.json'))?.current_phase;
    if (['complete', 'failed', 'cancelled'].includes(phase)) return;
    if (await isTerminalTaskSet(teamRoot)) return;
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
}

function extractTeamStartArgs(args) {
  const subcommand = args[1] || '';
  if (args[0] !== 'team' || TEAM_SUBCOMMANDS.has(subcommand)) {
    return null;
  }
  return args.slice(1);
}

async function main() {
  const cli = await import('../dist/cli/index.js');
  const team = await import('../dist/cli/team.js');

  const teamStartArgs = extractTeamStartArgs(args);
  let parsedTeamName = null;
  if (teamStartArgs) {
    try {
      parsedTeamName = team.parseTeamStartArgs(teamStartArgs).parsed.teamName;
    } catch {
      parsedTeamName = null;
    }
  }

  const shutdownOnSignal = async () => {
    if (!parsedTeamName) process.exit(130);
    try {
      await cli.main(['team', 'shutdown', parsedTeamName, '--force']);
    } catch {
      // best effort only
    }
    process.exit(130);
  };

  process.on('SIGINT', () => {
    void shutdownOnSignal();
  });
  process.on('SIGTERM', () => {
    void shutdownOnSignal();
  });

  await cli.main(args);

  if (parsedTeamName) {
    await keepLegacyTeamAlive(parsedTeamName);
  }
}

await main();
