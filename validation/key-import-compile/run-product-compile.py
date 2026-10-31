"""Fixed manual hosted compile entry. No produced test binary is ever invoked."""
import json
import os
from pathlib import Path
import re
import sys
sys.path.insert(0, str(Path(__file__).parent / 'adapter'))
from compile_adapter import plain, require
from product_identity import BRANCH
import controller


def main():
    require(os.name == 'nt' and os.environ.get('GITHUB_EVENT_NAME') == 'workflow_dispatch', 'manual Windows only')
    control = plain(Path(__file__).resolve().parents[2])
    root = plain(Path(os.environ['RUNNER_TEMP']) / 'key-import-compile-evidence')
    sha = os.environ.get('EXPECTED_CONTROL_SHA'); run = os.environ.get('GITHUB_RUN_ID')
    require(os.environ.get('GITHUB_REF') == 'refs/heads/' + BRANCH and os.environ.get('GITHUB_SHA') == sha,
            'fixed control ref')
    git = plain(Path('C:/Program Files/Git/cmd/git.exe'))
    env = dict(SYSTEMROOT='C:\\Windows', WINDIR='C:\\Windows',
               PATH='C:\\Program Files\\Git\\cmd;C:\\Windows\\System32',
               HOME=str(root/'home'), USERPROFILE=str(root/'home'), TEMP=str(root/'tmp'), TMP=str(root/'tmp'),
               GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL=os.devnull, GIT_TERMINAL_PROMPT='0')
    # Read one fixed installed compiler selector, not user configuration or SDK mutation.
    version_file = plain(Path('C:/Program Files/Microsoft Visual Studio/2022/Enterprise/VC/Auxiliary/Build/Microsoft.VCToolsVersion.default.txt'))
    require(version_file.is_file() and version_file.stat().st_size <= 128, 'VC selector bound')
    vc = version_file.read_text().strip(); require(re.fullmatch(r'14\.[0-9]{2}\.[0-9]{5}', vc), 'VC selector format')
    state = dict(state='not-started', control_sha=sha, run_id=run, product_runtime_attempted=False)
    try:
        state['state'] = 'running'
        result = controller.execute(root, control, sha, run, git, env, vc,
                                    Path(__file__).parent/'evidence/v2-native-metadata.json')
        state = dict(result, run_id=run)
        # Only artifact hash/target/features are exported, never its absolute path or binary.
        state['artifact'] = {k: v for k, v in result['artifact'].items() if k != 'path'}
        materials = state.get('compiler_materials', {})
        for row in materials.get('selected_files', {}).values(): row.pop('relative_to_fixed_installation', None)
    except BaseException as error:
        state.update(state='failed', error_type=type(error).__name__)
        raise
    finally:
        # Treat Cargo stdout as untrusted. Export only code/category/verified
        # relative coordinates, never free-form message/rendered/source snippets.
        try:
            from cargo_diagnostics import summarize
            if (root/'compile.stdout.log').is_file():
                diagnostics=summarize(root/'source',root/'compile.stdout.log')
            else:
                diagnostics=dict(structured_diagnostics_available=False,reason='no-compiler-stdout',
                    limitation='No product/compiler failure classification can be inferred from missing structured output.')
        except Exception:
            diagnostics=dict(structured_diagnostics_available=False,reason='structured-diagnostic-validation-failed',
                limitation='Raw error text intentionally omitted; this is not an environment diagnosis.')
        try:
            with (root/'cargo-diagnostics.json').open('x') as stream:json.dump(diagnostics,stream)
        except OSError:
            state['diagnostics_write_failed']=True
        # No raw Cargo output/environment/exception text escapes into artifacts.
        with (root/'product-compile-result.json').open('x') as stream: json.dump(state, stream, indent=2)


if __name__ == '__main__': main()
