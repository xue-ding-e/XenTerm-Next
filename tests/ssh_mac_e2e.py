"""Real SSH MAC negotiation through the candidate CLI/MCP, loopback only.

Requires the existing paramiko fixture dependencies. No real credentials or
external servers. AES-CTR forces separate MAC negotiation instead of AEAD.
"""
import argparse
import json
from pathlib import Path

from ssh_jump_chain_e2e import Fixture, fixture_directory
from ssh_proxy_e2e import isolated_environment


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True, type=Path)
    args = parser.parse_args()
    with fixture_directory() as root:
        fixture = Fixture(args.exe.resolve(), root)
        try:
            fixture.env = isolated_environment(Path(root), fixture.env)
            for value in ('HOME', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA',
                          'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_DATA_HOME',
                          'XDG_STATE_HOME', 'TEMP', 'TMP', 'TMPDIR'):
                Path(fixture.env[value]).mkdir(parents=True, exist_ok=True)
            for offered, expected in [
                (('hmac-sha1',), 'hmac-sha1'),
                (('hmac-sha1', 'hmac-sha2-256'), 'hmac-sha2-256'),
            ]:
                for node in fixture.nodes:
                    node.mac_algorithms = offered
                    node.cipher_algorithms = ('aes128-ctr',)
                    node.negotiated_macs.clear()
                for name in ('outer', 'inner', 'target'):
                    fixture.command(name)
                result = fixture.mcp('list_remote_files', session_id='target',
                                     path='.', timeout_seconds=8)
                assert not result.get('isError'), result
                assert 'fixture.txt' in json.dumps(result['structuredContent']), result
                for node in fixture.nodes:
                    assert node.negotiated_macs, f'{node.name}: handshake not observed'
                    assert all(pair == (expected, expected)
                               for pair in node.negotiated_macs), node.negotiated_macs
                print(f'PASS: {expected}, direct/multi-hop command and SFTP; both MAC directions')

            # Unsupported MD5-only peers must fail before authentication. Do not
            # turn a compatibility fix into an unauthenticated or MD5 fallback.
            node = fixture.nodes[0]
            node.mac_algorithms = ('hmac-md5',)
            node.auth_attempts.clear()
            result = fixture.mcp('run_command', session_id='outer',
                                 command='fixture', timeout_seconds=8)
            assert result.get('isError'), result
            assert not node.auth_attempts, 'MD5-only server reached authentication'
            print('PASS: MD5-only server rejected before authentication')
        finally:
            for node in fixture.nodes:
                node.close()


if __name__ == '__main__':
    main()
