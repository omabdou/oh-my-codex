import { createHash } from 'crypto';
import { existsSync, type Dirent } from 'fs';
import { mkdir, readFile, readdir, writeFile } from 'fs/promises';
import { join } from 'path';
import { getStateDir } from '../mcp/state-paths.js';
import { normalizeRalphPrdPolicy, type RalphPrdPolicy } from './contract.js';

const LEGACY_PRD_PATH = '.omx/prd.json';
const LEGACY_PROGRESS_PATH = '.omx/progress.txt';
const PRD_PREFIX = 'prd-';
const PRD_SUFFIX = '.md';

export interface RalphCanonicalArtifacts {
  canonicalPrdPath?: string;
  canonicalProgressPath: string;
  migratedPrd: boolean;
  migratedProgress: boolean;
  scaffoldedPrd: boolean;
}

export interface EnsureCanonicalRalphArtifactsOptions {
  ensurePrd?: boolean;
  prdPolicy?: RalphPrdPolicy | string;
  taskDescription?: string;
}

function sha256(text: string): string {
  return createHash('sha256').update(text).digest('hex');
}

function slugify(raw: string): string {
  return raw
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/-+/g, '-')
    .replace(/^-|-$/g, '')
    .slice(0, 48) || 'legacy';
}

function stableJson(value: unknown): string {
  if (value == null || typeof value !== 'object') {
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableJson(item)).join(',')}]`;
  }
  const entries = Object.entries(value as Record<string, unknown>)
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([key, val]) => `${JSON.stringify(key)}:${stableJson(val)}`);
  return `{${entries.join(',')}}`;
}

function stableJsonPretty(value: unknown): string {
  return JSON.stringify(JSON.parse(stableJson(value)), null, 2);
}

function resolveLegacyPrdTitle(parsed: Record<string, unknown>): string {
  const candidates = [
    parsed.project,
    parsed.title,
    parsed.branchName,
    parsed.description,
  ];
  for (const candidate of candidates) {
    if (typeof candidate === 'string' && candidate.trim() !== '') {
      return candidate.trim();
    }
  }
  return 'Legacy Ralph PRD';
}

async function listCanonicalPrdFiles(cwd: string): Promise<string[]> {
  const plansDir = join(cwd, '.omx', 'plans');
  if (!existsSync(plansDir)) return [];
  const entries = await readdir(plansDir, { withFileTypes: true }).catch(() => [] as Dirent[]);
  return entries
    .filter((entry) => entry.isFile() && entry.name.startsWith(PRD_PREFIX) && entry.name.endsWith(PRD_SUFFIX))
    .map((entry) => entry.name)
    .sort()
    .map((file) => join(plansDir, file));
}

export function resolveAvailableCanonicalPrdPath(plansDir: string, baseSlug: string): string {
  let candidate = join(plansDir, `${PRD_PREFIX}${baseSlug}${PRD_SUFFIX}`);
  let suffix = 1;
  while (existsSync(candidate)) {
    candidate = join(plansDir, `${PRD_PREFIX}${baseSlug}-${suffix}${PRD_SUFFIX}`);
    suffix += 1;
  }
  return candidate;
}

function splitProgressLines(content: string): string[] {
  return content
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

async function writeMigrationMarker(
  cwd: string,
  patch: Record<string, unknown>,
): Promise<void> {
  const markerPath = join(cwd, '.omx', 'plans', 'ralph-migration-marker.json');
  let existing: Record<string, unknown> = {};
  if (existsSync(markerPath)) {
    try {
      existing = JSON.parse(await readFile(markerPath, 'utf-8')) as Record<string, unknown>;
    } catch {
      existing = {};
    }
  }
  const merged = {
    compatibility_window: 'legacy-read-only-one-release-cycle',
    ...existing,
    ...patch,
  };
  await writeFile(markerPath, `${stableJsonPretty(merged)}\n`);
}

async function migrateLegacyPrdIfNeeded(
  cwd: string,
  existingCanonicalPrd: string | undefined,
): Promise<{ canonicalPrdPath?: string; migrated: boolean }> {
  if (existingCanonicalPrd) {
    return { canonicalPrdPath: existingCanonicalPrd, migrated: false };
  }

  const legacyPrdPath = join(cwd, LEGACY_PRD_PATH);
  if (!existsSync(legacyPrdPath)) {
    return { canonicalPrdPath: undefined, migrated: false };
  }

  const legacyRaw = await readFile(legacyPrdPath, 'utf-8');
  let legacyParsed: Record<string, unknown> = {};
  try {
    legacyParsed = JSON.parse(legacyRaw) as Record<string, unknown>;
  } catch {
    legacyParsed = { parse_error: 'invalid_json', raw: legacyRaw };
  }

  const plansDir = join(cwd, '.omx', 'plans');
  await mkdir(plansDir, { recursive: true });

  const title = resolveLegacyPrdTitle(legacyParsed);
  const baseSlug = slugify(title);
  const canonicalPrdPath = resolveAvailableCanonicalPrdPath(plansDir, baseSlug);

  const markdown = [
    `# ${title}`,
    '',
    '> Migrated from legacy `.omx/prd.json` (read-only compatibility import).',
    '',
    '## Migration Marker',
    `- Source: \`${LEGACY_PRD_PATH}\``,
    `- Source SHA256: \`${sha256(legacyRaw)}\``,
    '- Strategy: one-way conversion to canonical PRD markdown',
    '',
    '## Legacy Snapshot',
    '```json',
    stableJsonPretty(legacyParsed),
    '```',
    '',
  ].join('\n');
  await writeFile(canonicalPrdPath, markdown);

  await writeMigrationMarker(cwd, {
    prd_migration: {
      source: LEGACY_PRD_PATH,
      source_sha256: sha256(legacyRaw),
      canonical_path: canonicalPrdPath,
      strategy: 'one-way-read-only',
    },
  });

  return { canonicalPrdPath, migrated: true };
}

