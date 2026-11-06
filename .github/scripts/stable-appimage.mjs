import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [tag] = process.argv.slice(2);
const repo = process.env.GITHUB_REPOSITORY;
if (!tag || !repo) throw new Error('Release tag and GITHUB_REPOSITORY are required');
const gh = (...args) => execFileSync('gh', args, { encoding: 'utf8' });
const release = JSON.parse(gh('api', `repos/${repo}/releases/tags/${encodeURIComponent(tag)}`));
const version = tag.replace(/^v/, '');
const renamed = new Map();

for (const asset of release.assets) {
  if (!/\.AppImage(?:\.sig)?$/.test(asset.name)) continue;
  const name = asset.name.replace(`_${version}_`, '_');
  if (name === asset.name) continue;
  const updated = JSON.parse(gh('api', '--method', 'PATCH',
    `repos/${repo}/releases/assets/${asset.id}`, '-f', `name=${name}`));
  renamed.set(asset.browser_download_url, updated.browser_download_url);
  console.log(`${asset.name} -> ${name}`);
}

// Run after all matrix jobs: no concurrent updater-manifest uploads remain.
const dir = mkdtempSync(join(tmpdir(), 'stream-assist-release-'));
try {
  const file = join(dir, 'latest.json');
  gh('release', 'download', tag, '--repo', repo, '--pattern', 'latest.json', '--output', file);
  const manifest = JSON.parse(readFileSync(file, 'utf8'));
  let changed = false;
  for (const platform of Object.values(manifest.platforms ?? {})) {
    if (!platform.url) continue;
    const url = new URL(platform.url);
    if (!url.pathname.endsWith('.AppImage')) continue;
    // Also repairs a manifest left unchanged by a previously interrupted run.
    const replacement = renamed.get(platform.url);
    const name = decodeURIComponent(url.pathname.split('/').pop());
    const stable = name.replace(`_${version}_`, '_');
    if (replacement) { platform.url = replacement; changed = true; }
    else if (stable !== name) {
      url.pathname = url.pathname.slice(0, url.pathname.lastIndexOf('/') + 1) + encodeURIComponent(stable);
      platform.url = url.href;
      changed = true;
    }
  }
  if (changed) {
    writeFileSync(file, JSON.stringify(manifest, null, 2) + '\n');
    gh('release', 'upload', tag, file, '--repo', repo, '--clobber');
    console.log('Updated AppImage URLs in latest.json');
  }
} finally {
  rmSync(dir, { recursive: true, force: true });
}
