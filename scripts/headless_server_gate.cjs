// Resolve product source exclusively from a successful, same-repository main CI.
module.exports = async ({github, context, core}, env = process.env) => {
  if (`${context.repo.owner}/${context.repo.repo}` !== 'xue-ding-e/XenTerm-Next') throw new Error('Wrong repository');
  if (env.PUBLISH === 'true' && context.ref !== 'refs/heads/main') throw new Error('Publish requires maintained main tooling');
  if (!/^[0-9]+$/.test(env.CI_RUN_ID)) throw new Error('Expected numeric CI run ID');
  const {owner, repo} = context.repo;
  const {data: run} = await github.rest.actions.getWorkflowRun({owner, repo, run_id: env.CI_RUN_ID});
  if (run.repository.full_name !== 'xue-ding-e/XenTerm-Next' ||
      run.head_repository.full_name !== run.repository.full_name ||
      !/^[0-9a-f]{40}$/.test(run.head_sha) || run.head_branch !== 'main' ||
      run.path !== '.github/workflows/ci.yml' || !['push', 'workflow_dispatch'].includes(run.event) ||
      run.status !== 'completed' || run.conclusion !== 'success') {
    throw new Error('Source must have its own successful main CI');
  }
  const {data: main} = await github.rest.git.getRef({owner, repo, ref: 'heads/main'});
  const {data: comparison} = await github.rest.repos.compareCommitsWithBasehead({owner, repo, basehead: `${run.head_sha}...${main.object.sha}`});
  if (!['identical', 'ahead'].includes(comparison.status)) throw new Error('Source is not an ancestor of current main');
  const {data: reread} = await github.rest.git.getRef({owner, repo, ref: 'heads/main'});
  if (reread.object.sha !== main.object.sha) throw new Error('Main changed during ancestry verification');
  core.setOutput('source_sha', run.head_sha);
};
