import { existsSync, readFileSync } from 'node:fs';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { z } from 'zod';

export const RALPHTHON_SCHEMA_VERSION = 1;
export const RALPHTHON_MODE = 'ralphthon';
const DEFAULT_IDLE_TIMEOUT_SEC = 30;
const DEFAULT_POLL_INTERVAL_SEC = 120;
const DEFAULT_MAX_RETRIES = 3;

const taskStatusSchema = z.enum(['pending', 'in_progress', 'done', 'failed']);
const phaseSchema = z.enum(['development', 'hardening', 'complete']);

const taskBaseSchema = z.object({
  id: z.string().trim().min(1),
  desc: z.string().trim().min(1),
  status: taskStatusSchema.default('pending'),
  retries: z.number().int().min(0).default(0),
  lastError: z.string().trim().min(1).optional(),
  startedAt: z.string().trim().min(1).optional(),
  completedAt: z.string().trim().min(1).optional(),
});

const storyTaskSchema = taskBaseSchema;
const hardeningTaskSchema = taskBaseSchema.extend({
  wave: z.number().int().min(1),
});

const storySchema = z.object({
  id: z.string().trim().min(1),
  title: z.string().trim().min(1),
  status: taskStatusSchema.default('pending'),
  tasks: z.array(storyTaskSchema).default([]),
});

const configSchema = z.object({
  maxHardeningWaves: z.number().int().min(1).nullable().default(null),
  maxRetries: z.number().int().min(1).default(DEFAULT_MAX_RETRIES),
  pollIntervalSec: z.number().int().min(1).default(DEFAULT_POLL_INTERVAL_SEC),
  idleTimeoutSec: z.number().int().min(1).default(DEFAULT_IDLE_TIMEOUT_SEC),
});

const runtimeSchema = z.object({
  currentHardeningWave: z.number().int().min(0).default(0),
  consecutiveHardeningNoIssueWaves: z.number().int().min(0).default(0),
  lastInjectedTaskId: z.string().trim().min(1).optional(),
  lastInjectedAt: z.string().trim().min(1).optional(),
});

export const ralphthonPrdSchema = z.object({
  mode: z.literal(RALPHTHON_MODE).default(RALPHTHON_MODE),
  schemaVersion: z.literal(RALPHTHON_SCHEMA_VERSION).default(RALPHTHON_SCHEMA_VERSION),
  project: z.string().trim().min(1),
  phase: phaseSchema.default('development'),
  stories: z.array(storySchema).default([]),
  hardening: z.array(hardeningTaskSchema).default([]),
  config: configSchema.default({
    maxHardeningWaves: null,
    maxRetries: DEFAULT_MAX_RETRIES,
    pollIntervalSec: DEFAULT_POLL_INTERVAL_SEC,
    idleTimeoutSec: DEFAULT_IDLE_TIMEOUT_SEC,
  }),
  runtime: runtimeSchema.default({
    currentHardeningWave: 0,
    consecutiveHardeningNoIssueWaves: 0,
  }),
  createdAt: z.string().trim().min(1).optional(),
  updatedAt: z.string().trim().min(1).optional(),
});

export type RalphthonTaskStatus = z.infer<typeof taskStatusSchema>;
export type RalphthonPhase = z.infer<typeof phaseSchema>;
export type RalphthonStoryTask = z.infer<typeof storyTaskSchema>;
export type RalphthonHardeningTask = z.infer<typeof hardeningTaskSchema>;
export type RalphthonTask = RalphthonStoryTask | RalphthonHardeningTask;
export type RalphthonStory = z.infer<typeof storySchema>;
export type RalphthonConfig = z.infer<typeof configSchema>;
export type RalphthonRuntime = z.infer<typeof runtimeSchema>;
export type RalphthonPrd = z.infer<typeof ralphthonPrdSchema>;
export type RalphthonTaskKind = 'story' | 'hardening';

export interface RalphthonTaskRef {
  kind: RalphthonTaskKind;
  storyId?: string;
  storyTitle?: string;
  task: RalphthonTask;
}

export interface CreateRalphthonPrdInput {
  project: string;
  stories?: RalphthonStory[];
  hardening?: RalphthonHardeningTask[];
  config?: Partial<RalphthonConfig>;
}

function nowIso(): string {
  return new Date().toISOString();
}

function stableStringify(value: unknown): string {
  return JSON.stringify(value, null, 2);
}

export function ralphthonDir(cwd: string): string {
  return join(cwd, '.omx', 'ralphthon');
}

export function canonicalRalphthonPrdPath(cwd: string): string {
  return join(ralphthonDir(cwd), 'prd.json');
}

export function legacyRalphthonPrdPath(cwd: string): string {
  return join(cwd, '.omx', 'prd.json');
}

