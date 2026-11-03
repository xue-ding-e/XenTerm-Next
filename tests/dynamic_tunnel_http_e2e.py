"""Authenticated HTTP dynamic-owner lifetime checks with synthetic loopback SSH.
No real OAuth issuer, saved credentials, network proxy or GUI is used.
"""
import argparse
import concurrent.futures
from contextlib import closing
import http.client
import json
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time

from cryptography.hazmat.primitives.asymmetric import rsa, padding
from cryptography.hazmat.primitives import hashes
from remote_mcp_e2e import b64
from dynamic_tunnel_e2e import Peer, socks, assert_closed, free_port, configure_fixture_environment
from ssh_jump_chain_e2e import Fixture, fixture_directory


class Http:
    def __init__(self, fixture):
        self.fixture = fixture
        self.port = free_port()
        self.key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
        public = self.key.public_key().public_numbers()
        self.issuer = 'https://issuer.invalid'
        self.resource = 'https://dynamic-fixture.invalid/mcp'
        jwks = fixture.root / 'jwks.json'
        jwks.write_text(json.dumps({'keys': [dict(kty='RSA', kid='fixture', alg='RS256', use='sig',
            n=b64(public.n.to_bytes((public.n.bit_length()+7)//8, 'big')),
            e=b64(public.e.to_bytes((public.e.bit_length()+7)//8, 'big')))]}))
        config = fixture.root / 'http.json'
        config.write_text(json.dumps(dict(bind=f'127.0.0.1:{self.port}',
            allowed_origins=['https://client.invalid'], oauth=dict(issuer=self.issuer,
                resource=self.resource, jwks_file=str(jwks), required_scope='xenterm:mcp',
                allowed_subjects=['alice', 'bob']))))
        self.errors = tempfile.TemporaryFile(mode='w+', encoding='utf-8')
        self.process = subprocess.Popen([str(fixture.exe), '--data-dir', str(fixture.config),
            'mcp', 'serve', '--http-config', str(config)], env=fixture.env,
            stdout=subprocess.DEVNULL, stderr=self.errors)
        deadline = time.monotonic() + 10
        while True:
            assert self.process.poll() is None, 'HTTP fixture exited before startup'
            try:
                with socket.create_connection(('127.0.0.1', self.port), timeout=.1):
                    break
            except OSError:
                assert time.monotonic() < deadline, 'HTTP fixture not ready'
                time.sleep(.05)

    def token(self, subject='alice', seconds=120):
        claims = dict(iss=self.issuer, aud=self.resource, sub=subject,
                      exp=int(time.time()) + seconds, nbf=int(time.time()) - 1, scope='xenterm:mcp')
        value = b64(json.dumps(dict(alg='RS256', kid='fixture')).encode()) + '.' + b64(json.dumps(claims).encode())
        return value + '.' + b64(self.key.sign(value.encode(), padding.PKCS1v15(), hashes.SHA256()))

    def request(self, token, session=None, payload=None, method='POST'):
        client = http.client.HTTPConnection('127.0.0.1', self.port, timeout=15)
        headers = {'Content-Type': 'application/json', 'Accept': 'application/json, text/event-stream',
                   'MCP-Protocol-Version': '2025-06-18', 'Authorization': 'Bearer ' + token}
        if session:
            headers['Mcp-Session-Id'] = session
        try:
            client.request(method, '/mcp', body=json.dumps(payload) if payload is not None else None, headers=headers)
            response = client.getresponse()
            content = response.read().decode()
            value = None
            if content.lstrip().startswith('{'):
                value = json.loads(content)
            else:
                for line in content.splitlines():
                    if line.startswith('data:') and line[5:].strip():
                        candidate = json.loads(line[5:])
                        if isinstance(candidate, dict) and 'jsonrpc' in candidate:
                            value = candidate
            return response.status, dict((k.lower(), v) for k, v in response.getheaders()), value
        finally:
            client.close()

    def initialize(self, token):
        status, headers, value = self.request(token, payload=dict(jsonrpc='2.0', id=1, method='initialize',
            params=dict(protocolVersion='2025-06-18', capabilities={}, clientInfo=dict(name='dynamic-http-fixture', version='1'))))
        assert status == 200 and 'result' in value, (status, value)
        session = headers['mcp-session-id']
        assert self.request(token, session, dict(jsonrpc='2.0', method='notifications/initialized'))[0] == 202
        return session

    def call(self, token, session, name, arguments, error=False, raw=False):
        status, _, value = self.request(token, session, dict(jsonrpc='2.0', id=2, method='tools/call',
            params=dict(name=name, arguments=arguments)))
        if raw:
            return status, value
        assert status == 200 and value and 'result' in value, (status, value)
        result = value['result']
        assert bool(result.get('isError')) == error, result
        return result if error else result['structuredContent']

    def close(self):
        self.process.terminate()
        self.process.wait(timeout=10)
        self.errors.seek(0)
        text = self.errors.read()
        assert 'loopback-fixture-only' not in text and 'BEGIN OPENSSH PRIVATE KEY' not in text
        self.errors.close()


def set_commands(fixture, permitted):
    with closing(sqlite3.connect(fixture.config / 'sessions.db')) as db:
        settings = json.loads(db.execute("SELECT value FROM meta WHERE key='settings'").fetchone()[0])
        settings.update(mcp_access_mode='restricted', mcp_allow_commands=permitted)
        db.execute("UPDATE meta SET value=? WHERE key='settings'", (json.dumps(settings),))
        db.commit()


def wait_for(predicate, message, seconds=8):
    deadline = time.monotonic() + seconds
    while not predicate():
        assert time.monotonic() < deadline, message
        time.sleep(.05)


def check(binary, directory):
    fixture = Fixture(binary, directory)
    configure_fixture_environment(fixture)
    bind_port = free_port()
    fixture.sessions[2]['forwards'] = [dict(kind='dynamic', bind_addr='127.0.0.1', bind_port=bind_port),
                                       dict(kind='dynamic', bind_addr='127.0.0.1', bind_port=0)]
    fixture.use_key(2)
    peer = Peer()
    fixture.nodes[2].routes[('echo.invalid', 23)] = peer.address
    http = None
    rule = dict(session_id='target', forward_index=0)
    try:
        http = Http(fixture)
        alice, bob = http.token(), http.token('bob')
        owner = http.initialize(alice)
        other = http.initialize(bob)
        first = http.call(alice, owner, 'start_dynamic_tunnel', rule)
        live = socks(first['proxy_url'])
        http.call(bob, other, 'dynamic_tunnel_status', dict(handle=first['handle']), error=True)
        assert http.call(bob, owner, 'dynamic_tunnel_status', dict(handle=first['handle']), raw=True)[0] == 404
        second = http.call(bob, other, 'start_dynamic_tunnel', dict(session_id='target', forward_index=1))
        assert http.request(alice, owner, method='DELETE')[0] in (200, 202, 204)
        assert_closed(first['proxy_url'])
        try:
            assert live.recv(1) == b''
        except ConnectionResetError:
            pass
        live.close()
        socks(second['proxy_url']).close()
        http.call(bob, other, 'release_dynamic_tunnel', dict(handle=second['handle']))
        print('PASS: authenticated HTTP owner isolation; DELETE closes only its own listener and active clients')

        revocable = http.call(bob, other, 'start_dynamic_tunnel', rule)
        active = socks(revocable['proxy_url'])
        set_commands(fixture, False)
        wait_for(lambda: http.call(bob, other, 'dynamic_tunnel_status', dict(handle=revocable['handle']))['state'] == 'permission_revoked',
                 'HTTP permission revocation left the tunnel running')
        assert_closed(revocable['proxy_url'])
        try:
            assert active.recv(1) == b''
        except ConnectionResetError:
            pass
        active.close()
        http.call(bob, other, 'release_dynamic_tunnel', dict(handle=revocable['handle']))
        set_commands(fixture, True)
        print('PASS: HTTP permission revocation closes active SOCKS clients while release remains allowed')

        expiring = http.token(seconds=5)
        expires_owner = http.initialize(expiring)
        expiring_tunnel = http.call(expiring, expires_owner, 'start_dynamic_tunnel', rule)
        active = socks(expiring_tunnel['proxy_url'])
        # Do not send another HTTP request: expiry must close the lease on its
        # own, not as a side effect of status polling or session cleanup.
        assert_closed(expiring_tunnel['proxy_url'])
        try:
            assert active.recv(1) == b'', 'token expiry left an established SOCKS client alive'
        except ConnectionResetError:
            pass
        active.close()
        assert http.call(alice, expires_owner, 'dynamic_tunnel_status',
                         dict(handle=expiring_tunnel['handle']))['state'] == 'authorization_expired'
        http.call(alice, expires_owner, 'release_dynamic_tunnel', dict(handle=expiring_tunnel['handle']))
        print('PASS: an HTTP tunnel lease ends at the starting token expiry, even without another request')

        # Exercise expiry and owner DELETE while real SSH authentication is
        # pending. Completion after cancellation must never resurrect a listener.
        for cancel in ('expiry', 'delete'):
            node = fixture.nodes[2]
            node.release_stalls = threading.Event()
            node.stall_auth = True
            short = http.token(seconds=4 if cancel == 'expiry' else 120)
            pending_owner = http.initialize(short)
            before = len(node.auth_attempts)
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                pending = pool.submit(http.call, short, pending_owner, 'start_dynamic_tunnel', rule, raw=True)
                wait_for(lambda: len(node.auth_attempts) > before, 'fixture never reached pending SSH authentication', seconds=3)
                if cancel == 'delete':
                    assert http.request(short, pending_owner, method='DELETE')[0] in (200, 202, 204)
                status, response = pending.result(timeout=10)
                assert status != 200 or not response or 'error' in response or response.get('result', {}).get('isError'), (status, response)
            assert_closed(f'socks5h://127.0.0.1:{bind_port}')
            node.stall_auth = False
            node.release_stalls.set()
            # Paramiko's auth callback was deliberately blocked. Let it return,
            # then establish that target and both bastions have actually closed,
            # rather than inferring full route rollback from the listening port.
            wait_for(lambda: all(not transport.is_active()
                                 for hop in fixture.nodes for transport in hop.transports),
                     f'HTTP {cancel} left a target or bastion transport active')
            assert_closed(f'socks5h://127.0.0.1:{bind_port}')
            print(f'PASS: HTTP {cancel} during startup rolls back all three SSH transports and never resurrects the listener')
    finally:
        if http:
            http.close()
        peer.close()
        for node in fixture.nodes:
            node.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', required=True)
    args = parser.parse_args()
    with fixture_directory() as directory:
        check(args.exe, directory)


if __name__ == '__main__':
    main()
