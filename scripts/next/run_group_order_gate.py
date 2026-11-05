"""Pinned synthetic group ordering RED/GREEN gate; never runs a product session."""
import argparse
import difflib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import tempfile
from group_order_process import (Cancellation, ControlError, Deadline, checked, digest, verify_hashes, MAX_OUTPUT, PRODUCT_PATHS, KNOWN_IGNORED_RUST)

SOURCE='06b7e6b07ced17718a11d41a5e01e25cef803529'
SCREEN='apps/xenterm/lib/screens/hosts/hosts_screen.dart'
CONFIG='crates/xenterm/src/config/impls/config.rs'
DISPLAY='apps/xenterm/test/group_order_display_test.dart'
RED_DART='host sections preserve the core group display order'
RED_RUST='config::config::tests::reorder_session_hop_honors_persisted_group_order'
REASON='host sections discarded the core group display order'
SMOKE='the packaged bridge loads and returns its version'


def parse_git_paths(output):
    """Decode Git -z output without quoting, line splitting, or whitespace loss."""
    if output == '':
        return []
    if not output.endswith('\0'):
        raise ControlError('Git path inventory lacks final NUL delimiter')
    paths = output[:-1].split('\0')
    if any(not path or any(part in ('', '.', '..') for part in path.split('/')) for path in paths):
        raise ControlError('invalid or non-relative Git inventory path')
    if len(paths) != len(set(paths)):
        raise ControlError('duplicate Git inventory path')
    return paths


def machine_events(output):
    events=[]
    try:
        for line in output.splitlines():
            if not line.strip(): continue
            e=json.loads(line)
            if e==[{'event':'test.startedProcess','params':{'vmServiceUri':None}}]: continue
            if not isinstance(e,dict): raise ControlError('unreviewed machine envelope')
            events.append(e)
    except ValueError as e: raise ControlError('not machine JSON') from e
    return events


def verify_flutter(output, code, expected=None, *, red=False, full=False):
    events=machine_events(output)
    starts=[e['test'] for e in events if e.get('type')=='testStart']
    ids={e['id']:e['name'] for e in starts}
    if len(ids)!=len(starts): raise ControlError('duplicate test IDs')
    finishes=[e for e in events if e.get('type')=='testDone' and not e.get('hidden',False)]
    names=[ids.get(e.get('testID')) for e in finishes]
    done=[e for e in events if e.get('type')=='done']
    errors=[e for e in events if e.get('type')=='error']
    if len(done)!=1 or not finishes or any(n is None for n in names): raise ControlError('missing test identity or final result')
    if len({e['testID'] for e in finishes})!=len(finishes): raise ControlError('duplicate test finishes')
    if expected is not None and sorted(names)!=sorted(expected): raise ControlError('unexpected test inventory')
    skips=[ids[e['testID']] for e in finishes if e.get('skipped') is not False]
    if skips != ([SMOKE] if full else []): raise ControlError('unknown or missing expected skip')
    if full and len(finishes)-len(skips)!=133: raise ControlError('full Flutter must have exactly 133 passing cases')
    if red:
        if names!=[RED_DART] or code!=1 or done[0].get('success') is not False: raise ControlError('wrong Flutter RED status')
        target=finishes[0]['testID']
        if finishes[0].get('result') not in ('error','failure') or len(errors)!=1 or errors[0].get('testID')!=target: raise ControlError('wrong RED error identity')
        e=errors[0]
        def reason(message):
            return REASON in message and re.search(r'Expected:.*?(?:a value )?less than',message,re.S) and re.search(r'Actual:.*?[0-9]',message,re.S) and 'group_order_display_test.dart' in message
        direct=reason(e.get('error','')+'\n'+e.get('stackTrace',''))
        blocks=[x for x in events if x.get('type')=='print' and 'EXCEPTION CAUGHT BY FLUTTER TEST FRAMEWORK' in x.get('message','')]
        printed=(len(blocks)==1 and blocks[0].get('testID')==target and blocks[0].get('messageType')=='print' and reason(blocks[0].get('message','')) and e.get('error')=='Test failed. See exception logs above.\nThe test description was: '+RED_DART and e.get('stackTrace')=='')
        if not (direct or printed): raise ControlError('unreviewed Flutter RED reason')
    elif code or done[0].get('success') is not True or errors or any(e.get('result')!='success' for e in finishes):
        raise ControlError('Flutter GREEN failed')
    return {'executed':len(finishes)-len(skips),'skipped':skips,'names':names}


