import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

function readRepoFile(...parts: string[]): string {
  return readFileSync(join(process.cwd(), ...parts), 'utf-8');
}

describe('native runtime documentation contract', () => {
  it('README advertises the native omx bundle as the primary runtime path', () => {
    const readme = readRepoFile('README.md');
    assert.match(readme, /Rust-native release path/i);
    assert.match(readme, /Primary install path:/i);
    assert.match(readme, /launcher\/downloader shim only/i);
    assert.doesNotMatch(readme, /- Node\.js >= 20 \(CI validates Node 20 and current LTS, currently Node 22\)/);
  });

  it('static docs treat npm as transitional and native omx as the normal path', () => {
    const home = readRepoFile('docs', 'index.html');
    const gettingStarted = readRepoFile('docs', 'getting-started.html');

    assert.match(home, /install the native <code>omx<\/code> bundle/i);
    assert.match(home, /launcher\/downloader shim only/i);
    assert.match(gettingStarted, /primary path: install the native omx bundle/i);
    assert.match(gettingStarted, /Node\.js 20\+ only if you are using the temporary npm shim/i);
  });
});
