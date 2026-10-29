"""Original-crate GPUI control: synthetic imports only, no release or real SSH."""
import hashlib, importlib.util, json, os, re, shutil, signal, sys, time
from pathlib import Path
HERE=Path(__file__).resolve().parent
RUNTIME_HASH='f6ce086e8578fb4066c5ee73d776f4d3db77282a0928a22acd3ba34e223b40df'
OWNER_HASH='bc48ae5497f44cbdd97d73bce1a67c36e970c3dd13995336ce992de6c750b62c'
assert hashlib.sha256((HERE/'bounded-runtime.py').read_bytes()).hexdigest()==RUNTIME_HASH
spec=importlib.util.spec_from_file_location('bounded',HERE/'bounded-runtime.py');b=importlib.util.module_from_spec(spec);spec.loader.exec_module(b)
SHA='8819e2ec132760047209e9dcff7341c5f8a15f94'
PRODUCT_TREE='dfa077f04993a231ac57782be5c62a79ad825238'
TEST_TREE='a52405b45597c4bff2c3a3e89fda2b622f90632a'
PATCHES={'key-import.patch':'1740f70f99d2c715a2f652d526d81896dd5f17db59e513cc50b2916f0e435305','busy-save-tests-only.patch':'cfc6051f6374a1bb3f72ea74a1b47795047e3c68778ba1ad5ebd9492fa31ed7a'}
CONTROLS=tuple('ui::session_editor::key_import::tests::'+s for s in ('ordinary_save_click_persists_synthetic_import','ordinary_cancel_click_discards_synthetic_import_without_writes','busy_save_enter_preserves_dialog_draft_and_pending_import'))
CORE=('ssh::key_import::tests::validates_plain_and_encrypted_synthetic_containers_without_decryption', 'ssh::key_import::tests::rejects_public_keys_certificates_malformed_text_and_oversized_content', 'ssh::key_import::tests::file_import_checks_regular_file_size_and_utf8', 'ssh::key_import::tests::rejects_symlinks_without_reading_the_target', 'ssh::key_import::tests::refuses_replacement_file_symlink_and_fifo_after_metadata_check', 'ssh::key_import::tests::accepts_synthetic_openssh_and_pkcs8_pem', 'ssh::key_import::tests::imports_encrypted_pkcs8_without_a_passphrase_then_checks_it_at_use', 'ssh::key_import::tests::every_admitted_pkcs8_cipher_and_kdf_family_decrypts_with_locked_features', 'ssh::key_import::tests::accepts_generated_synthetic_sec1_ec_fixture', 'ssh::key_import::tests::rejects_parseable_hardware_key_without_a_signing_backend', 'ssh::key_import::tests::every_enabled_openssh_cipher_family_round_trips', 'ssh::key_import::tests::synthetic_rsa_pkcs1_pkcs8_and_openssh_use_the_same_bounds', 'ssh::key_import::tests::supported_encrypted_openssh_and_ppk_fixtures_decrypt_only_at_use', 'ssh::key_import::tests::rejects_kdf_boundary_excess_without_running_the_kdf', 'ssh::key_import::tests::legacy_encrypted_pem_is_rejected_without_decrypting_or_replacing_a_key', 'ssh::key_import::tests::cancelled_blocking_work_keeps_admission_until_it_actually_finishes', 'ssh::key_import::bounds::tests::rejects_small_synthetic_boundary_crossings_before_big_integer_work', 'ssh::key_import::bounds::tests::rsa_parameter_admission_uses_der_not_the_pem_label', 'ssh::key_import::bounds::tests::backend_support_is_narrower_than_parseable_algorithm_names')
UI=('ui::session_editor::key_import::tests::import_buttons_enter_activate_once_without_dismissing_or_saving', 'ui::session_editor::key_import::tests::disabled_import_buttons_consume_enter_without_activating', 'ui::session_editor::key_import::tests::key_import_stays_in_draft_until_save_and_uses_encrypted_store', 'ui::session_editor::key_import::tests::discard_or_invalid_import_does_not_replace_the_saved_key', 'ui::session_editor::key_import::tests::late_import_cannot_replace_a_new_auth_choice_or_cancelled_generation', 'ui::session_editor::key_import::tests::import_control_uses_actual_setting_layout_and_keeps_actions_inside', 'ui::session_editor::button_tests::enter_outside_editor_action_controls_keeps_the_dialog_default')
NAMES=CORE+UI+CONTROLS
FIXTURES={'tests/fixtures/ppk-v3-ed25519.ppk': '5dbddcb49f25a3771daba968f6ae450dc6fdd58df40330b29298150c59e93d63', 'tests/fixtures/ppk-v3-ecdsa-encrypted.ppk': '75967a3649661c34eab410e463b92a0b1f9a9e16ceeb82198ecec3184968824f', 'tests/fixtures/import-synthetic-ec-sec1.pem': '4233f389ef2e23585992c325419490421722668b91276201a70b2444dc027c9a', 'tests/fixtures/import-synthetic-ec-p384-sec1.pem': 'f2600b0024e7d282325f012caffc9aa7906dc3c5368635ce21c242603fb9e872', 'tests/fixtures/import-synthetic-ec-p521-sec1.pem': 'eacb1873d938c5060eb6fde0d47378d48d2405084d92832261ae37f9b0c5526d', 'tests/fixtures/ppk-v2-rsa.ppk': '74bae94a5022904bc6a196fbf72d8dbec66a7dcd39f5d7a8652855a4e3c60d4f', 'tests/fixtures/ppk-v2-rsa-encrypted.ppk': 'd5dfe920a32a95da795b5cec9a934564a2ff7b5b9110d74c32b00157037dada5'}
FEATURES=['default','desktop','remote-mcp'];TARGET='x86_64-unknown-linux-gnu'
BRANCH='validation/key-import-busy-save-20261029'
FAILED=re.compile(r'^test result: FAILED\. 0 passed; 1 failed; 0 ignored; 0 measured; \d+ filtered out; finished in [0-9.]+s$')

