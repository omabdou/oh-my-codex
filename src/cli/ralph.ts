import { startMode, updateModeState } from '../modes/base.js';
import { ensureCanonicalRalphArtifacts } from '../ralph/persistence.js';
import type { RalphPrdPolicy } from '../ralph/contract.js';

const RALPH_HELP = `omx ralph - Launch Codex with ralph persistence mode active

Usage:
  omx ralph [codex-args...]   Initialize ralph state and launch Codex

Options:
  --help, -h    Show this help message
  --no-prd      Skip PRD auto-scaffold (does not bypass ralplan-first gate)

Ralph persistence mode initializes state tracking so the OMC ralph loop
can maintain context across Codex sessions.
`;

/**
 * Codex CLI flags that consume the next argv token as their value.
 * Both long (--flag value) and short (-f value) forms are listed.
 * Flags using --flag=value syntax are handled generically.
 */
const VALUE_TAKING_FLAGS = new Set([
  '--model',
  '--provider',
  '--config',
  '-c',            // codex -c key=value
  '-i',            // images-dir short form
  '--images-dir',
]);

export interface RalphLaunchArgs {
  forwardedArgs: string[];
  taskDescription: string;
  prdPolicy: RalphPrdPolicy;
}

export interface RalphCommandDependencies {
  cwd?: string;
  launchWithHud?: (args: string[]) => Promise<void>;
  ensureArtifacts?: typeof ensureCanonicalRalphArtifacts;
  startModeFn?: typeof startMode;
  updateModeStateFn?: typeof updateModeState;
  logger?: (message: string) => void;
}

export function normalizeRalphLaunchArgs(args: readonly string[]): RalphLaunchArgs {
  const forwardedArgs: string[] = [];
  const words: string[] = [];
  let prdPolicy: RalphPrdPolicy = 'required';
  let i = 0;

  while (i < args.length) {
    const token = args[i];

    if (token === '--') {
      forwardedArgs.push(token);
      for (let j = i + 1; j < args.length; j++) {
        forwardedArgs.push(args[j]);
        words.push(args[j]);
      }
      break;
    }

    if (token === '--no-prd') {
      prdPolicy = 'opt_out';
      i++;
      continue;
    }

    if (token.startsWith('--') && token.includes('=')) {
      forwardedArgs.push(token);
      i++;
      continue;
    }

    if (token.startsWith('-') && VALUE_TAKING_FLAGS.has(token)) {
      forwardedArgs.push(token);
      if (i + 1 < args.length) {
        forwardedArgs.push(args[i + 1]);
      }
      i += 2;
      continue;
    }

    if (token.startsWith('-')) {
      forwardedArgs.push(token);
      i++;
      continue;
    }

    forwardedArgs.push(token);
    words.push(token);
    i++;
  }

  return {
    forwardedArgs,
    taskDescription: words.join(' ') || 'ralph-cli-launch',
    prdPolicy,
  };
}

/**
 * Extract the human-readable task description from ralph CLI argv,
 * excluding option flags and their values.
 *
 * Supports:
 *  - `--` separator: everything after `--` is treated as task text
 *  - `--flag=value` syntax: the entire token is skipped
 *  - `--flag value` / `-f value` for known VALUE_TAKING_FLAGS: both tokens skipped
 *  - Unknown flags (e.g. `--yolo`): skipped as boolean flags
 *  - Positional tokens (not starting with `-`): collected as task text
 */
export function extractRalphTaskDescription(args: readonly string[]): string {
  return normalizeRalphLaunchArgs(args).taskDescription;
}

async function launchRalph(args: string[], launchWithHudOverride?: (args: string[]) => Promise<void>): Promise<void> {
  if (launchWithHudOverride) {
    await launchWithHudOverride(args);
    return;
  }
  // Dynamic import avoids a circular dependency with index.ts
  const { launchWithHud } = await import('./index.js');
  await launchWithHud(args);
}

export async function ralphCommand(args: string[], deps: RalphCommandDependencies = {}): Promise<void> {
  const cwd = deps.cwd ?? process.cwd();
  const logger = deps.logger ?? ((message: string) => console.log(message));
  const ensureArtifacts = deps.ensureArtifacts ?? ensureCanonicalRalphArtifacts;
  const startModeFn = deps.startModeFn ?? startMode;
  const updateModeStateFn = deps.updateModeStateFn ?? updateModeState;

  if (args[0] === '--help' || args[0] === '-h') {
    logger(RALPH_HELP);
    return;
  }

  const normalized = normalizeRalphLaunchArgs(args);

  // Initialize ralph persistence artifacts (state dirs, legacy PRD/progress migration)
  const artifacts = await ensureArtifacts(cwd, undefined, {
    prdPolicy: normalized.prdPolicy,
    ensurePrd: normalized.prdPolicy !== 'opt_out',
    taskDescription: normalized.taskDescription,
  });

  // Write initial ralph mode state
  await startModeFn('ralph', normalized.taskDescription, 50);
  await updateModeStateFn('ralph', {
    current_phase: 'starting',
    prd_policy: normalized.prdPolicy,
    canonical_progress_path: artifacts.canonicalProgressPath,
    ...(artifacts.canonicalPrdPath ? { canonical_prd_path: artifacts.canonicalPrdPath } : {}),
  });

  if (artifacts.migratedPrd) {
    logger(`[ralph] Migrated legacy PRD -> ${artifacts.canonicalPrdPath}`);
  }
  if (artifacts.scaffoldedPrd) {
    logger(`[ralph] Created canonical PRD scaffold -> ${artifacts.canonicalPrdPath}`);
  }
  if (artifacts.migratedProgress) {
    logger(`[ralph] Migrated legacy progress -> ${artifacts.canonicalProgressPath}`);
  }

  logger('[ralph] Ralph persistence mode active. Launching Codex...');
  await launchRalph(normalized.forwardedArgs, deps.launchWithHud);
}
