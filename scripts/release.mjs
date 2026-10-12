// Signed release build + the latest.json the in-app updater reads.
//
//   npm run release -- https://github.com/<you>/<repo>/releases/download/v0.2.0
//
// The argument is the folder URL the installer will be uploaded to. Bump "version" in
// src-tauri/tauri.conf.json first. The signing key lives outside the repo, in ~/.tauri/fix-ui.key.
import { execSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const base = process.argv[2]?.replace(/\/$/, '');
if (!base?.startsWith('https://')) {
  console.error('usage: npm run release -- <https URL of the folder the installer will be uploaded to>');
  process.exit(1);
}

const key = path.join(os.homedir(), '.tauri', 'fix-ui.key');
execSync('npx tauri build', {
  stdio: 'inherit',
  env: { ...process.env, TAURI_SIGNING_PRIVATE_KEY: fs.readFileSync(key, 'utf8'), TAURI_SIGNING_PRIVATE_KEY_PASSWORD: '' },
});

const { version } = JSON.parse(fs.readFileSync('src-tauri/tauri.conf.json', 'utf8'));
const dir = 'src-tauri/target/release/bundle/nsis';
const installer = fs.readdirSync(dir).find(f => f.endsWith('-setup.exe') && f.includes(`_${version}_`));
if (!installer) throw new Error(`no ${version} installer in ${dir}`);

// GitHub turns spaces in asset names into dots, which would break the URL below: upload a copy
// without spaces. The signature covers the bytes, not the name, so it still matches.
const asset = installer.replaceAll(' ', '-');
fs.copyFileSync(path.join(dir, installer), path.join(dir, asset));

const latest = {
  version,
  pub_date: new Date().toISOString(),
  platforms: {
    'windows-x86_64': {
      signature: fs.readFileSync(path.join(dir, `${installer}.sig`), 'utf8'),
      url: `${base}/${encodeURIComponent(asset)}`,
    },
  },
};
fs.writeFileSync(path.join(dir, 'latest.json'), JSON.stringify(latest, null, 2));
console.log(`\nUpload to ${base}/:\n  ${dir}/${asset}\n  ${dir}/latest.json\nUpdate URL for Settings: ${base}/latest.json`);
