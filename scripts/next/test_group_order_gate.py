import copy
import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import unittest
from unittest.mock import patch
from group_order_process import Cancellation, ControlError, Deadline, checked, live_members, digest, verify_hashes
from run_group_order_gate import (RED_DART, RED_RUST, REASON, SOURCE, SMOKE, verify_flutter, verify_rust, test_binary, transplant_test, isolated_env, rust_inventory)


def events(name=RED_DART,red=False,printed=False):
    result=[{'type':'testStart','test':{'id':1,'name':name}}]
    if red:
        message='Expected: a value less than <200.0>\nActual: <300.0>\n'+REASON
        if printed: result.append({'type':'print','testID':1,'messageType':'print','message':'EXCEPTION CAUGHT BY FLUTTER TEST FRAMEWORK\n'+message+'\ngroup_order_display_test.dart:50:5'})
        result.append({'type':'error','testID':1,'error':'Test failed. See exception logs above.\nThe test description was: '+RED_DART if printed else message,'stackTrace':'' if printed else 'group_order_display_test.dart:50:5'})
    result.extend([{'type':'testDone','testID':1,'hidden':False,'skipped':False,'result':'error' if red else 'success'},{'type':'done','success':not red}])
    return result


def encode(value): return '\n'.join(json.dumps(e) for e in value)


def rust_log(red=False,name=RED_RUST):
    return ('running 1 test\n'+'test '+name+' ... '+('FAILED' if red else 'ok')+'\n'+
            ("thread '"+name+"' panicked at crates/xenterm/src/config/impls/config.rs:3211:9:\nassertion `left == right` failed\n  left: \"alpha\"\n right: \"beta\"\n" if red else '')+
            'test result: '+('FAILED. 0 passed; 1 failed' if red else 'ok. 1 passed; 0 failed')+'; 0 ignored; 0 measured; 300 filtered out; finished in 0.00s\n')


