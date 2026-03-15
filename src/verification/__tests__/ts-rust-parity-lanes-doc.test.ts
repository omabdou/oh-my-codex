import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

function read(relativePath: string): string {
  return readFileSync(join(process.cwd(), relativePath), 'utf8');
}

describe('TS→Rust parity lanes doc contract', () => {
  it('documents the five parity lanes with TypeScript as SSOT and Rust boundary status', () => {
    const relativePath = 'docs/reference/ts-rust-parity-lanes.md';
    const fullPath = join(process.cwd(), relativePath);

    assert.equal(existsSync(fullPath), true, `missing parity lanes doc: ${relativePath}`);

    const doc = read(relativePath);

    assert.match(doc, /TypeScript remains the source of truth/i);
    assert.match(doc, /## Lane 1 — startup contract \/ runtime parity/);
    assert.match(doc, /## Lane 2 — team runtime \/ tmux control-plane parity/);
    assert.match(doc, /## Lane 3 — HUD behavior parity/);
    assert.match(doc, /## Lane 4 — watcher \/ reply-listener parity/);
    assert.match(doc, /## Lane 5 — MCP \/ CLI boundary mapping and truthfulness/);

    assert.match(doc, /src\/team\/runtime\.ts:725/);
    assert.match(doc, /src\/team\/tmux-session\.ts:760/);
    assert.match(doc, /src\/hud\/index\.ts/);
    assert.match(doc, /src\/notifications\/reply-listener\.ts:446/);
    assert.match(doc, /src\/mcp\/team-server\.ts:327-333/);

    assert.match(doc, /crates\/omx-runtime\/src\/runtime_run\.rs:255/);
    assert.match(doc, /crates\/omx-runtime\/src\/hud\.rs/);
    assert.match(doc, /crates\/omx-runtime\/src\/watchers\.rs/);
    assert.match(doc, /crates\/omx-runtime\/src\/reply_listener\.rs/);
    assert.match(doc, /npm run build -- --pretty false` → PASS/i);
    assert.match(doc, /node --test dist\/verification\/__tests__\/phase1-runtime-surface-parity\.test\.js` → PASS/i);
    assert.match(doc, /node --test dist\/verification\/__tests__\/ts-rust-parity-lanes-doc\.test\.js` → PASS/i);

    assert.match(doc, /Safe statements/);
    assert.match(doc, /Unsafe statements/);
    assert.match(doc, /Recommended next review order/);
  });

  it('keeps the parity doc and topology source aligned on bounded ownership claims', () => {
    const parityDoc = read('docs/reference/ts-rust-parity-lanes.md');
    const topologySource = read('crates/omx-runtime/src/topology.rs');

    assert.match(parityDoc, /expired-claim reclaim, ready-work rebalance, mailbox-delivery state tracking, structured verification-evidence gating, monitor snapshot persistence, and bounded linked-Ralph terminal-state sync/i);
    assert.match(parityDoc, /no full shutdown request \/ worker-ACK \/ linked-Ralph event parity/i);

    assert.match(topologySource, /guarded hud-watch launch seam/);
    assert.match(topologySource, /bounded monitor\/shutdown slices/);
    assert.match(topologySource, /TypeScript SSOT/);
    assert.match(topologySource, /parity-incomplete relative to the TypeScript watcher scripts/);

    const ownershipRowsBlock = topologySource.match(/const OWNERSHIP_ROWS:[\s\S]*?\];/)?.[0] ?? '';
    assert.doesNotMatch(ownershipRowsBlock, /Rust-native team lifecycle, polling, stale-worker detection, and shutdown ownership\./);
    assert.doesNotMatch(ownershipRowsBlock, /Fallback watcher, derived watcher, and reply polling loops move behind one native lifecycle owner\./);
  });
});