def classify(output,name,code):
    if code==0:
        b.parse_result(output,name);return 'passed'
    assert code==101
    lines=output.splitlines()
    assert lines.count('running 1 test')==1
    assert [s for s in lines if s.startswith('test ') and ' ... ' in s]==['test '+name+' ... FAILED']
    summaries=[s for s in lines if s.startswith('test result:')]
    assert len(summaries)==1 and FAILED.fullmatch(summaries[0])
    return 'failed-test-needs-review'


def safe_failure_evidence(output):
    # Only fixed source locations and allowlisted diagnostic identifiers leave
    # the runner. Never emit arbitrary panic messages, paths or key contents.
    files = ('src/ui/impls/session_editor_key_import.rs',
             'src/ui/impls/session_editor_button_tests.rs',
             'src/ssh/impls/key_import.rs','src/ssh/impls/key_import_bounds.rs')
    locations=[]
    for file in files:
        for line,column in re.findall(r"panicked at " + re.escape(file) + r":([0-9]+):([0-9]+):",output):
            locations.append({'file':file,'line':int(line),'column':int(column)})
    markers={
        'assertion failed: window.has_active_dialog(cx)':('dialog-closed','assertion'),
        'synthetic draft key changed':('draft-key-changed','assertion'),
        'run this regression serially':('admission-not-idle','setup'),
        'actual editor button was not keyboard-focusable: editor-save':('save-focus-unavailable','setup'),
        'editor button did not settle: editor-save':('save-geometry-unsettled','setup'),
        'editor button did not settle: editor-cancel':('cancel-geometry-unsettled','setup'),
        'assertion failed: editor.key_import.busy()':('pending-import-retired','assertion'),
        'assertion failed: !ran.load(Ordering::SeqCst)':('import-ran-before-release','assertion'),
    }
    found=[{'marker':marker,'type':kind} for text,(marker,kind) in markers.items() if text in output]
    return {'locations':locations,'markers':found,'classification':'failed-test-needs-review; marker is not standalone product RED proof'}

def seal_command_logs():
    rows=[]
    for record in b.owner.RECORDS:
        logs={}
        for stream,name in record.get('logs',{}).items():
            path=b.OUT/name
            # Owner-generated basenames only; never follow a symlink for evidence.
            assert path.parent==b.OUT and path.name==name and not path.is_symlink()
            if path.is_file():logs[stream]={'bytes':path.stat().st_size,'sha256':b.digest(path)}
            else:logs[stream]={'missing':True}
        rows.append({'label':record['label'],'exit_code':record['exit_code'],'reason':record['reason'],'survivors':record['survivors'],'logs':logs})
    (b.OUT/'log-identities.json').write_text(json.dumps(rows,indent=2))

