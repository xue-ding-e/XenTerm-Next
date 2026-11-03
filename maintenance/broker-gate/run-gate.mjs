// Fixed-source headless control. --compile builds the reviewed candidate;
// default mode executes only its identified binary in fixture environments.
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import crypto from 'node:crypto';
import {performance} from 'node:perf_hooks';
import {pathToFileURL, fileURLToPath} from 'node:url';
import {spawn} from 'node:child_process';

function requireThat(value, message) { if (!value) throw new Error(message); }
export function qualificationVersion(text, commit) {
  requireThat(/^[0-9a-f]{40}$/.test(commit), 'invalid qualification source SHA');
  requireThat(new RegExp(`^xt-dev-${commit.slice(0,7)}-T[0-9]{10}\\n?$`).test(text), 'version does not identify fixed qualification source');
  return text.trim();
}
export function listing(text) {
  const counts = new Map();
  for (const line of text.split(/\r?\n/)) {
    const match = /^(\S+): test$/.exec(line);
    if (match) counts.set(match[1], (counts.get(match[1]) ?? 0) + 1);
  }
  return counts;
}
export function verifyInventory(required, all, ignored) {
  requireThat(new Set(required).size === required.length, 'duplicate required test');
  for (const name of required) {
    requireThat(all.get(name) === 1, `missing or duplicated test: ${name}`);
    requireThat(!ignored.has(name), `ignored required test: ${name}`);
  }
}
export function exactPass(run, output) {
  const summaries = output.match(/^test result: .*$/gm) ?? [];
  return run.exit === 0 && !run.timedOut && summaries.length === 1
    && /^test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured;/.test(summaries[0]);
}
export function artifact(records, source, target) {
  const artifacts = records.filter(value => value.reason === 'compiler-artifact'
    && value.target?.name === 'xenterm' && value.target?.kind?.includes('bin')
    && value.profile?.test === true && value.executable);
  requireThat(artifacts.length === 1, 'expected exactly one xenterm test artifact');
  const value = artifacts[0];
  requireThat(path.resolve(value.manifest_path) === path.join(source, 'Cargo.toml'), 'artifact manifest is not candidate');
  requireThat(path.resolve(value.target.src_path) === path.join(source, 'src/main.rs'), 'artifact entrypoint is not candidate');
  const executable = path.resolve(value.executable);
  requireThat(executable.startsWith(target + path.sep), 'artifact executable escaped controlled target');
  return executable;
}
const active = new Set();
let fatal;
function checkpoint() { if (fatal) throw fatal; }
function cancel(error) {
  fatal ??= error instanceof Error ? error : new Error(String(error));
  process.exitCode = 1;
  for (const stop of active) stop(fatal);
}
export function installHandlers() {
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(signal, () => cancel(new Error(`cancelled: ${signal}`)));
  process.on('uncaughtException', cancel);
  process.on('unhandledRejection', cancel);
}
export function liveGroup(pgid) {
  requireThat(fs.existsSync('/proc/self/stat'), 'cleanup_unconfirmed: /proc unavailable');
  return fs.readdirSync('/proc').filter(name => /^\d+$/.test(name)).filter(name => {
    try {
      const text = fs.readFileSync(`/proc/${name}/stat`, 'utf8');
      const fields = text.slice(text.lastIndexOf(')') + 1).trim().split(/\s+/);
      return Number(fields[2]) === pgid && !['Z','X'].includes(fields[0]);
    } catch (error) { if (['ENOENT','ESRCH'].includes(error.code)) return false; throw error; }
  });
}
export function bounded(exe, args, env, budget, stdout, stderr, cwd) {
  checkpoint();
  requireThat(budget > 0, 'total budget exhausted');
  return new Promise((resolve, reject) => {
    let out, err, child, timer, cleaning = false, closed = false, failure;
    let exit = null, signal = null, timedOut = false;
    const stop = async error => {
      failure ??= error;
      if (cleaning) return;
      cleaning = true;
      clearTimeout(timer);
      // close is not proof of descendant shutdown. Always kill the owned group,
      // including success/nonzero/error/cancellation, then verify and reap.
      try {
        if (child?.pid) {
          try { process.kill(-child.pid, 'SIGKILL'); } catch (e) { if (e.code !== 'ESRCH') throw e; }
          const end = performance.now() + 10000;
          while (!closed || liveGroup(child.pid).length) {
            requireThat(performance.now() < end, 'cleanup_unconfirmed: owned group or parent remains');
            await new Promise(r => setTimeout(r, 10));
          }
        }
      } catch (e) { failure = e; }
      finally {
        for (const fd of [out, err]) if (fd !== undefined) { try { fs.closeSync(fd); } catch (e) { failure ??= e; } }
        active.delete(stop);
      }
      if (failure || fatal) reject(failure ?? fatal);
      else resolve({exit, signal, timedOut});
    };
    active.add(stop);
    try {
      out = fs.openSync(stdout, 'w'); err = fs.openSync(stderr, 'w');
      child = spawn(exe, args, {env, cwd, detached: true, stdio: ['ignore', out, err]});
      timer = setTimeout(() => { timedOut = true; void stop(); }, budget);
      child.once('error', error => { void stop(error); });
      child.once('close', (code, sig) => { closed = true; exit = code; signal = sig; void stop(); });
    } catch (error) { void stop(error); }
  });
}
// Capture only the helper's narrow stdout, using the identical owned-group
// lifecycle. Temporary output is deleted after confirmed cleanup, not uploaded.
export async function captured(exe, args, env, budget, cwd) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'broker-gate-helper-'));
  try {
    const stdout = path.join(root,'stdout'), stderr = path.join(root,'stderr');
    const run = await bounded(exe,args,env,budget,stdout,stderr,cwd);
    requireThat(run.exit === 0 && !run.timedOut, 'helper failed or budget exhausted');
    return fs.readFileSync(stdout,'utf8');
  } finally { fs.rmSync(root,{recursive:true,force:true}); }
}
async function sha256(file) {
  const hash = crypto.createHash('sha256');
  for await (const block of fs.createReadStream(file)) hash.update(block);
  return hash.digest('hex');
}
async function main() {
  requireThat(process.platform === 'linux', 'this first-round control is Linux only');
  const [sourceArg, evidenceArg, manifestArg, mode] = process.argv.slice(2);
  requireThat(mode === undefined || mode === '--compile', 'unknown gate mode');
  const deadline = performance.now() + (mode === '--compile' ? 25 : 15)*60*1000;
  const remaining = cap => { checkpoint(); const left = deadline-performance.now(); requireThat(left > 0, 'total budget exhausted'); return Math.min(cap,left); };
  requireThat(sourceArg && evidenceArg && manifestArg, 'source, evidence and inventory arguments required');
  const originalSource = fs.realpathSync(sourceArg), evidence = path.resolve(evidenceArg);
  let source = originalSource;
  const manifest = JSON.parse(fs.readFileSync(manifestArg, 'utf8'));
  const required = Object.values(manifest.groups).flat();
  const regression = 'ssh::connection::managed_connection_tests::abandoned_open_late_confirmation_preserves_another_active_echo';
  requireThat(required.filter(name => name === regression).length === 1, 'missing unique close ACK regression');
  required.splice(required.indexOf(regression), 1); required.unshift(regression);
  requireThat(required.length === 52, 'unexpected required test count');
  const git = async (...args) => (await captured('git', ['-C', source, ...args], {PATH:process.env.PATH}, remaining(10000))).trim();
  requireThat(await git('rev-parse','HEAD') === manifest.source_commit, 'candidate commit mismatch');
  requireThat(await git('rev-parse','HEAD^{tree}') === manifest.source_tree, 'candidate tree mismatch');
  requireThat(await git('status','--porcelain') === '', 'original fixed checkout changed');
  const helper = fileURLToPath(new URL('./prepare-backport.py', import.meta.url));
  const prepare = async action => captured('python3', ['-B','-I',helper,action,originalSource,evidence], {PATH:process.env.PATH}, remaining(Infinity));
  if (mode === '--compile') await prepare('prepare');
  await prepare('verify');
  const receipt = JSON.parse(fs.readFileSync(path.join(evidence,'backport-receipt.json'),'utf8'));
  const work = fs.realpathSync(receipt.work_root);
  requireThat(work.startsWith(os.tmpdir()+path.sep+'broker-russh675-'), 'unexpected prepared root');
  source = fs.realpathSync(path.join(work,'source'));
  const controlledTarget = path.join(work,'target');
  // Import and invoke the existing reviewed factory, rather than maintaining
  // a second allowlist. Python helper dependencies are prepared by control.
  const factory = `import sys, pathlib, os, json\nsys.path.insert(0,sys.argv[1])\nfrom ssh_proxy_e2e import isolated_environment\nprint(json.dumps(isolated_environment(pathlib.Path(sys.argv[2]),os.environ)))`;
  const fixtureBase = fs.mkdtempSync(path.join(os.tmpdir(), 'broker-gate-fixtures-'));
  const environment = async serial => {
    const root = path.join(fixtureBase,String(serial)); fs.mkdirSync(root,{recursive:true});
    const env = JSON.parse(await captured('python3',['-B','-I','-c',factory,path.join(source,'tests'),root],{PATH:process.env.PATH},remaining(10000)));
    for (const key of ['HOME','USERPROFILE','APPDATA','LOCALAPPDATA','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XDG_STATE_HOME','TEMP','TMP','TMPDIR']) {
      requireThat(path.resolve(env[key]).startsWith(root+path.sep), 'profile directory escaped fixture root');
      fs.mkdirSync(env[key],{recursive:true});
    }
    return env;
  };
  try {
    if (mode === '--compile') {
      const env = await environment('compile');
      // Fresh private Cargo home: no shared registry/cache file is modified.
      // Toolchain remains the official pinned installation.
      env.CARGO_HOME = path.join(work, 'cargo-home');
      env.RUSTUP_HOME = path.join(os.homedir(), '.rustup');
      env.RUSTUP_TOOLCHAIN = '1.99.1';
      env.CARGO_TARGET_DIR = controlledTarget;
      // Resolve with the original legal assembly script, not ambient CI values.
      // Both invocations write version.json outside the immutable source copy.
      const versionEnv = {...env, GITHUB_SHA:manifest.source_commit,
        GITHUB_REF_TYPE:'branch', GITHUB_REF_NAME:'validation/broker-russh675-20261103',
        XT_CHANNEL:'dev', XT_VERSION_JSON:path.join(evidence,'version-original.json')};
      delete versionEnv.XT_VERSION;
      const versionRun = await bounded('bash',[path.join(originalSource,'scripts/version.sh')],versionEnv,remaining(10000),path.join(evidence,'version.stdout'),path.join(evidence,'version.stderr'),originalSource);
      requireThat(versionRun.exit === 0 && !versionRun.timedOut, 'fixed-source version resolution failed');
      env.XT_VERSION = qualificationVersion(fs.readFileSync(path.join(evidence,'version.stdout'),'utf8'),manifest.source_commit);
      env.XT_VERSION_JSON = path.join(evidence,'version-build.json');
      fs.writeFileSync(path.join(evidence,'version-inputs.json'),JSON.stringify({source_commit:manifest.source_commit,script_sha256:await sha256(path.join(originalSource,'scripts/version.sh')),GITHUB_REF_TYPE:versionEnv.GITHUB_REF_TYPE,GITHUB_REF_NAME:versionEnv.GITHUB_REF_NAME,XT_CHANNEL:versionEnv.XT_CHANNEL,XT_VERSION:env.XT_VERSION},null,2)+'\n');
      const run = await bounded('cargo', ['+1.99.1','test','--locked','--no-default-features','--features','headless','--bin','xenterm','--jobs','2','--no-run','--message-format=json'], env, remaining(Infinity), path.join(evidence,'build-artifacts.jsonl'),path.join(evidence,'compile.stderr'),source);
      fs.writeFileSync(path.join(evidence,'compile-result.json'),JSON.stringify({...run,status:run.exit===0&&!run.timedOut?'compiled':'not_passed',budget_seconds:1500,automatic_budget_expansion:false},null,2)+'\n');
      requireThat(run.exit===0&&!run.timedOut,'compile failed or budget exhausted');
      await prepare('verify');
      const builtVersion = JSON.parse(fs.readFileSync(env.XT_VERSION_JSON,'utf8'));
      requireThat(builtVersion.version === env.XT_VERSION && builtVersion.hash === manifest.source_commit.slice(0,7) && builtVersion.source === 'xt-version', 'compiled version provenance mismatch');
      const records = fs.readFileSync(path.join(evidence,'build-artifacts.jsonl'),'utf8').split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line));
      const dependencies = records.filter(row => row.reason === 'compiler-artifact' && row.target?.name === 'russh');
      requireThat(dependencies.length === 1 && path.resolve(dependencies[0].manifest_path) === path.join(work,'russh-0.61.2','Cargo.toml'), 'compiled russh is not the isolated backport');
      fs.writeFileSync(path.join(evidence,'actual-russh-artifact.json'), JSON.stringify(dependencies[0],null,2)+'\n');
      const compiledExecutable = fs.realpathSync(artifact(records,source,controlledTarget));
      requireThat(compiledExecutable.startsWith(controlledTarget+path.sep), 'compiled executable escaped controlled target');
      fs.writeFileSync(path.join(evidence,'compiled-binary-identity.json'),JSON.stringify({source, target:controlledTarget, executable:compiledExecutable, executable_sha256:await sha256(compiledExecutable), source_commit:manifest.source_commit, temporary_lock_sha256:receipt.temporary_lock_sha256},null,2)+'\n');
      remaining(Infinity);
      return;
    }
    const target = fs.realpathSync(controlledTarget);
    const records = fs.readFileSync(path.join(evidence,'build-artifacts.jsonl'),'utf8').split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line));
    const executable = fs.realpathSync(artifact(records, source, target));
    requireThat(executable.startsWith(target + path.sep), 'real executable escaped controlled target');
    const compiledIdentity = JSON.parse(fs.readFileSync(path.join(evidence,'compiled-binary-identity.json'),'utf8'));
    requireThat(compiledIdentity.source === source && compiledIdentity.target === target
      && compiledIdentity.executable === executable && compiledIdentity.executable_sha256 === await sha256(executable)
      && compiledIdentity.source_commit === manifest.source_commit && compiledIdentity.temporary_lock_sha256 === receipt.temporary_lock_sha256,
      'compiled binary identity drift between phases');
    const result = {source_commit:manifest.source_commit, source_tree:manifest.source_tree,
      qualification:'russh 0.61.2 + PR675 isolated path backport, NOT original locked dependency', temporary_lock_sha256:receipt.temporary_lock_sha256, modified_file_sha256:receipt.modified_file_sha256, executable_sha256:await sha256(executable), required:required.length, tests:[], status:'running'};
    const save = () => fs.writeFileSync(path.join(evidence,'test-results.json'), JSON.stringify(result,null,2)+'\n');
    save();
    try {
      for (const [name,args] of [['all',['--list','--format','terse']],['ignored',['--list','--ignored','--format','terse']]]) {
        const env = await environment(name);
        const run = await bounded(executable,args,env,remaining(30000),path.join(evidence,`${name}.stdout`),path.join(evidence,`${name}.stderr`));
        requireThat(run.exit === 0 && !run.timedOut, `test enumeration failed: ${name}`);
      }
      verifyInventory(required,listing(fs.readFileSync(path.join(evidence,'all.stdout'),'utf8')),
        listing(fs.readFileSync(path.join(evidence,'ignored.stdout'),'utf8')));
      for (const [index,name] of required.entries()) {
        const env = await environment(index);
        const prefix = path.join(evidence,`test-${String(index+1).padStart(2,'0')}`);
        const run = await bounded(executable,[name,'--exact','--test-threads=1'],env,remaining(90000),prefix+'.stdout',prefix+'.stderr');
        const output = fs.readFileSync(prefix+'.stdout','utf8');
        const pass = exactPass(run, output);
        result.tests.push({name,...run,passed:pass}); save();
        requireThat(pass, `test failed or budget exceeded: ${name}`);
      }
      remaining(Infinity);
      await prepare('verify');
      result.status='passed'; save();
    } catch (error) {
      result.status='not_passed'; result.error=String(error.message); save(); throw error;
    }
  } finally {
    // Only the directory created by this process; never upload generated keys.
    fs.rmSync(fixtureBase,{recursive:true,force:true});
  }
}
// Synchronous exit/SIGKILL cannot run JavaScript cleanup. Catchable runner
// signals and Node exceptions use the same bounded owned-group shutdown.
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  installHandlers();
  if (process.argv[2] === '--self-test') {
    verifyInventory(['a'],listing('a: test\n'),listing(''));
    for (const [all,ignored] of [['a: test\na: test\n',''],['',''],['a: test\n','a: test\n']]) {
      let rejected=false; try { verifyInventory(['a'],listing(all),listing(ignored)); } catch { rejected=true; }
      requireThat(rejected,'negative inventory control failed');
    }
    const base={reason:'compiler-artifact',target:{name:'xenterm',kind:['bin'],src_path:'/s/src/main.rs'},profile:{test:true},manifest_path:'/s/Cargo.toml',executable:'/s/target/broker-control/test'};
    requireThat(artifact([base],'/s','/s/target/broker-control')===base.executable,'artifact positive control');
    for(const records of [[],[base,base],[{...base,executable:'/outside/test'}],[{...base,manifest_path:'/other/Cargo.toml'}]]) {
      let rejected=false;try{artifact(records,'/s','/s/target/broker-control');}catch{rejected=true;}requireThat(rejected,'artifact negative control failed');
    }
    console.log('control parser self-tests passed; no Cargo or product executed');
  } else {
    main().catch(error => { console.error(error.message); process.exitCode=1; });
  }
}