class Contracts(unittest.TestCase):
    def test_manifest_current_proposal(self):
        root=Path(__file__).parent;m=json.loads((root/'group-order-manifest.json').read_text())
        self.assertEqual(m['source'],SOURCE);self.assertEqual(len(m['baseline']),742)
        self.assertEqual(len(m['files']),10);self.assertEqual(len(set(m['files'])-set(m['baseline'])),3)
        self.assertEqual([len(n) for n in m['flutter_tests'].values()],[4,13,4]);self.assertEqual(len(m['rust_tests']),12)
        verify_hashes(root,{'group-order.patch':m['patch_sha256']})

    def test_flutter_green_and_red(self):
        verify_flutter(encode(events()),0,[RED_DART])
        for printed in (False,True): verify_flutter(encode(events(red=True,printed=printed)),1,[RED_DART],red=True)

    def test_flutter_rejects_bad_inventory_status_skip(self):
        for expected in ([],['wrong'],[RED_DART,RED_DART]):
            with self.assertRaises(ControlError):verify_flutter(encode(events()),0,expected)
        for key,value in [('skipped',True),('result','error'),('testID',44)]:
            es=events();es[-2][key]=value
            with self.assertRaises(ControlError):verify_flutter(encode(es),0,[RED_DART])
        for code in (1,2,-9):
            with self.assertRaises(ControlError):verify_flutter(encode(events()),code,[RED_DART])

    def test_flutter_rejects_compile_failure_and_wrong_red_reason(self):
        for old,new in [(REASON,'different reason'),('less than','greater than'),('group_order_display_test.dart','unrelated.dart')]:
            for printed in (False,True):
                with self.assertRaises(ControlError):verify_flutter(encode(events(red=True,printed=printed)).replace(old,new),1,[RED_DART],red=True)
        for output in ('Compilation failed','',encode(events(red=True)[:-1])):
            with self.assertRaises(ControlError):verify_flutter(output,1,[RED_DART],red=True)
        for code in (0,2,-9):
            with self.assertRaises(ControlError):verify_flutter(encode(events(red=True)),code,[RED_DART],red=True)

    def test_flutter_rejects_duplicate_errors_ids_or_completions(self):
        for duplicate in (0,1,2,3):
            es=events(red=True);es.insert(duplicate,copy.deepcopy(es[duplicate]))
            with self.assertRaises(ControlError):verify_flutter(encode(es),1,[RED_DART],red=True)

    def test_only_packaged_smoke_may_skip_in_full(self):
        es=events(SMOKE);es[-2]['skipped']=True
        for index in range(133):
            es[-1:-1]=[{'type':'testStart','test':{'id':index+2,'name':'full-'+str(index)}},{'type':'testDone','testID':index+2,'hidden':False,'skipped':False,'result':'success'}]
        self.assertEqual(verify_flutter(encode(es),0,full=True)['skipped'],[SMOKE])
        for name in ('unknown',RED_DART):
            es[0]['test']['name']=name
            with self.assertRaises(ControlError):verify_flutter(encode(es),0,full=True)

    def test_rust_requires_exact_real_panic(self):
        verify_rust(rust_log(),0,[RED_RUST]);verify_rust(rust_log(True),101,[RED_RUST],red=True)
        for old,new in [('alpha','beta'),('right: "beta"','right: "alpha"'),('config.rs','other.rs'),('1 failed','0 failed'),('FAILED','ok')]:
            with self.assertRaises(ControlError):verify_rust(rust_log(True).replace(old,new),101,[RED_RUST],red=True)
        for code in (0,1,2,-9):
            with self.assertRaises(ControlError):verify_rust(rust_log(True),code,[RED_RUST],red=True)
        with self.assertRaises(ControlError):verify_rust('error[E0425]: missing name',101,[RED_RUST],red=True)
        with self.assertRaises(ControlError):verify_rust('running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out;',0,[RED_RUST])

    def test_rust_binary_requires_success_unique_owned_artifact(self):
        with tempfile.TemporaryDirectory() as t:
            root=Path(t);(root/'target').mkdir();binary=root/'target/unit';binary.touch()
            artifact={'reason':'compiler-artifact','target':{'name':'xenterm','kind':['lib']},'profile':{'test':True},'executable':str(binary)}
            done={'reason':'build-finished','success':True}
            self.assertEqual(test_binary(encode([artifact,done]),0,root),str(binary))
            for es,code in [([artifact,done],1),([artifact],0),([artifact,artifact,done],0),([dict(artifact,executable='/bin/sh'),done],0),([artifact,dict(done,success=False)],0)]:
                with self.assertRaises(ControlError):test_binary(encode(es),code,root)

    def test_transplant_only_one_old_api_test(self):
        anchor='    #[test]\n    fn reorder_session_swaps_same_group_neighbours()'
        block='    #[test]\n    fn '+RED_RUST.rsplit('::',1)[1]+'() {\n        old_api();\n    }\n'
        self.assertEqual(transplant_test(anchor,block),block+'\n'+anchor)
        for base,green in [(anchor,block+block),('',block),(anchor+anchor,block),(block+anchor,block)]:
            with self.assertRaises(ControlError):transplant_test(base,green)

    def test_environment_is_explicit_and_private(self):
        with tempfile.TemporaryDirectory() as t:
            root=Path(t);rust=root/'group-order-rustup';cargo=root/'group-order-cargo';rust.mkdir();cargo.mkdir();private=root/'private';private.mkdir()
            with patch.dict(os.environ,{'RUSTUP_HOME':str(rust),'CARGO_HOME':str(cargo),'PUB_HOSTED_URL':'https://untrusted.invalid','GH_TOKEN':'fake','HTTPS_PROXY':'fake','SSH_AUTH_SOCK':'fake','CARGO_REGISTRIES_X_TOKEN':'fake'}):
                env=isolated_env(private,root/'logs')
                for key in ('GH_TOKEN','HTTPS_PROXY','SSH_AUTH_SOCK','CARGO_REGISTRIES_X_TOKEN'):self.assertNotIn(key,env)
                self.assertEqual(env['PUB_HOSTED_URL'],'https://pub.flutter-io.cn');self.assertEqual(env['CARGO_BUILD_JOBS'],'2');self.assertEqual(env['RUSTUP_TOOLCHAIN'],'1.99.1')
                self.assertEqual(Path(env['HOME']).stat().st_mode&0o777,0o700)
                (cargo/'credentials.toml').touch()
                with self.assertRaises(ControlError):isolated_env(root,root/'logs')

    def test_full_rust_inventory_rejects_missing_or_unknown_ignored(self):
        self.assertEqual(rust_inventory('a: test\nb: test\n2 tests, 0 benchmarks\n',0),['a','b'])
        for text,code in [('a: test\n2 tests, 0 benchmarks',0),('a: test\n1 test, 0 benchmarks',1),('a: test\n1 test, 1 benchmark',0)]:
            with self.assertRaises(ControlError):rust_inventory(text,code)
        ignored=rust_log().replace('300 filtered out','0 filtered out')+'test bench ... ignored, benchmark\ntest result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s\n'
        verify_rust(ignored,0,[RED_RUST,'bench'],allow_ignored=['bench'])
        with self.assertRaises(ControlError):verify_rust(ignored,0,[RED_RUST,'bench'])
        with self.assertRaises(ControlError):verify_rust(ignored,0,[RED_RUST],allow_ignored=['bench'])

    def test_deadline_is_global(self):
        with patch('group_order_process.time.monotonic',side_effect=[10,12,19,21]):
            deadline=Deadline(10)
            self.assertEqual(deadline.limit(9),8);self.assertEqual(deadline.limit(9),1)
            with self.assertRaises(ControlError):deadline.limit(9)

    def test_frozen_lock_and_red_green_restore_contract(self):
        script=Path(__file__).with_name('run_group_order_gate.py').read_text()
        for text in ("'--enforce-lockfile'", "'--locked'", "finally: restore(green)", "'Cargo.lock','apps/xenterm/pubspec.lock'", "set(added)!=set(files)-set(baseline)", "XENTERM_WIDGET_EVIDENCE_DIR=str(logs)"):
            self.assertIn(text,script)
        self.assertEqual(script.count('finally: restore(green)'),2)

    def test_workflow_initializes_toolchain_homes_in_first_step(self):
        text=(Path(__file__).resolve().parents[2]/'.github/workflows/next-flutter-group-order.yml').read_text()
        job_header,steps=text.split('    steps:',1)
        self.assertNotIn('    env:',job_header)
        self.assertNotIn('${{ runner.',text)
        first_step=steps.split('      - uses:',1)[0]
        self.assertIn('      - name: Initialize isolated toolchain homes',first_step)
        self.assertIn('        shell: bash',first_step)
        for key,suffix in [('RUSTUP_HOME','rustup'),('CARGO_HOME','cargo')]:
            self.assertIn("printf '"+key+"=%s/group-order-"+suffix+r"\n' "+'"$RUNNER_TEMP" >> "$GITHUB_ENV"',first_step)
        self.assertEqual(first_step.count('>> "$GITHUB_ENV"'),2)

    def test_workflow_pins_and_scope(self):
        text=(Path(__file__).resolve().parents[2]/'.github/workflows/next-flutter-group-order.yml').read_text()
        for item in ('ref: '+SOURCE,"flutter-version: '3.47.7'",'dtolnay/rust-toolchain@1.99.1','ubuntu-24.04','timeout-minutes: 60','timeout-minutes: 50','group: next-product-heavy','cancel-in-progress: false','if: always()','contents: read','libudev-dev libdbus-1-dev pkg-config'):self.assertIn(item,text)
        self.assertEqual(text.count('persist-credentials: false'),2)
        for item in ('contents: write','workflow_dispatch','flutter build','cargo build','gh release'):self.assertNotIn(item,text)