export function resolveExistingRalphthonPrdPath(cwd: string): string | null {
  const canonicalPath = canonicalRalphthonPrdPath(cwd);
  if (existsSync(canonicalPath)) {
    try {
      parseRalphthonPrd(JSON.parse(readFileSync(canonicalPath, 'utf-8')));
      return canonicalPath;
    } catch {
      return null;
    }
  }

  const legacyPath = legacyRalphthonPrdPath(cwd);
  if (!existsSync(legacyPath)) return null;
  try {
    const raw = JSON.parse(readFileSync(legacyPath, 'utf-8'));
    if (!raw || typeof raw !== 'object' || raw.mode !== RALPHTHON_MODE) return null;
    parseRalphthonPrd(raw);
    return legacyPath;
  } catch {
    return null;
  }
}


export function parseRalphthonPrd(input: unknown): RalphthonPrd {
  const parsed = ralphthonPrdSchema.parse(input);
  return recomputeStoryStatuses({
    ...parsed,
    stories: parsed.stories.map((story: RalphthonStory) => ({
      ...story,
      tasks: [...story.tasks],
    })),
    hardening: [...parsed.hardening],
  });
}

export function createRalphthonPrd(input: CreateRalphthonPrdInput): RalphthonPrd {
  const timestamp = nowIso();
  return parseRalphthonPrd({
    mode: RALPHTHON_MODE,
    schemaVersion: RALPHTHON_SCHEMA_VERSION,
    project: input.project,
    phase: 'development',
    stories: input.stories ?? [],
    hardening: input.hardening ?? [],
    config: {
      maxHardeningWaves: input.config?.maxHardeningWaves ?? null,
      maxRetries: input.config?.maxRetries ?? DEFAULT_MAX_RETRIES,
      pollIntervalSec: input.config?.pollIntervalSec ?? DEFAULT_POLL_INTERVAL_SEC,
      idleTimeoutSec: input.config?.idleTimeoutSec ?? DEFAULT_IDLE_TIMEOUT_SEC,
    },
    runtime: {
      currentHardeningWave: 0,
      consecutiveHardeningNoIssueWaves: 0,
    },
    createdAt: timestamp,
    updatedAt: timestamp,
  });
}

export async function readRalphthonPrd(cwd: string): Promise<RalphthonPrd | null> {
  const prdPath = resolveExistingRalphthonPrdPath(cwd);
  if (!prdPath) return null;
  try {
    return parseRalphthonPrd(JSON.parse(await readFile(prdPath, 'utf-8')));
  } catch {
    return null;
  }
}

export async function writeRalphthonPrd(cwd: string, prd: RalphthonPrd): Promise<string> {
  const normalized = recomputeStoryStatuses({
    ...prd,
    updatedAt: nowIso(),
    createdAt: prd.createdAt ?? nowIso(),
  });
  const path = canonicalRalphthonPrdPath(cwd);
  await mkdir(ralphthonDir(cwd), { recursive: true });
  await writeFile(path, `${stableStringify(normalized)}\n`);
  return path;
}

export function recomputeStoryStatuses(prd: RalphthonPrd): RalphthonPrd {
  const stories = prd.stories.map((story: RalphthonStory) => {
    const statuses = story.tasks.map((task: RalphthonStoryTask) => task.status);
    let status: RalphthonTaskStatus = 'pending';
    if (statuses.length === 0) {
      status = 'done';
    } else if (statuses.every((entry: RalphthonTaskStatus) => entry === 'done')) {
      status = 'done';
    } else if (statuses.some((entry: RalphthonTaskStatus) => entry === 'in_progress')) {
      status = 'in_progress';
    } else if (statuses.every((entry: RalphthonTaskStatus) => entry === 'failed')) {
      status = 'failed';
    } else if (statuses.some((entry: RalphthonTaskStatus) => entry === 'pending')) {
      status = 'pending';
    } else if (statuses.some((entry: RalphthonTaskStatus) => entry === 'failed')) {
      status = 'failed';
    }
    return { ...story, status, tasks: [...story.tasks] };
  });

  return {
    ...prd,
    stories,
    hardening: [...prd.hardening],
    runtime: { ...prd.runtime },
  };
}

export function listRalphthonTasks(prd: RalphthonPrd): RalphthonTaskRef[] {
  const refs: RalphthonTaskRef[] = [];
  for (const story of prd.stories) {
    for (const task of story.tasks) {
      refs.push({
        kind: 'story',
        storyId: story.id,
        storyTitle: story.title,
        task,
      });
    }
  }
  for (const task of prd.hardening) {
    refs.push({ kind: 'hardening', task });
  }
  return refs;
}

export function findTaskRef(prd: RalphthonPrd, taskId: string): RalphthonTaskRef | null {
  return listRalphthonTasks(prd).find((entry: RalphthonTaskRef) => entry.task.id === taskId) ?? null;
}

export function allStoryTasksTerminal(prd: RalphthonPrd): boolean {
  return prd.stories.every((story: RalphthonStory) => story.tasks.every((task: RalphthonStoryTask) => task.status === 'done' || task.status === 'failed'));
}

export function allStoryTasksDone(prd: RalphthonPrd): boolean {
  return prd.stories.every((story: RalphthonStory) => story.tasks.every((task: RalphthonStoryTask) => task.status === 'done'));
}

export function hasPendingWork(prd: RalphthonPrd): boolean {
  return listRalphthonTasks(prd).some((entry: RalphthonTaskRef) => entry.task.status === 'pending' || entry.task.status === 'in_progress');
}

