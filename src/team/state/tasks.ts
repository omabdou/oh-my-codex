import { randomUUID } from 'crypto';
import { join } from 'path';
import { existsSync } from 'fs';
import { readFile, readdir } from 'fs/promises';

export type TeamTaskStatus = 'pending' | 'blocked' | 'in_progress' | 'completed' | 'failed';

export interface TeamTask {
  id: string;
  subject: string;
  description: string;
  status: TeamTaskStatus;
  requires_code_change?: boolean;
  role?: string;
  owner?: string;
  result?: string;
  error?: string;
  blocked_by?: string[];
  depends_on?: string[];
  version?: number;
  claim?: TeamTaskClaim;
  created_at: string;
  completed_at?: string;
}

export interface TeamTaskClaim {
  owner: string;
  token: string;
  leased_until: string;
}

export interface TeamTaskV2 extends TeamTask {
  version: number;
}

export type TaskReadiness =
  | { ready: true }
  | { ready: false; reason: 'blocked_dependency'; dependencies: string[] };

export type ClaimTaskResult =
  | { ok: true; task: TeamTaskV2; claimToken: string }
  | { ok: false; error: 'claim_conflict' | 'blocked_dependency' | 'task_not_found' | 'already_terminal' | 'worker_not_found'; dependencies?: string[] };

export type TransitionTaskResult =
  | { ok: true; task: TeamTaskV2 }
  | { ok: false; error: 'claim_conflict' | 'invalid_transition' | 'task_not_found' | 'already_terminal' | 'lease_expired' };

export type ReleaseTaskClaimResult =
  | { ok: true; task: TeamTaskV2 }
  | { ok: false; error: 'claim_conflict' | 'task_not_found' | 'already_terminal' | 'lease_expired' };

export interface TeamMonitorSnapshotState {
  taskStatusById: Record<string, string>;
  workerAliveByName: Record<string, boolean>;
  workerStateByName: Record<string, string>;
  workerTurnCountByName: Record<string, number>;
  workerTaskIdByName: Record<string, string>;
  mailboxNotifiedByMessageId: Record<string, string>;
  completedEventTaskIds: Record<string, boolean>;
  monitorTimings?: {
    list_tasks_ms: number;
    worker_scan_ms: number;
    mailbox_delivery_ms: number;
    total_ms: number;
    updated_at: string;
  };
}

interface TaskReadDeps {
  readTask: (teamName: string, taskId: string, cwd: string) => Promise<TeamTask | null>;
}

export async function computeTaskReadiness(
  teamName: string,
  taskId: string,
  cwd: string,
  deps: TaskReadDeps,
): Promise<TaskReadiness> {
  const task = await deps.readTask(teamName, taskId, cwd);
  if (!task) return { ready: false, reason: 'blocked_dependency', dependencies: [] };

  const depIds = task.depends_on ?? task.blocked_by ?? [];
  if (depIds.length === 0) return { ready: true };

  const depTasks = await Promise.all(depIds.map((depId) => deps.readTask(teamName, depId, cwd)));
  const incomplete = depIds.filter((_, idx) => depTasks[idx]?.status !== 'completed');
  if (incomplete.length > 0) return { ready: false, reason: 'blocked_dependency', dependencies: incomplete };

  return { ready: true };
}

interface ClaimTaskDeps extends TaskReadDeps {
  teamName: string;
  cwd: string;
  readTeamConfig: (teamName: string, cwd: string) => Promise<{ workers: Array<{ name: string }> } | null>;
  withTaskClaimLock: <T>(teamName: string, taskId: string, cwd: string, fn: () => Promise<T>) => Promise<{ ok: true; value: T } | { ok: false }>;
  normalizeTask: (task: TeamTask) => TeamTaskV2;
  isTerminalTaskStatus: (status: TeamTaskStatus) => boolean;
  taskFilePath: (teamName: string, taskId: string, cwd: string) => string;
  writeAtomic: (path: string, data: string) => Promise<void>;
}

