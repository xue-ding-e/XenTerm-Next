const assert = require('node:assert/strict');
const gate = require('../scripts/headless_server_gate.cjs');
const source = 'a'.repeat(40), main = 'b'.repeat(40);
const valid = {repository:{full_name:'xue-ding-e/XenTerm-Next'},head_repository:{full_name:'xue-ding-e/XenTerm-Next'},head_sha:source,head_branch:'main',path:'.github/workflows/ci.yml',event:'push',status:'completed',conclusion:'success'};
async function test(change={}, status='ahead', race=false, env={CI_RUN_ID:'123',PUBLISH:'false'}, ref='refs/heads/topic', repo='XenTerm-Next') {
  const outputs=[]; let reads=0;
  const github={rest:{actions:{getWorkflowRun:async()=>({data:{...valid,...change}})},git:{getRef:async()=>({data:{object:{sha:race && reads++ ? source : main}}})},repos:{compareCommitsWithBasehead:async()=>({data:{status}})}}};
  await gate({github,context:{repo:{owner:'xue-ding-e',repo},ref},core:{setOutput:(...x)=>outputs.push(x)}},env);
  assert.deepEqual(outputs,[['source_sha',source]]);
}
(async()=>{
  await test();await test({},'identical');await test({},'ahead',false,{CI_RUN_ID:'123',PUBLISH:'true'},'refs/heads/main');
  for(const change of [{conclusion:'failure'},{status:'in_progress'},{head_sha:'main'},{head_branch:'topic'},{path:'.github/workflows/other.yml'},{event:'pull_request'},{repository:{full_name:'other/repo'}},{head_repository:{full_name:'other/repo'}}])await assert.rejects(()=>test(change));
  await assert.rejects(()=>test({},'diverged'));await assert.rejects(()=>test({},'behind'));await assert.rejects(()=>test({},'ahead',true));
  await assert.rejects(()=>test({},'ahead',false,{CI_RUN_ID:'abc'}));
  await assert.rejects(()=>test({},'ahead',false,{CI_RUN_ID:'123',PUBLISH:'true'}));
  await assert.rejects(()=>test({},'ahead',false,{CI_RUN_ID:'123'},'refs/heads/main','wrong'));
  console.log('17 source provenance and race tests passed');
})().catch(e=>{console.error(e);process.exitCode=1;});
