import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
const rootIndex = args.indexOf('--root');
const root = rootIndex >= 0 ? path.resolve(args.splice(rootIndex, 2)[1]) : path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const command = args.shift() || 'check';
const sourcePath = path.join(root, 'CHANGELOG.json');
const read = file => readFileSync(path.join(root, file), 'utf8');
const log = JSON.parse(read('CHANGELOG.json'));
const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
const save = () => writeFileSync(sourcePath, JSON.stringify(log, null, 2) + '\n');
const categories = ['Added', 'Fixed', 'Changed', 'Security', 'Tooling'];
const generated = new Set(['CHANGELOG.json', 'app/public/guide.html', 'extension/guide.html', 'extension/guide.js', 'app/public/legal.html', 'extension/legal.html', 'extension/legal.js']);
const guideSource = 'scripts/templates/guide.html';
// Ignored local copies must fingerprint as removed after git rm --cached,
// matching a fresh checkout where those copies do not exist.
const workingFiles = new Set(execFileSync('git', ['ls-files', '--cached', '--others', '--exclude-standard', '-z'], {cwd:root}).toString().split('\0'));
const escape = value => String(value).replace(/[&<>"']/g, char => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[char]));
const withoutGenerated = text => text.replace(/<!-- CHANGELOG(?:-SCRIPT)?:START -->[\s\S]*?<!-- CHANGELOG(?:-SCRIPT)?:END -->\r?\n?/g, '').replace(/\r\n/g,'\n').replace(/\n\s*\n/g,'\n').trim();
function fingerprint(file) {
  if (!workingFiles.has(file) || !existsSync(path.join(root, file))) return 'deleted';
  const raw = readFileSync(path.join(root, file));
  const bytes = file === guideSource ? withoutGenerated(read(file)) : raw.includes(0) ? raw : raw.toString('utf8').replace(/\r\n/g, '\n');
  return createHash('sha256').update(bytes).digest('hex');
}
function changedFiles() {
  const baseline = log.development?.baselineCommit;
  if (!/^[a-f0-9]{40}$/.test(baseline || '')) throw Error('Missing valid development Git baseline.');
  const tracked = execFileSync('git', ['diff', '--name-only', '-z', baseline], {cwd:root}).toString().split('\0');
  const untracked = execFileSync('git', ['ls-files', '--others', '--exclude-standard', '-z'], {cwd:root}).toString().split('\0');
  return [...new Set([...tracked,...untracked])].filter(file=>{
    if (!file || generated.has(file)) return false;
    if (log.development.baselineFileHashes?.[file] === fingerprint(file)) return false;
    if (file === guideSource && existsSync(path.join(root,file))) {
      try { if (withoutGenerated(git('show',baseline+':'+file)) === withoutGenerated(read(file))) return false; }
      catch { /* A newly added canonical guide must be documented. */ }
    }
    return true;
  });
}
function current() {
  const version = JSON.parse(read('app/package.json')).version;
  if (version !== JSON.parse(read('app/src-tauri/tauri.conf.json')).version || version !== log.development?.version) throw Error('App, Tauri, and development changelog versions must agree.');
  const release = log.releases[version];
  if (!release?.summary?.trim() || !release.changes?.length) throw Error(`Document changes for ${version} before building.`);
  const ids = new Set();
  for (const change of release.changes) {
    if (!change.id || ids.has(change.id) || !categories.includes(change.category) || !change.brief?.trim() || !Array.isArray(change.details) || !change.details.length || change.details.some(text=>typeof text!=='string'||!text.trim()) || !Array.isArray(change.files) || !change.files.length) throw Error('Each change needs a unique id, category, brief, details and files.');
    if (change.files.some(file=>typeof file!=='string'||file.startsWith('/')||file.includes('..')||file.includes('\\'))) throw Error('Change files must be repository-relative paths.');
    ids.add(change.id);
  }
  return release;
}
function audit(checkHashes) {
  const release = current();
  const documented = new Set(release.changes.flatMap(change=>change.files));
  const files = changedFiles();
  const missing = files.filter(file=>!documented.has(file));
  const stale = checkHashes ? files.filter(file=>log.development.fileHashes?.[file] !== fingerprint(file)) : [];
  if (missing.length || stale.length) throw Error(`Changelog audit failed.\n${missing.length ? 'Undocumented files:\n'+missing.join('\n')+'\n' : ''}${stale.length ? 'Changed since documentation checkpoint:\n'+stale.join('\n') : ''}\nWrite/update notes, then run node scripts/changelog.mjs checkpoint.`);
  return files;
}
function render() {
  const releases = Object.entries(log.releases)
    .filter(([version]) => !log.historyStartVersion || version.localeCompare(log.historyStartVersion, undefined, {numeric:true}) >= 0)
    .reverse().sort(([a],[b])=>a===log.development.version?-1:b===log.development.version?1:b.localeCompare(a,undefined,{numeric:true}));
  const currentVersion = log.development.version;
  const categoryOptions = categories.map(category=>`<option>${category}</option>`).join('');
  const html = `<main id="changelogPage" class="changelog-page" data-current-version="${escape(currentVersion)}" hidden>
<h1>Changelog</h1><p>Development progress and version history. Draft notes describe work in progress, not a published release.</p>
<p class="changelog-context">Developing <strong>v${escape(currentVersion)}</strong>${log.development.previousVersion ? ` · Previous version <strong>v${escape(log.development.previousVersion)}</strong>` : ' · First standalone release'}</p>
<div class="changelog-controls"><label>Search changes<input id="changeSearch" type="search" autocomplete="off" placeholder="Feature, fix, or version…"></label><label>Version<select id="changeVersion" aria-label="Version"><option value="">All versions</option>${releases.map(([version])=>`<option>${escape(version)}</option>`).join('')}</select></label><label>Category<select id="changeCategory" aria-label="Category"><option value="">All categories</option>${categoryOptions}</select></label><button id="changeDetails" type="button" aria-pressed="false">Expand details</button><button id="changeReset" type="button">Reset filters</button></div>
<p id="changeCount" role="status" aria-live="polite"></p>
${releases.map(([version,release])=>`<article class="change-release" id="release-${escape(version)}" data-version="${escape(version)}" data-superseded="${release.supersededBy?escape(release.supersededBy):''}"><header><h2><a href="?view=changelog#release-${escape(version)}">v${escape(version)}</a></h2><span class="change-status">${escape(release.status || 'historical')}</span>${release.releasedAt||release.updatedAt?`<time datetime="${escape(release.releasedAt||release.updatedAt)}">${release.releasedAt?'Released':'Documented'} ${escape((release.releasedAt||release.updatedAt).slice(0,10))}</time>`:''}${release.sourceCommit && /^[a-f0-9]{40}$/.test(release.sourceCommit)?`<a href="https://github.com/giran03/Zoltraak/commit/${release.sourceCommit}" target="_blank" rel="noopener noreferrer">Git snapshot</a>`:''}</header><p>${escape(release.summary)}</p>${release.supersededBy?`<p>Notes moved to <a href="?view=changelog#release-${escape(release.supersededBy)}">v${escape(release.supersededBy)}</a>.</p>`:''}${release.previousVersion?`<p class="changelog-context">Based on v${escape(release.previousVersion)}</p>`:''}
${categories.map(category=>{const changes=release.changes.filter(change=>(change.category||'Changed')===category);return changes.length?`<section class="change-category"><h3>${category}</h3>${changes.map(change=>`<details class="change-entry" data-category="${category}"><summary>${escape(change.brief)}</summary><ul>${change.details.map(detail=>`<li>${escape(detail)}</li>`).join('')}</ul></details>`).join('')}</section>`:'';}).join('')}</article>`).join('')}
<p id="changeEmpty" hidden>No changes match these filters.</p>
</main>`;
  let guide = read(guideSource);
  const region = `<!-- CHANGELOG:START -->\n${html}\n<!-- CHANGELOG:END -->`;
  guide = guide.includes('<!-- CHANGELOG:START -->') ? guide.replace(/<!-- CHANGELOG:START -->[\s\S]*?<!-- CHANGELOG:END -->/,region) : guide.replace('    <!-- Footer -->',region+'\n    <!-- Footer -->');
  const script = `<!-- CHANGELOG:START -->\n<script>\n${read('scripts/changelog-view.js')}\n</script>\n<!-- CHANGELOG:END -->`;
  // Generated script lives in a separate marked region at the end of the page.
  guide = guide.replace(/<!-- CHANGELOG-SCRIPT:START -->[\s\S]*?<!-- CHANGELOG-SCRIPT:END -->\s*/, '');
  guide = guide.replace('</body>',script.replaceAll('CHANGELOG:','CHANGELOG-SCRIPT:')+'\n</body>');
  writeFileSync(path.join(root,guideSource),guide);
  writeFileSync(path.join(root,'app/public/guide.html'),guide);
  const scripts=[];
  let extensionGuide=guide.replace(/<script>([\s\S]*?)<\/script>/g,(_,code)=>{scripts.push(code);return '';});
  extensionGuide=extensionGuide.replace('</body>','<script src="guide.js"></script>\n</body>');
  writeFileSync(path.join(root,'extension/guide.html'),extensionGuide);
  writeFileSync(path.join(root,'extension/guide.js'),scripts.join('\n'));
  if (existsSync(path.join(root, 'scripts/templates/legal.html'))) {
    const legal = read('scripts/templates/legal.html');
    writeFileSync(path.join(root, 'app/public/legal.html'), legal);
    const legalScripts = [];
    const extensionLegal = legal.replace(/<script>([\s\S]*?)<\/script>/g, (_, code) => { legalScripts.push(code); return ''; })
      .replace('</body>', '<script src="legal.js"></script>\n</body>').replace(/^[ \t]+$/gm, '');
    writeFileSync(path.join(root, 'extension/legal.html'), extensionLegal);
    writeFileSync(path.join(root, 'extension/legal.js'), legalScripts.map(code => code.trim()).join('\n') + '\n');
  }
}

try {
  if (command === 'start') {
    const [version,previousVersion]=args;
    if (!/^\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(version||'')) throw Error('Use start <new version> <previous version>.');
    const repairTransfer = args.includes('--transfer') && log.releases[version]?.carriedFrom === previousVersion;
    if (log.development.version !== version || !log.releases[version]?.changes?.length || repairTransfer) {
      const previous = log.releases[previousVersion];
      const pendingDraft = previous?.status === 'development' && previous.changes?.length > 0;
      if (pendingDraft && !args.includes('--transfer') && !args.includes('--keep')) throw Error('Logged development changes detected. Choose --transfer or --keep; the version manager asks Y/N before changing files.');
      const carryDraft = pendingDraft && args.includes('--transfer');
      const versionFiles = ['app/package.json','app/package-lock.json','app/src-tauri/tauri.conf.json','app/src-tauri/Cargo.toml','app/src-tauri/Cargo.lock','engine/Cargo.toml','engine/Cargo.lock'];
      if (existsSync(path.join(root, 'app/package-lock.json'))) {
        const lock = JSON.parse(read('app/package-lock.json'));
        lock.version = version;
        if (lock.packages?.['']) lock.packages[''].version = version;
        writeFileSync(path.join(root, 'app/package-lock.json'), JSON.stringify(lock, null, 2) + '\n');
      }
      for (const file of ['app/src-tauri/Cargo.lock', 'engine/Cargo.lock']) {
        if (existsSync(path.join(root, file))) writeFileSync(path.join(root, file), read(file).replace(
          /(name = "zoltraak-(?:app|api|core|cli)"\r?\nversion = ")[^"]+/g,
          (_, prefix) => prefix + version));
      }
      log.releases[version] ||= {status:'development',previousVersion,summary:`Development updates for Zoltraak ${version}.`,changes:[]};
      const release = log.releases[version];
      if (carryDraft) {
        release.changes = [...new Map([...structuredClone(previous.changes),...release.changes].map(change=>[change.id,change])).values()];
        if (!release.carriedFrom) release.summary = previous.summary;
        release.transferredFrom = previousVersion;
        delete release.carriedFrom;
        previous.changes = [];
        previous.status = 'superseded';
        previous.supersededBy = version;
        previous.summary = `Unpublished development notes transferred to ${version}. This draft was not released.`;
      }
      if (!release.changes.some(change=>change.id===`version-${version}`)) release.changes.push({id:`version-${version}`,category:'Tooling',brief:`Update the application version to ${version}.`,
        details:[`App and engine version metadata now target ${version}.`,carryDraft ? `Unpublished development notes from ${previousVersion} were transferred after confirmation.` : `Previous notes stay on ${previousVersion}; this version starts with its own metadata change.`],files:versionFiles});
      log.development={version,previousVersion,baselineCommit:pendingDraft ? log.development.baselineCommit : git('rev-parse','HEAD'),
        baselineFileHashes:carryDraft ? (log.development.baselineFileHashes||{}) : pendingDraft ? {...log.development.baselineFileHashes,...log.development.fileHashes} : {},
        fileHashes:{...(carryDraft ? log.development.fileHashes : {}),...Object.fromEntries(versionFiles.map(file=>[file,fingerprint(file)]))}};
      save();
      console.log(`Changelog now tracks v${version}${carryDraft ? `; development notes transferred from v${previousVersion}` : '; previous notes kept in history'}.`);
    }
  } else if (command === 'checkpoint') {
    const files=audit(false);
    log.releases[log.development.version].updatedAt = new Intl.DateTimeFormat('en-CA',{timeZone:'Asia/Manila',year:'numeric',month:'2-digit',day:'2-digit'}).format(new Date());
    render();
    log.development.fileHashes=Object.fromEntries(files.map(file=>[file,fingerprint(file)]));
    save();
    console.log(`Documented checkpoint: ${files.length} changed files for v${log.development.version}.`);
  } else if (command === 'check') {
    console.log(`Changelog audit passed: ${audit(true).length} changed files documented.`);
  } else if (command === 'sync') { current(); render(); }
  else throw Error('Commands: check, checkpoint, sync, start <version> <previous>.');
} catch(error) { console.error(error.message); process.exitCode=1; }
