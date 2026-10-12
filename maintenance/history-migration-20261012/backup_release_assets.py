#!/usr/bin/env python3
"""Download public release artifacts and verify their published digests, without running them."""
import concurrent.futures,hashlib,json,pathlib,subprocess
P=pathlib.Path(__file__).resolve().parent;release=json.loads((P/'beta-release.json').read_text());dest=P/'release-assets';dest.mkdir(exist_ok=True)
def one(a):
    name=a['name']
    if pathlib.Path(name).name!=name:raise ValueError('Unsafe asset name')
    path=dest/name
    subprocess.check_call(['curl','--fail','--location','--retry','2','--connect-timeout','20','--max-time','300','--silent','--show-error','--output',str(path),a['browser_download_url']])
    if path.stat().st_size!=a['size']:raise ValueError('Size mismatch '+name)
    h=hashlib.sha256(path.read_bytes()).hexdigest();expected=a.get('digest')
    if expected and expected!='sha256:'+h:raise ValueError('Digest mismatch '+name)
    return {'name':name,'bytes':a['size'],'sha256':h,'published_digest_verified':bool(expected),'source':a['browser_download_url']}
if __name__=='__main__':
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:results=list(pool.map(one,release['assets']))
    (P/'release-assets-verified.json').write_text(json.dumps(results,indent=2)+'\n');print('Verified',len(results),'release assets; downloaded only, none executed')
