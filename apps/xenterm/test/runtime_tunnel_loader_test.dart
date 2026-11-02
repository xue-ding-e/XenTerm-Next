import 'package:flutter_test/flutter_test.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';

class _Source implements RuntimeTunnelSource {
  final String json;
  const _Source(this.json);

  @override
  String runtimeTunnelsJson() => json;
}

RuntimeTunnelSource? _unavailable() => null;
RuntimeTunnelSource _empty() => const _Source('[]');
RuntimeTunnelSource _failedRow() => const _Source('''[
  {"id":"synthetic-rule","name":"Synthetic proxy","kind":"dynamic",
   "bindAddr":"127.0.0.1","bindPort":41080,"host":"","hostPort":0,
   "active":false,"status":"failed: synthetic bind error","session_handle":42}
]''');

void main() {
  test('runtime loader reports an unavailable native source instead of empty rows', () async {
    await expectLater(
      loadRuntimeTunnelsFromCore(loadBridge: _unavailable),
      throwsA(isA<NativeBridgeError>()),
    );
  });

  test('runtime loader accepts a legitimate empty native snapshot', () async {
    expect(await loadRuntimeTunnelsFromCore(loadBridge: _empty), isEmpty);
  });

  test('runtime loader preserves native failure details and owner identity', () async {
    final rows = await loadRuntimeTunnelsFromCore(loadBridge: _failedRow);
    expect(rows, hasLength(1));
    expect(rows.single.id, 'synthetic-rule');
    expect(rows.single.sessionHandle, 42);
    expect(rows.single.active, isFalse);
    expect(rows.single.status, 'failed: synthetic bind error');
  });
}
