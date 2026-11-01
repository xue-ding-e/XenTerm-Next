// Publish only a package already validated by headless_server_verify.py.
// No credentials are persisted, and existing tags/releases are never replaced.
module.exports = async ({github, context, core}, directory = 'dist') => {
  const fs = require('fs'), crypto = require('crypto');
  const {owner, repo} = context.repo;
  if (`${owner}/${repo}` !== 'xue-ding-e/XenTerm-Next') throw new Error('Wrong repository');
  const manifest = JSON.parse(fs.readFileSync(`${directory}/BUILD.json`, 'utf8'));
  const source = manifest.source_sha;
  if (!/^[0-9a-f]{40}$/.test(source)) throw new Error('Invalid source SHA');
  const tag = `headless-${source}`;
  const releases = await github.paginate(github.rest.repos.listReleases, {owner, repo, per_page: 100});
  if (releases.some(r => r.tag_name === tag)) throw new Error('Existing draft or published release');
  // Atomic creation fails for existing tags, including concurrent publication.
  await github.rest.git.createRef({owner, repo, ref: `refs/tags/${tag}`, sha: source});
  const verifyTag = async () => {
    const {data: ref} = await github.rest.git.getRef({owner, repo, ref: `tags/${tag}`});
    if (ref.object.sha !== source || ref.object.type !== 'commit') throw new Error('Tag changed');
  };
  await verifyTag();
  const body = `独立服务器 headless 预发布，不替换桌面 beta。\n\n产品源码: ${source}\n构建工具: ${manifest.tooling_sha}\n源CI: https://github.com/${owner}/${repo}/actions/runs/${manifest.source_ci_run}\n构建及真实release回归: https://github.com/${owner}/${repo}/actions/runs/${manifest.build_run}\n\nUbuntu 22.04 x86_64构建，最高GLIBC ${manifest.maximum_glibc}，仅依赖libc/libm/libgcc_s/loader。含MIT及第三方许可证。部署前需一致性profile备份及兼容性检查；不宣称glibc2.28兼容。请核验SHA256SUMS和BUILD.json。`;
  const {data: release} = await github.rest.repos.createRelease({owner, repo, tag_name: tag,
    target_commitish: source, name: `XenTerm Next headless ${source.slice(0, 12)} (Ubuntu 22.04)`,
    body, draft: true, prerelease: true, make_latest: 'false'});
  const names = fs.readdirSync(directory).sort();
  for (const name of names) {
    await github.rest.repos.uploadReleaseAsset({owner, repo, release_id: release.id,
      name, data: fs.readFileSync(`${directory}/${name}`)});
  }
  const {data: assets} = await github.rest.repos.listReleaseAssets({owner, repo, release_id: release.id});
  if (JSON.stringify(assets.map(a => a.name).sort()) !== JSON.stringify(names) ||
      assets.some(a => a.state !== 'uploaded' || a.size !== fs.statSync(`${directory}/${a.name}`).size ||
        a.digest !== 'sha256:' + crypto.createHash('sha256').update(fs.readFileSync(`${directory}/${a.name}`)).digest('hex'))) {
    throw new Error('Asset name/size/digest mismatch; keep draft');
  }
  await verifyTag();
  await github.rest.repos.updateRelease({owner, repo, release_id: release.id,
    draft: false, prerelease: true, make_latest: 'false'});
  await verifyTag();
  await core.summary.addHeading('Headless server preview published')
    .addRaw(`Source ${source}; build ${manifest.build_run}. Separate from rolling desktop beta.`).write();
};