export async function claimTask(
  taskId: string,
  workerName: string,
  expectedVersion: number | null,
  deps: ClaimTaskDeps,
): Promise<ClaimTaskResult> {
  const cfg = await deps.readTeamConfig(deps.teamName, deps.cwd);
  if (!cfg || !cfg.workers.some((w) => w.name === workerName)) return { ok: false, error: 'worker_not_found' };

  const existing = await deps.readTask(deps.teamName, taskId, deps.cwd);
  if (!existing) return { ok: false, error: 'task_not_found' };

  const readiness = await computeTaskReadiness(deps.teamName, taskId, deps.cwd, deps);
  if (!readiness.ready) return { ok: false, error: 'blocked_dependency', dependencies: readiness.dependencies };

  const lock = await deps.withTaskClaimLock(deps.teamName, taskId, deps.cwd, async () => {
    const current = await deps.readTask(deps.teamName, taskId, deps.cwd);
    if (!current) return { ok: false as const, error: 'task_not_found' as const };

    const v = deps.normalizeTask(current);
    if (expectedVersion !== null && v.version !== expectedVersion) return { ok: false as const, error: 'claim_conflict' as const };

    const readinessAfterLock = await computeTaskReadiness(deps.teamName, taskId, deps.cwd, deps);
    if (!readinessAfterLock.ready) return { ok: false as const, error: 'blocked_dependency' as const, dependencies: readinessAfterLock.dependencies };

    if (deps.isTerminalTaskStatus(v.status)) return { ok: false as const, error: 'already_terminal' as const };
    if (v.status === 'in_progress') return { ok: false as const, error: 'claim_conflict' as const };

    if (v.status === 'pending' || v.status === 'blocked') {
      if (v.claim) return { ok: false as const, error: 'claim_conflict' as const };
      if (v.owner && v.owner !== workerName) return { ok: false as const, error: 'claim_conflict' as const };
    }

    const claimToken = randomUUID();
    const updated: TeamTaskV2 = {
      ...v,
      status: 'in_progress',
      owner: workerName,
      claim: { owner: workerName, token: claimToken, leased_until: new Date(Date.now() + 15 * 60 * 1000).toISOString() },
      version: v.version + 1,
    };

    await deps.writeAtomic(deps.taskFilePath(deps.teamName, taskId, deps.cwd), JSON.stringify(updated, null, 2));
    return { ok: true as const, task: updated, claimToken };
  });

  if (!lock.ok) return { ok: false, error: 'claim_conflict' };
  return lock.value;
}

interface TransitionDeps extends ClaimTaskDeps {
  canTransitionTaskStatus: (from: TeamTaskStatus, to: TeamTaskStatus) => boolean;
  appendTeamEvent: (
    teamName: string,
    event: {
      type: 'task_completed' | 'task_failed';
      worker: string;
      task_id?: string;
      message_id?: string | null;
      reason?: string;
    },
    cwd: string,
  ) => Promise<unknown>;
  readMonitorSnapshot: (teamName: string, cwd: string) => Promise<TeamMonitorSnapshotState | null>;
  writeMonitorSnapshot: (teamName: string, snapshot: TeamMonitorSnapshotState, cwd: string) => Promise<void>;
}

export async function transitionTaskStatus(
  taskId: string,
  from: TeamTaskStatus,
  to: TeamTaskStatus,
  claimToken: string,
  deps: TransitionDeps,
): Promise<TransitionTaskResult> {
  if (!deps.canTransitionTaskStatus(from, to)) return { ok: false, error: 'invalid_transition' };

  const lock = await deps.withTaskClaimLock(deps.teamName, taskId, deps.cwd, async () => {
    const current = await deps.readTask(deps.teamName, taskId, deps.cwd);
    if (!current) return { ok: false as const, error: 'task_not_found' as const };

    const v = deps.normalizeTask(current);
    if (deps.isTerminalTaskStatus(v.status)) return { ok: false as const, error: 'already_terminal' as const };
    if (!deps.canTransitionTaskStatus(v.status, to)) return { ok: false as const, error: 'invalid_transition' as const };
    if (v.status !== from) return { ok: false as const, error: 'invalid_transition' as const };

    if (!v.owner || !v.claim || v.claim.owner !== v.owner || v.claim.token !== claimToken) {
      return { ok: false as const, error: 'claim_conflict' as const };
    }
    if (new Date(v.claim.leased_until) <= new Date()) return { ok: false as const, error: 'lease_expired' as const };

    const updated: TeamTaskV2 = {
      ...v,
      status: to,
      completed_at: new Date().toISOString(),
      claim: undefined,
      version: v.version + 1,
    };
    await deps.writeAtomic(deps.taskFilePath(deps.teamName, taskId, deps.cwd), JSON.stringify(updated, null, 2));

    if (to === 'completed') {
      await deps.appendTeamEvent(
        deps.teamName,
        { type: 'task_completed', worker: updated.owner || 'unknown', task_id: updated.id, message_id: null, reason: undefined },
        deps.cwd,
      );
    } else if (to === 'failed') {
      await deps.appendTeamEvent(
        deps.teamName,
        { type: 'task_failed', worker: updated.owner || 'unknown', task_id: updated.id, message_id: null, reason: updated.error || 'task_failed' },
        deps.cwd,
      );
    }

    return { ok: true as const, task: updated };
  });

  if (!lock.ok) return { ok: false, error: 'claim_conflict' };

  if (to === 'completed') {
    const existing = await deps.readMonitorSnapshot(deps.teamName, deps.cwd);
    const updated: TeamMonitorSnapshotState = existing
      ? { ...existing, completedEventTaskIds: { ...(existing.completedEventTaskIds ?? {}), [taskId]: true } }
      : {
          taskStatusById: {},
          workerAliveByName: {},
          workerStateByName: {},
          workerTurnCountByName: {},
          workerTaskIdByName: {},
          mailboxNotifiedByMessageId: {},
          completedEventTaskIds: { [taskId]: true },
        };
    await deps.writeMonitorSnapshot(deps.teamName, updated, deps.cwd);
  }

  return lock.value;
}