function resolveScaffoldTitle(taskDescription?: string): string {
  const trimmed = taskDescription?.trim();
  if (!trimmed) return 'Ralph Session';
  return trimmed.length > 120 ? `${trimmed.slice(0, 117)}...` : trimmed;
}

function buildCanonicalPrdScaffold(title: string): string {
  return [
    `# PRD: ${title}`,
    '',
    '## Context',
    '- Problem statement:',
    '- Scope and assumptions:',
    '',
    '## Work Objectives',
    '- Objective 1:',
    '- Objective 2:',
    '',
    '## Must Have',
    '- [ ]',
    '',
    '## Must NOT Have',
    '- [ ]',
    '',
    '## Acceptance Criteria',
    '- [ ] Build/tests pass',
    '- [ ] Behavior validated end-to-end',
    '',
  ].join('\n');
}

async function createCanonicalPrdScaffoldIfNeeded(
  cwd: string,
  existingCanonicalPrd: string | undefined,
  options: EnsureCanonicalRalphArtifactsOptions,
): Promise<{ canonicalPrdPath?: string; scaffolded: boolean }> {
  if (existingCanonicalPrd) {
    return { canonicalPrdPath: existingCanonicalPrd, scaffolded: false };
  }

  const prdPolicy = normalizeRalphPrdPolicy(options.prdPolicy).policy;
  const ensurePrd = options.ensurePrd === true;
  if (!ensurePrd || prdPolicy === 'opt_out') {
    return { canonicalPrdPath: undefined, scaffolded: false };
  }

  const legacyPrdPath = join(cwd, LEGACY_PRD_PATH);
  if (existsSync(legacyPrdPath)) {
    return { canonicalPrdPath: undefined, scaffolded: false };
  }

  const plansDir = join(cwd, '.omx', 'plans');
  await mkdir(plansDir, { recursive: true });

  const title = resolveScaffoldTitle(options.taskDescription);
  const baseSlug = slugify(options.taskDescription || 'ralph-session');
  const canonicalPrdPath = resolveAvailableCanonicalPrdPath(plansDir, baseSlug);
  await writeFile(canonicalPrdPath, buildCanonicalPrdScaffold(title), 'utf-8');
  return { canonicalPrdPath, scaffolded: true };
}

async function migrateLegacyProgressIfNeeded(
  cwd: string,
  canonicalProgressPath: string,
): Promise<boolean> {
  if (existsSync(canonicalProgressPath)) return false;

  const legacyProgressPath = join(cwd, LEGACY_PROGRESS_PATH);
  if (!existsSync(legacyProgressPath)) return false;

  const raw = await readFile(legacyProgressPath, 'utf-8');
  const lines = splitProgressLines(raw);
  const payload = {
    schema_version: 1,
    source: LEGACY_PROGRESS_PATH,
    source_sha256: sha256(raw),
    strategy: 'one-way-read-only',
    entries: lines.map((line, index) => ({
      index: index + 1,
      text: line,
    })),
  };
  await mkdir(join(canonicalProgressPath, '..'), { recursive: true });
  await writeFile(canonicalProgressPath, `${stableJsonPretty(payload)}\n`);

  await writeMigrationMarker(cwd, {
    progress_migration: {
      source: LEGACY_PROGRESS_PATH,
      source_sha256: sha256(raw),
      canonical_path: canonicalProgressPath,
      imported_entries: lines.length,
      strategy: 'one-way-read-only',
    },
  });
  return true;
}

export async function ensureCanonicalRalphArtifacts(
  cwd: string,
  sessionId?: string,
  options: EnsureCanonicalRalphArtifactsOptions = {},
): Promise<RalphCanonicalArtifacts> {
  const canonicalProgressPath = join(getStateDir(cwd, sessionId), 'ralph-progress.json');
  await mkdir(join(cwd, '.omx', 'plans'), { recursive: true });
  await mkdir(getStateDir(cwd, sessionId), { recursive: true });

  const canonicalPrdFiles = await listCanonicalPrdFiles(cwd);
  const migratedPrdResult = await migrateLegacyPrdIfNeeded(cwd, canonicalPrdFiles[0]);
  const scaffoldResult = await createCanonicalPrdScaffoldIfNeeded(cwd, migratedPrdResult.canonicalPrdPath, options);
  const migratedProgress = await migrateLegacyProgressIfNeeded(cwd, canonicalProgressPath);

  return {
    canonicalPrdPath: scaffoldResult.canonicalPrdPath,
    canonicalProgressPath,
    migratedPrd: migratedPrdResult.migrated,
    migratedProgress,
    scaffoldedPrd: scaffoldResult.scaffolded,
  };
}