def apply(name,tree):
    assert b.digest(HERE/name)==PATCHES[name]
    b.run(['git','apply','--check','--whitespace=error',str(HERE/name)],name+'-check',10)
    b.run(['git','apply','--index',str(HERE/name)],name+'-apply',10)
    assert b.run(['git','write-tree'],name+'-tree',10).strip()==tree

def validate():
    assert sys.platform=='linux' and not sys.flags.optimize
    b.owner.JOB_DEADLINE=time.monotonic()+58*60
    expected=os.environ['EXPECTED_CONTROL_SHA'];control=HERE.parent
    assert re.fullmatch('[0-9a-f]{40}',expected) and os.environ['GITHUB_SHA']==expected
    assert b.run(['git','-C',str(control),'rev-parse','HEAD'],'control-head',10).strip()==expected
    assert not b.run(['git','-C',str(control),'status','--porcelain'],'control-clean',10).strip()
    assert b.run(['git','rev-parse','HEAD'],'source-head',10).strip()==SHA
    assert not b.run(['git','status','--porcelain','--untracked-files=all','--ignored'],'source-clean',10).strip()
    assert b.run(['git','rev-parse','HEAD:Cargo.lock'],'base-lock',10).strip()=='314d59e30b8547fd697742310a7e3fd4939ebc2a'
    b.SHA=SHA;version=b.version_preflight(b.OUT)
    for key,sub in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_DATA_HOME','data'),('XDG_CACHE_HOME','cache'),('XDG_RUNTIME_DIR','runtime'),('XENTERM_DATA_DIR','profile'),('CARGO_TARGET_DIR','target')]:
        path=b.OUT/sub;path.mkdir(mode=0o700);b.ENV[key]=str(path)
    for key in ['HTTP_PROXY','HTTPS_PROXY','ALL_PROXY','http_proxy','https_proxy','all_proxy','XT_VERSION','XT_CHANNEL','RUSTUP_TOOLCHAIN']:b.ENV.pop(key,None)
    b.ENV.update(XT_VERSION_JSON=str(version),GITHUB_SHA=SHA,GITHUB_REF_TYPE='branch',GITHUB_REF_NAME=BRANCH)
    apply('key-import.patch',PRODUCT_TREE);apply('busy-save-tests-only.patch',TEST_TREE)
    for name,h in FIXTURES.items():assert b.digest(b.SOURCE/name)==h
    before=b.run(['git','status','--porcelain','--untracked-files=all','--ignored'],'candidate-status',10).strip()
    # Tree binds every source/fixture byte; no copied helper crate or reduced graph.
    out=b.run([b.CARGO,'+1.99.1','test','--locked','--bin','xenterm','--target',TARGET,'--jobs','1','--no-run','--message-format=json'],'compile',2700,64*1024*1024)
    artifacts=[]
    for line in out.splitlines():
        try:item=json.loads(line)
        except json.JSONDecodeError:continue
        if item.get('reason')=='compiler-artifact' and item.get('profile',{}).get('test') and item.get('executable') and Path(item['target']['src_path']).resolve()==b.SOURCE/'src/main.rs':
            assert sorted(item.get('features',[]))==FEATURES
            assert item['target']['name']=='xenterm' and 'bin' in item['target']['kind']
            artifacts.append(Path(item['executable']).resolve())
    assert len(artifacts)==1;binary=artifacts[0]
    assert binary.is_relative_to((b.OUT/'target').resolve()) and binary.is_file()
    digest=b.digest(binary);results=[];runtime_deadline=time.monotonic()+600
    for i,name in enumerate(NAMES):
        remaining=lambda cap:min(cap,max(0,runtime_deadline-time.monotonic()))
        assert remaining(30)>0
        b.verify_hash(b.digest(binary),digest)
        b.parse_inventory(b.run([str(binary),name,'--exact','--list','--format','terse'],f'list-{i}',remaining(30)),name)
        b.verify_hash(b.digest(binary),digest)
        try:
            output=b.run([str(binary),name,'--exact','--test-threads=1','--format','pretty','--color','never'],f'test-{i}',remaining(60));code=0
        except RuntimeError:
            record=b.owner.RECORDS[-1]
            # Preserve known test assertion failures for review and still run all controls.
            # Never continue after timeout, resource failure or unconfirmed cleanup.
            assert record['label']==f'test-{i}' and record['exit_code']==101 and record['reason']=='nonzero_exit' and not record['survivors']
            output=(b.OUT/record['logs']['stdout']).read_text();code=101
        state=classify(output,name,code)
        diagnostic=output+'\n'+(b.OUT/b.owner.RECORDS[-1]['logs']['stderr']).read_text()
        results.append({'name':name,'exit_code':code,'state':state,'executed':1,'ignored':0,'failure_evidence':safe_failure_evidence(diagnostic) if code else None,'focus_after_disable':{'observations':re.findall(r'busy-save-focus-after-disable=(?:Some\((?:true|false)\)|None)',diagnostic),'availability':'captured-only-if-libtest-emits; empty means not-emitted, not None'}})
        (b.OUT/'tests.json').write_text(json.dumps(results,indent=2))
    b.verify_hash(b.digest(binary),digest)
    b.owner.FINALIZING=True
    assert not b.run(['git','diff','--exit-code'],'source-unchanged',10).strip()
    assert b.run(['git','write-tree'],'final-tree',10).strip()==TEST_TREE
    assert b.run(['git','status','--porcelain','--untracked-files=all','--ignored'],'final-status',10).strip()==before
    assert b.run(['git','rev-parse','HEAD'],'final-head',10).strip()==SHA
    assert b.run(['git','-C',str(control),'rev-parse','HEAD'],'final-control-head',10).strip()==expected
    assert not b.run(['git','-C',str(control),'status','--porcelain'],'final-control-clean',10).strip()
    for name,h in PATCHES.items():assert b.digest(HERE/name)==h
    assert b.digest(HERE/'bounded-runtime.py')==RUNTIME_HASH and b.digest(HERE/'owned-supervisor.py')==OWNER_HASH
    for name,h in FIXTURES.items():assert b.digest(b.SOURCE/name)==h
    receipt={'fixtures':FIXTURES,'source_sha':SHA,'control_sha':expected,'product_tree':PRODUCT_TREE,'test_tree':TEST_TREE,'patches':PATCHES,'binary_sha256':digest,'features':FEATURES,'coverage_counts':{'core':len(CORE),'existing_ui':len(UI),'controls':len(CONTROLS)},'tests':results,'version':b.version_receipt(version),'limits':'Linux GPUI test-platform synthetic fixture only; no real desktop/SSH credentials, no namespace operation, no release; does not prove broad feature correctness'}
    (b.OUT/'receipt.json').write_text(json.dumps(receipt,indent=2))
    assert all(r['exit_code']==0 for r in results),'exact test failed; inspect bounded receipts before classifying'

if __name__=='__main__':
    b.SOURCE=Path(sys.argv[1]).resolve();b.OUT=Path(sys.argv[2]).resolve();b.OUT.mkdir(parents=True,exist_ok=False)
    b.CARGO=os.path.abspath(shutil.which('cargo'));home=Path.home()
    b.ENV=dict(os.environ,CARGO_HOME=os.environ.get('CARGO_HOME',str(home/'.cargo')),RUSTUP_HOME=os.environ.get('RUSTUP_HOME',str(home/'.rustup')),CARGO_BUILD_JOBS='1',CARGO_PROFILE_DEV_DEBUG='0',CARGO_PROFILE_TEST_DEBUG='0',CARGO_INCREMENTAL='0')
    signal.signal(signal.SIGTERM,b.interrupted);signal.signal(signal.SIGINT,b.interrupted)
    try:validate()
    except BaseException as exc:
        # Do not export raw compile/assertion logs: they can contain synthetic key text.
        (b.OUT/'failure.json').write_text(json.dumps({'error_type':type(exc).__name__,'source_sha':SHA,'classification':'unclassified; NOT automatic product RED'}))
        raise SystemExit(1)
    finally:
        seal_command_logs()