def test_binary(output, code, source):
    if code: raise ControlError('Rust compilation failed; not RED evidence')
    artifacts=[]; final=[]
    for line in output.splitlines():
        if not line.startswith('{'): continue # Cargo stderr progress is not JSON.
        try: e=json.loads(line)
        except ValueError as error: raise ControlError('malformed Cargo JSON') from error
        if e.get('reason')=='build-finished': final.append(e)
        if e.get('reason')=='compiler-artifact' and e.get('target',{}).get('name')=='xenterm' and e.get('target',{}).get('kind')==['lib'] and e.get('profile',{}).get('test') is True and e.get('executable'):
            artifacts.append(Path(e['executable']).resolve())
    if len(final)!=1 or final[0].get('success') is not True or len(artifacts)!=1: raise ControlError('missing unique successful Rust lib-test artifact')
    binary=artifacts[0]
    if not binary.is_relative_to(source/'target') or not binary.is_file(): raise ControlError('Rust binary outside owned target')
    return str(binary)


def verify_rust(output, code, expected=None, *, red=False, allow_ignored=(), full=False):
    rows=re.findall(r'^test (.+?) \.\.\. (ok|FAILED|ignored[^\n]*)$',output,re.M)
    summaries=re.findall(r'^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;.*$',output,re.M)
    if not summaries or not rows: raise ControlError('missing Rust execution evidence')
    passed=sum(int(s[1]) for s in summaries);failed=sum(int(s[2]) for s in summaries);ignored=sum(int(s[3]) for s in summaries)
    if passed!=sum(s=='ok' for _,s in rows) or failed!=sum(s=='FAILED' for _,s in rows) or ignored!=sum(s.startswith('ignored') for _,s in rows): raise ControlError('Rust counts mismatch')
    if full and any(int(s[5]) for s in summaries): raise ControlError('full Rust suite filtered tests')
    if any(int(s[4]) for s in summaries): raise ControlError('unexpected measured tests')
    names=[n for n,_ in rows]
    if expected is not None and sorted(names)!=sorted(expected): raise ControlError('wrong Rust exact inventory')
    if any(n.rsplit('::',1)[-1] not in allow_ignored for n,status in rows if status.startswith('ignored')): raise ControlError('unreviewed ignored Rust test')
    if red:
        if code!=101 or names!=[RED_RUST] or passed!=0 or failed!=1 or len(summaries)!=1 or summaries[0][0]!='FAILED': raise ControlError('wrong Rust RED status')
        if not re.search(r"panicked at .*config\.rs:\d+:\d+",output) or "assertion `left == right` failed" not in output or not re.search(r'left:\s*"alpha"\s*\n\s*right:\s*"beta"',output): raise ControlError('wrong Rust RED reason')
    elif code or failed or not passed or any(s[0]!='ok' for s in summaries): raise ControlError('Rust GREEN failed')
    return {'passed':passed,'failed':failed,'ignored':ignored,'tests':rows,'summaries':summaries}


def rust_inventory(output, code):
    if code: raise ControlError('Rust inventory command failed')
    names=re.findall(r'^(.+): test$',output,re.M)
    counts=re.findall(r'^(\d+) tests?, (\d+) benchmarks?$',output,re.M)
    if not names or not counts or sum(int(n) for n,_ in counts)!=len(names) or any(int(n) for _,n in counts): raise ControlError('incomplete Rust inventory')
    return names


def transplant_test(base, green):
    # Exact top-level-in-tests function boundary; no new production API copied.
    name=RED_RUST.rsplit('::',1)[1]
    pattern=r'    #\[test\]\n    fn '+name+r'\(\) \{.*?\n    \}\n'
    blocks=re.findall(pattern,green,re.S)
    anchor='    #[test]\n    fn reorder_session_swaps_same_group_neighbours()'
    if len(blocks)!=1 or base.count(anchor)!=1 or name in base: raise ControlError('ambiguous old-API test transplant')
    return base.replace(anchor,blocks[0]+'\n'+anchor)


def isolated_env(root, logs):
    env={k:os.environ[k] for k in ('PATH','LANG','LC_ALL') if k in os.environ}
    # Toolchain homes are newly created by this workflow, not user caches.
    for key in ('RUSTUP_HOME','CARGO_HOME'):
        path=Path(os.environ.get(key,'/missing')).resolve()
        if not path.is_dir() or path.name not in ('group-order-rustup','group-order-cargo'): raise ControlError('unreviewed Rust toolchain home')
        if any((path/n).exists() for n in ('credentials','credentials.toml','config','config.toml')): raise ControlError('credential/config-bearing toolchain cache')
        env[key]=str(path)
    for key in ('HOME','TMPDIR','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XENTERM_DATA_DIR','PUB_CACHE'):
        path=root/key;path.mkdir(mode=0o700);env[key]=str(path)
    env.update(PUB_HOSTED_URL='https://pub.flutter-io.cn',CI='true',CARGO_BUILD_JOBS='2',RUSTUP_TOOLCHAIN='1.99.1',CARGO_TERM_COLOR='never',RUST_BACKTRACE='0',FLUTTER_SUPPRESS_ANALYTICS='true',DART_SUPPRESS_ANALYTICS='true',XENTERM_WIDGET_EVIDENCE_DIR=str(logs))
    return env


