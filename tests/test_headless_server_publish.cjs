const assert = require('node:assert/strict');
const fs=require('fs'),os=require('os'),path=require('path'),crypto=require('crypto');
const publish=require('../scripts/headless_server_publish.cjs');
const source='a'.repeat(40), tag=`headless-${source}`;
async function scenario(mode='ok') {
  const dir=fs.mkdtempSync(path.join(os.tmpdir(),'headless-publish-'));
  const manifest={source_sha:source,tooling_sha:'b'.repeat(40),source_ci_run:'123',build_run:'456',maximum_glibc:'2.34'};
  fs.writeFileSync(path.join(dir,'BUILD.json'),JSON.stringify(manifest));
  for(const name of ['SHA256SUMS','package.tar.gz'])fs.writeFileSync(path.join(dir,name),'fixture');
  let created=0,published=0,uploads=0,refs=0;
  const assets=()=>fs.readdirSync(dir).map(name=>({name,state:'uploaded',size:fs.statSync(path.join(dir,name)).size,digest:'sha256:'+crypto.createHash('sha256').update(fs.readFileSync(path.join(dir,name))).digest('hex')}));
  const github={paginate:async()=>mode==='draft'?[{tag_name:tag,draft:true}]:[],rest:{git:{createRef:async x=>{assert.equal(x.sha,source);if(['existing-tag','race'].includes(mode))throw Error('exists');},getRef:async()=>({data:{object:{type:'commit',sha:mode==='drift' && refs++>0?'c'.repeat(40):source}}})},repos:{listReleases:()=>{},createRelease:async x=>{assert(x.draft&&x.prerelease);assert.equal(x.target_commitish,source);created++;return {data:{id:1}};},uploadReleaseAsset:async()=>{if(mode==='upload-failure')throw Error('failed');uploads++;},listReleaseAssets:async()=>{let a=assets();if(mode==='missing')a.pop();if(mode==='digest')a[0].digest='sha256:wrong';if(mode==='size')a[0].size++;if(mode==='name')a[0].name='unexpected';if(mode==='state')a[0].state='new';return {data:a};},updateRelease:async x=>{assert.equal(x.draft,false);published++;}}}};
  const summary={addHeading(){return this},addRaw(){return this},async write(){}};
  try {
    if(mode==='ok'){await publish({github,context:{repo:{owner:'xue-ding-e',repo:'XenTerm-Next'}},core:{summary}},dir);assert.equal(created,1);assert.equal(uploads,3);assert.equal(published,1);}
    else {await assert.rejects(()=>publish({github,context:{repo:{owner:'xue-ding-e',repo:mode==='repo'?'wrong':'XenTerm-Next'}},core:{summary}},dir));assert.equal(published,0);}
  } finally {fs.rmSync(dir,{recursive:true,force:true});}
}
(async()=>{for(const m of ['ok','draft','existing-tag','race','drift','missing','digest','size','name','state','upload-failure','repo'])await scenario(m);console.log('12 publication state tests passed');})().catch(e=>{console.error(e);process.exitCode=1;});
