"""Local-only freeze. Does not touch work, Git refs, network, or run SDKs."""
import argparse
import difflib
import json
import re
from pathlib import Path
from group_order_process import digest, PRODUCT_PATHS, KNOWN_IGNORED_RUST

SOURCE = '06b7e6b07ced17718a11d41a5e01e25cef803529'

def dart_names(text):
    names = re.findall(r"\btest(?:Widgets)?\(\s*'((?:[^'\\]|\\.|'[^']*')*?)'\s*,", text)
    # These two reviewed parameterized tests expand to two exact cases each.
    out = []
    for name in names:
        if "${throws ? 'exception' : 'response'}" in name:
            out.extend(name.replace("${throws ? 'exception' : 'response'}", x) for x in ('exception','response'))
        elif "${staleError ? 'error' : 'success'}" in name:
            out.extend(name.replace("${staleError ? 'error' : 'success'}", x) for x in ('error','success'))
        else:
            if '$' in name: raise ValueError('unreviewed parameterized test')
            out.append(name)
    return out

def freeze(root, approved=False):
    base, work = root/'base', root/'work'
    dest = root/'validation/scripts/next'
    baseline = {str(p.relative_to(base)): digest(p.read_bytes()) for p in sorted(base.rglob('*')) if p.is_file()}
    files = {str(p.relative_to(work)): digest(p.read_bytes()) for p in sorted(work.rglob('*')) if p.is_file() and baseline.get(str(p.relative_to(work))) != digest(p.read_bytes())}
    assert set(files)==PRODUCT_PATHS
    assert len(files)==10 and sum(p.endswith('.dart') for p in files)==5 and sum(p.endswith('.rs') for p in files)==5
    assert len(set(files)-set(baseline))==3
    assert all((work/p).is_file() for p in baseline), 'unexpected deletion'
    patch=''
    for name in files:
        old=(base/name).read_text().splitlines(True) if (base/name).exists() else []
        patch+=''.join(difflib.unified_diff(old,(work/name).read_text().splitlines(True),fromfile='a/'+name if old else '/dev/null',tofile='b/'+name))
    (dest/'group-order.patch').write_text(patch)
    tests={name:dart_names((work/name).read_text()) for name in ('apps/xenterm/test/group_order_display_test.dart','apps/xenterm/test/group_order_actions_test.dart','apps/xenterm/test/groups_test.dart')}
    assert list(map(len,tests.values()))==[4,13,4], tests
    rust='crates/xenterm/src/config/impls/session_group_order_tests.rs'
    rust_tests=['config::config::session_group_order_tests::'+n for n in re.findall(r'#\[test\]\s*fn (\w+)\(', (work/rust).read_text())]
    rust_tests+=['config::config::tests::reorder_session_hop_honors_persisted_group_order']
    assert len(rust_tests)==12
    ignored=[]
    for path in (work/'crates/xenterm').rglob('*.rs'):
        ignored.extend(re.findall(r'#\[ignore[^\n]*\]\s*fn (\w+)\(',path.read_text()))
    assert set(ignored)==KNOWN_IGNORED_RUST
    m={'rust_ignored_names':sorted(ignored),'source':SOURCE,'frozen':approved,'baseline':baseline,'files':files,'patch_sha256':digest(patch.encode()),'flutter_tests':tests,'rust_tests':rust_tests}
    (dest/'group-order-manifest.json').write_text(json.dumps(m,indent=2)+'\n')
    print(json.dumps({'frozen':approved,'paths':list(files),'flutter':list(map(len,tests.values())),'rust':len(rust_tests)}))

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--root',type=Path,required=True);p.add_argument('--approve-freeze',action='store_true');a=p.parse_args();freeze(a.root.resolve(),a.approve_freeze)
