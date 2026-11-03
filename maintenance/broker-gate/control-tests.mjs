// Lightweight controller tests only: synthetic local children, no product/Cargo/SSH.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {spawn} from 'node:child_process';
import {bounded, captured, liveGroup, exactPass, qualificationVersion} from './run-gate.mjs';
const root = fs.mkdtempSync(path.join(os.tmpdir(),'broker-control-selftest-'));
const env = {PATH:process.env.PATH};
// The descendant ignores TERM and closes inherited output, reproducing the old
// close-handler race. All paths must still remove executable group members.
const fixture = `import os,sys,time,signal
pid=os.fork()
if pid==0:
 signal.signal(signal.SIGTERM,signal.SIG_IGN)
 os.close(1);os.close(2)
 while True: time.sleep(1)
else:
 open(sys.argv[1],'w').write(str(os.getpgrp()))
 time.sleep(0.1)
 if sys.argv[2]=='timeout': time.sleep(60)
 sys.exit(int(sys.argv[2]) if sys.argv[2]!='timeout' else 0)
`;
try {
 const source='495f000b69856384732f7281c3b544bef783a755';
 assert.equal(qualificationVersion('xt-dev-495f000-T1793740000\n',source),'xt-dev-495f000-T1793740000');
 for(const value of ['xt-dev-0000000-T1793740000','xt-beta-495f000-T1793740000','xt-v1.0.0','xt-dev-495f000-T1793740000\nextra','xt-dev-495f000-T123']) assert.throws(()=>qualificationVersion(value,source));
 const good='test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 5 filtered out; finished in 0.00s\n';
 assert.equal(exactPass({exit:0,timedOut:false},good),true);
 for(const [run,text] of [[{exit:1},good],[{exit:0},good.replace('1 passed','0 passed')],[{exit:0},good+good],[{exit:0,timedOut:true},good],[{exit:0},'']]) assert.equal(exactPass(run,text),false);
 for (const mode of ['0','9','timeout']) {
  const pidfile=path.join(root,mode+'.pid');
  const result=await bounded('python3',['-c',fixture,pidfile,mode],env,mode==='timeout'?300:3000,path.join(root,mode+'.out'),path.join(root,mode+'.err'));
  assert.equal(result.timedOut,mode==='timeout');
  if(mode!=='timeout') assert.equal(result.exit,Number(mode));
  assert.deepEqual(liveGroup(Number(fs.readFileSync(pidfile,'utf8'))),[]);
 }
 for(const mode of ['0','9','timeout']) {
  const pidfile=path.join(root,'helper-'+mode+'.pid');
  const run=captured('python3',['-c',fixture,pidfile,mode],env,mode==='timeout'?300:3000);
  if(mode==='0') assert.equal(await run,''); else await assert.rejects(run);
  assert.deepEqual(liveGroup(Number(fs.readFileSync(pidfile,'utf8'))),[]);
 }
 await assert.rejects(bounded('/no-such-controller-executable',[],env,1000,path.join(root,'error.out'),path.join(root,'error.err')));
 // Independent Node drivers exercise actual external cancellation and an
 // unhandled exception while a real descendant is running.
 for(const mode of ['SIGTERM','SIGINT','exception','rejection']) {
  const pidfile=path.join(root,mode+'.pid');
  const driver=path.join(root,mode+'.mjs');
  fs.writeFileSync(driver,`import {bounded,installHandlers} from ${JSON.stringify(new URL('./run-gate.mjs',import.meta.url).href)};\ninstallHandlers();\nconst run=bounded('python3',${JSON.stringify(['-c',fixture,pidfile,'timeout'])},${JSON.stringify(env)},5000,${JSON.stringify(path.join(root,mode+'.out'))},${JSON.stringify(path.join(root,mode+'.err'))});\n${mode==='exception'?"setTimeout(()=>{throw Error('injected exception')},300);":mode==='rejection'?"setTimeout(()=>{Promise.reject(Error('injected rejection'))},300);":''}\ntry {await run; process.exitCode=2} catch {process.exitCode=1}\n`);
  const child=spawn(process.execPath,[driver],{stdio:'ignore'});
  const done=new Promise((resolve,reject)=>{child.once('error',reject);child.once('close',(code,signal)=>resolve({code,signal}));});
  const end=Date.now()+3000;
  while(!fs.existsSync(pidfile)){assert.ok(Date.now()<end,'fixture did not start');await new Promise(r=>setTimeout(r,10));}
  if(mode.startsWith('SIG')) { child.kill(mode); child.kill(mode); }
  const result=await done;
  assert.equal(result.code,1);
  assert.deepEqual(liveGroup(Number(fs.readFileSync(pidfile,'utf8'))),[]);
 }
 console.log('PASS: direct and captured-helper success/nonzero/timeout, spawn error, repeated SIGTERM/SIGINT, uncaught exception, unhandled rejection; no live descendants');
} finally {fs.rmSync(root,{recursive:true,force:true});}