def run(args):
    source,logs=args.source.resolve(),args.logs.resolve();logs.mkdir(parents=True,exist_ok=True)
    controls=Path(__file__).resolve().parent
    manifest=json.loads((controls/'group-order-manifest.json').read_text())
    if manifest.get('frozen') is not True or manifest['source']!=SOURCE: raise ControlError('final reviewed freeze required')
    baseline,files=manifest['baseline'],manifest['files']
    if set(files)!=PRODUCT_PATHS or len(set(files)-set(baseline))!=3 or any(n.endswith('/main.rs') for n in files): raise ControlError('wrong product scope')
    if set(manifest['rust_ignored_names'])!=KNOWN_IGNORED_RUST: raise ControlError('unknown ignored test allowlist')
    verify_hashes(controls,{'group-order.patch':manifest['patch_sha256']})
    deadline,cancel=Deadline(),Cancellation()
    previous={sig:signal.signal(sig,cancel.handle) for sig in (signal.SIGINT,signal.SIGTERM)}
    results={}
    try:
        with tempfile.TemporaryDirectory(prefix='group-order-') as temp:
            env=isolated_env(Path(temp),logs)
            def call(command,label,seconds=120,cwd=source,allow_failure=False):
                return checked(command,cwd=cwd,env=env,logs=logs,label=label,seconds=seconds,deadline=deadline,cancellation=cancel,allow_failure=allow_failure)
            def git(*args,label): return call(['git',*args],label)[1]
            if git('rev-parse','HEAD',label='source-sha').strip()!=SOURCE or git('status','--porcelain',label='clean-source'): raise ControlError('wrong or dirty source')
            verify_hashes(source,baseline)
            if set(parse_git_paths(git('ls-files','-z',label='baseline-tracked-inventory')))!=set(baseline): raise ControlError('baseline tracked inventory mismatch')
            base={n:(source/n).read_bytes() if n in baseline else None for n in files}
            git('apply','--check',str(controls/'group-order.patch'),label='check-main-patch')
            git('apply',str(controls/'group-order.patch'),label='apply-main-patch');verify_hashes(source,files)
            raw={n:(source/n).read_bytes() for n in files}
            app=source/'apps/xenterm'
            flutter=shutil.which('flutter',path=env['PATH'])
            if not flutter: raise ControlError('Flutter unavailable')
            flutter=str(Path(flutter).resolve());dart=str(Path(flutter).with_name('dart'))
            version=json.loads(call([flutter,'--version','--machine'],'flutter-version',90,app)[1])
            if version.get('frameworkVersion')!='3.47.7': raise ControlError('wrong Flutter')
            rust=call(['rustc','--version'],'rust-version')[1]
            if not rust.startswith('rustc 1.99.1 '): raise ControlError('wrong Rust')
            call([flutter,'pub','get','--enforce-lockfile'],'locked-pub',300,app)
            verify_hashes(source,{n:baseline[n] for n in ('Cargo.lock','apps/xenterm/pubspec.lock')})
            dart_files=[n for n in files if n.endswith('.dart')]
            call([dart,'format',*[str(source/n) for n in dart_files]],'format-five-dart',90,app)
            green={n:(source/n).read_bytes() for n in files}
            diff=''.join(''.join(difflib.unified_diff(raw[n].decode().splitlines(True),green[n].decode().splitlines(True),fromfile='raw/'+n,tofile='formatted/'+n)) for n in dart_files)
            if len(diff.encode())>MAX_OUTPUT: raise ControlError('formatter evidence too large')
            (logs/'formatter-only.patch').write_text(diff)
            for n,data in green.items():
                p=logs/'final-files'/n;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(data)
            identity={'source':SOURCE,'manifest_sha256':digest((controls/'group-order-manifest.json').read_bytes()),'patch_sha256':manifest['patch_sha256'],'raw':files,'formatted':{n:digest(d) for n,d in green.items()},'flutter':version,'rust':rust,'locks':{n:baseline[n] for n in ('Cargo.lock','apps/xenterm/pubspec.lock')}}
            (logs/'source-identity.json').write_text(json.dumps(identity,indent=2)+'\n')
            def restore(snapshot):
                for n,data in snapshot.items():
                    if data is None: (source/n).unlink(missing_ok=True)
                    else: (source/n).write_bytes(data)
            try:
                (source/SCREEN).write_bytes(base[SCREEN])
                code,out=call([flutter,'test','--no-pub','--machine',str(Path(DISPLAY).relative_to('apps/xenterm')),'--plain-name',RED_DART],'red-flutter',180,app,True)
                results['red_flutter']=verify_flutter(out,code,[RED_DART],red=True)
            finally: restore(green)
            rust_base={n:d for n,d in base.items() if n.endswith('.rs')}
            try:
                restore(rust_base)
                transplanted=transplant_test(base[CONFIG].decode(),green[CONFIG].decode())
                (source/CONFIG).write_text(transplanted)
                (logs/'rust-red-transplanted-config.sha256').write_text(digest(transplanted.encode())+'\n')
                (logs/'rust-red-test-only.patch').write_text(''.join(difflib.unified_diff(base[CONFIG].decode().splitlines(True),transplanted.splitlines(True),fromfile='base/'+CONFIG,tofile='red/'+CONFIG)))
                verify_hashes(source,{n:baseline[n] for n in rust_base if n in baseline and n!=CONFIG})
                cargo=['cargo','test','--locked','-p','xenterm','--no-default-features','--features','headless','--lib']
                code,out=call(cargo+['--no-run','--message-format=json'],'red-rust-build',1500,allow_failure=True)
                binary=test_binary(out,code,source)
                listing=call([binary,'--list'],'red-rust-inventory')[1]
                if listing.splitlines().count(RED_RUST+': test')!=1: raise ControlError('RED case missing from binary')
                code,out=call([binary,RED_RUST,'--exact','--test-threads=1'],'red-rust',120,allow_failure=True)
                results['red_rust']=verify_rust(out,code,[RED_RUST],red=True)
            finally: restore(green)
            call([flutter,'analyze','--no-pub'],'flutter-analyze',300,app)
            for file,names in manifest['flutter_tests'].items():
                for index,name in enumerate(names):
                    code,out=call([flutter,'test','--no-pub','--machine',str(Path(file).relative_to('apps/xenterm')),'--plain-name',name],f'green-{Path(file).stem}-{index}',150,app,True)
                    results[f'{file}:{index}']=verify_flutter(out,code,[name])
                code,out=call([flutter,'test','--no-pub','--machine',str(Path(file).relative_to('apps/xenterm'))],f'green-full-{Path(file).stem}',180,app,True)
                verify_flutter(out,code,names)
            code,out=call([flutter,'test','--no-pub','--machine'],'green-full-flutter',600,app,True)
            results['full_flutter']=verify_flutter(out,code,full=True)
            if results['full_flutter']['executed']!=133: raise ControlError('full Flutter must pass exactly 133 tests plus one known smoke skip')
            required=[n for names in manifest['flutter_tests'].values() for n in names]
            if any(results['full_flutter']['names'].count(n)!=1 for n in required): raise ControlError('full Flutter missing reviewed cases')
            cargo=['cargo','test','--locked','-p','xenterm','--no-default-features','--features','headless']
            code,out=call(cargo+['--lib','--no-run','--message-format=json'],'green-rust-build',900,allow_failure=True)
            binary=test_binary(out,code,source)
            listing=call([binary,'--list'],'green-rust-inventory')[1]
            for index,name in enumerate(manifest['rust_tests']):
                if listing.splitlines().count(name+': test')!=1: raise ControlError('GREEN case missing from binary')
                code,out=call([binary,name,'--exact','--test-threads=1'],f'green-rust-exact-{index}',120,allow_failure=True)
                results[f'rust:{name}']=verify_rust(out,code,[name])
            code,out=call(cargo+['--','--list'],'green-full-headless-inventory',900,allow_failure=True)
            all_rust=rust_inventory(out,code)
            code,out=call(cargo+['--','--test-threads=1'],'green-full-headless',900,allow_failure=True)
            results['full_headless']=verify_rust(out,code,all_rust,allow_ignored=manifest['rust_ignored_names'],full=True)
            code,out=call(['cargo','test','--locked','-p','xenterm-bridge','--test','groups','--','--test-threads=1'],'green-native-groups',900,allow_failure=True)
            results['native_groups']=verify_rust(out,code,['host_groups_round_trip'])
            verify_hashes(source,{n:sha for n,sha in baseline.items() if n not in files})
            verify_hashes(source,{n:digest(d) for n,d in green.items()})
            tracked=parse_git_paths(git('diff','--name-only','-z',label='final-tracked-scope'))
            added=parse_git_paths(git('ls-files','--others','--exclude-standard','-z',label='final-untracked-scope'))
            if set(tracked)!=set(files)&set(baseline) or set(added)!=set(files)-set(baseline): raise ControlError('unexpected final Git scope')
            (logs/'results.json').write_text(json.dumps(results,indent=2)+'\n')
            cancel.check();print('GROUP_ORDER_RED_GREEN_PASS synthetic_only=true',flush=True)
    finally:
        for sig,handler in previous.items(): signal.signal(sig,handler)
        (logs/'partial-results.json').write_text(json.dumps(results,indent=2)+'\n')

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--source',type=Path,required=True);parser.add_argument('--logs',type=Path,required=True);run(parser.parse_args())