class RealProcessControls(unittest.TestCase):
    def invoke(self, script, root, *, failure=False, seconds=3, limit=8192):
        return checked([sys.executable, "-c", script], cwd=root, env={"PATH": os.environ.get("PATH", "")},
                       logs=root, label="test", seconds=seconds, deadline=Deadline(5),
                       cancellation=Cancellation(), allow_failure=failure, max_output=limit)

    def test_nonzero_is_only_returned_when_explicitly_requested(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(ControlError):
                self.invoke("print('failure'); raise SystemExit(1)", root)
            code, output = self.invoke("print('failure'); raise SystemExit(1)", root, failure=True)
            self.assertEqual((code, output.strip()), (1, "failure"))

    def test_output_overflow_fails_and_evidence_is_capped(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ControlError, "output limit"):
                self.invoke("print('x'*100000)", root, limit=1024)
            self.assertLessEqual((root / "test.log").stat().st_size, 1024)

    def test_timeout_terminates_direct_process(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ControlError, "timeout"):
                self.invoke("import os,time; print(os.getpid(),flush=True); time.sleep(30)", root, seconds=.15)
            pid = int((root / "test.log").read_text())
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)

    def test_parent_exits_with_closed_stdout_descendant_is_cleaned(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            script = ("import os,sys,subprocess; "
                      "p=subprocess.Popen([sys.executable,'-c','import time; time.sleep(30)'],"
                      "stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL); "
                      "print(os.getpid(),flush=True)")
            _, output = self.invoke(script, root)
            self.assertEqual(live_members(int(output)), [])

    def test_cancel_before_launch_does_not_spawn(self):
        cancel = Cancellation()
        cancel.handle(signal.SIGTERM, None)
        with tempfile.TemporaryDirectory() as directory, patch("group_order_process.subprocess.Popen") as launch:
            with self.assertRaisesRegex(ControlError, "cancelled"):
                checked(["never"], cwd=Path(directory), env={}, logs=Path(directory), label="cancel",
                        seconds=1, deadline=Deadline(5), cancellation=cancel)
            launch.assert_not_called()


if __name__ == "__main__":
    unittest.main()