export function nextPendingTask(prd: RalphthonPrd): RalphthonTaskRef | null {
  if (prd.phase === 'development') {
    for (const story of prd.stories) {
      const task = story.tasks.find((entry: RalphthonStoryTask) => entry.status === 'pending');
      if (task) {
        return {
          kind: 'story',
          storyId: story.id,
          storyTitle: story.title,
          task,
        };
      }
    }
    return null;
  }

  const hardeningTask = prd.hardening.find((entry: RalphthonHardeningTask) => entry.status === 'pending');
  return hardeningTask ? { kind: 'hardening', task: hardeningTask } : null;
}

export function markTaskStatus(
  prd: RalphthonPrd,
  taskId: string,
  status: RalphthonTaskStatus,
  options: {
    incrementRetries?: boolean;
    lastError?: string;
    timestamp?: string;
  } = {},
): RalphthonPrd {
  const timestamp = options.timestamp ?? nowIso();
  const storyTasks = prd.stories.map((story: RalphthonStory) => ({
    ...story,
    tasks: story.tasks.map((task: RalphthonStoryTask) => {
      if (task.id !== taskId) return task;
      return {
        ...task,
        status,
        retries: options.incrementRetries ? task.retries + 1 : task.retries,
        ...(options.lastError ? { lastError: options.lastError } : {}),
        ...(status === 'in_progress' ? { startedAt: timestamp } : {}),
        ...(status === 'done' || status === 'failed' ? { completedAt: timestamp } : {}),
      };
    }),
  }));

  const hardening = prd.hardening.map((task: RalphthonHardeningTask) => {
    if (task.id !== taskId) return task;
    return {
      ...task,
      status,
      retries: options.incrementRetries ? task.retries + 1 : task.retries,
      ...(options.lastError ? { lastError: options.lastError } : {}),
      ...(status === 'in_progress' ? { startedAt: timestamp } : {}),
      ...(status === 'done' || status === 'failed' ? { completedAt: timestamp } : {}),
    };
  });

  return recomputeStoryStatuses({
    ...prd,
    stories: storyTasks,
    hardening,
    updatedAt: timestamp,
  });
}

export function ensureHardeningPhase(prd: RalphthonPrd): RalphthonPrd {
  if (prd.phase !== 'development') return prd;
  if (!allStoryTasksTerminal(prd)) return prd;
  return {
    ...prd,
    phase: 'hardening',
    runtime: {
      ...prd.runtime,
      currentHardeningWave: Math.max(0, prd.runtime.currentHardeningWave),
    },
    updatedAt: nowIso(),
  };
}

export function appendHardeningTasks(
  prd: RalphthonPrd,
  descriptions: string[],
  wave: number,
): RalphthonPrd {
  const existingIds = new Set(listRalphthonTasks(prd).map((entry: RalphthonTaskRef) => entry.task.id));
  const hardening = [...prd.hardening];
  let counter = hardening.length + 1;
  for (const description of descriptions) {
    const trimmed = description.trim();
    if (!trimmed) continue;
    let id = `H${counter}`;
    while (existingIds.has(id)) {
      counter += 1;
      id = `H${counter}`;
    }
    existingIds.add(id);
    hardening.push({
      id,
      desc: trimmed,
      status: 'pending',
      retries: 0,
      wave,
    });
    counter += 1;
  }

  return {
    ...prd,
    phase: 'hardening',
    hardening,
    runtime: {
      ...prd.runtime,
      currentHardeningWave: Math.max(prd.runtime.currentHardeningWave, wave),
      consecutiveHardeningNoIssueWaves: descriptions.length > 0 ? 0 : prd.runtime.consecutiveHardeningNoIssueWaves,
    },
    updatedAt: nowIso(),
  };
}

export function recordHardeningWaveResult(prd: RalphthonPrd, issuesFound: number): RalphthonPrd {
  const nextWave = Math.max(1, prd.runtime.currentHardeningWave + 1);
  return {
    ...prd,
    phase: 'hardening',
    runtime: {
      ...prd.runtime,
      currentHardeningWave: nextWave,
      consecutiveHardeningNoIssueWaves: issuesFound > 0
        ? 0
        : prd.runtime.consecutiveHardeningNoIssueWaves + 1,
    },
    updatedAt: nowIso(),
  };
}

export function shouldTerminateHardening(prd: RalphthonPrd): boolean {
  if (prd.phase !== 'hardening') return false;
  const hasInFlightHardening = prd.hardening.some((task: RalphthonHardeningTask) => task.status === 'pending' || task.status === 'in_progress');
  if (hasInFlightHardening) return false;
  if (prd.runtime.consecutiveHardeningNoIssueWaves >= 3) return true;
  const maxWaves = prd.config.maxHardeningWaves;
  if (typeof maxWaves === 'number' && prd.runtime.currentHardeningWave >= maxWaves) {
    return true;
  }
  return false;
}

export function completeRalphthonPrd(prd: RalphthonPrd): RalphthonPrd {
  return {
    ...prd,
    phase: 'complete',
    updatedAt: nowIso(),
  };
}