interface ReleaseDeps extends ClaimTaskDeps {}

export async function releaseTaskClaim(
  taskId: string,
  claimToken: string,
  _workerName: string,
  deps: ReleaseDeps,
): Promise<ReleaseTaskClaimResult> {
  const lock = await deps.withTaskClaimLock(deps.teamName, taskId, deps.cwd, async () => {
    const current = await deps.readTask(deps.teamName, taskId, deps.cwd);
    if (!current) return { ok: false as const, error: 'task_not_found' as const };

    const v = deps.normalizeTask(current);
    if (v.status === 'pending' && !v.claim && !v.owner) return { ok: true as const, task: v };
    if (v.status === 'completed' || v.status === 'failed') return { ok: false as const, error: 'already_terminal' as const };

    if (!v.owner || !v.claim || v.claim.owner !== v.owner || v.claim.token !== claimToken) {
      return { ok: false as const, error: 'claim_conflict' as const };
    }
    if (new Date(v.claim.leased_until) <= new Date()) return { ok: false as const, error: 'lease_expired' as const };

    const updated: TeamTaskV2 = {
      ...v,
      status: 'pending',
      owner: undefined,
      claim: undefined,
      version: v.version + 1,
    };
    await deps.writeAtomic(deps.taskFilePath(deps.teamName, taskId, deps.cwd), JSON.stringify(updated, null, 2));
    return { ok: true as const, task: updated };
  });

  if (!lock.ok) return { ok: false, error: 'claim_conflict' };
  return lock.value;
}

export async function listTasks(
  teamName: string,
  cwd: string,
  deps: {
    teamDir: (teamName: string, cwd: string) => string;
    isTeamTask: (value: unknown) => value is TeamTask;
    normalizeTask: (task: TeamTask) => TeamTaskV2;
  },
): Promise<TeamTask[]> {
  const tasksRoot = join(deps.teamDir(teamName, cwd), 'tasks');
  if (!existsSync(tasksRoot)) return [];

  const entries = await readdir(tasksRoot, { withFileTypes: true });
  const matched = entries.flatMap((entry) => {
    if (!entry.isFile()) return [];
    const match = /^task-(\d+)\.json$/.exec(entry.name);
    if (!match) return [];
    return [{ id: match[1], fileName: entry.name }];
  });

  const loaded = await Promise.all(
    matched.map(async ({ id, fileName }) => {
      try {
        const raw = await readFile(join(tasksRoot, fileName), 'utf8');
        const parsed = JSON.parse(raw) as unknown;
        if (!deps.isTeamTask(parsed)) return null;
        const normalized = deps.normalizeTask(parsed);
        if (normalized.id !== id) return null;
        return normalized;
      } catch {
        return null;
      }
    }),
  );

  const tasks: TeamTaskV2[] = [];
  for (const task of loaded) {
    if (task) tasks.push(task);
  }
  tasks.sort((a, b) => Number(a.id) - Number(b.id));
  return tasks;
}
