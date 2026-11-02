import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/screens/forwards/forwards_screen.dart';
import 'package:xenterm_app/shell/window_activity.dart';
import 'package:xenterm_app/theme/app_theme.dart';

RuntimeTunnel _row({
  String id = 'synthetic-rule',
  String name = 'Synthetic proxy',
  int session = 1,
  bool active = false,
  String status = 'stopped',
}) => RuntimeTunnel(
  id: id,
  name: name,
  kind: 'dynamic',
  bindAddr: '127.0.0.1',
  bindPort: 41080,
  host: '',
  hostPort: 0,
  active: active,
  status: status,
  sessionHandle: session,
);

Widget _app(Future<List<RuntimeTunnel>> Function() load) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(
    child: ForwardsScreen(
      loadForwards: () async => const [],
      loadHosts: () async => const [],
      loadJumps: () async => const [],
      loadTunnels: load,
    ),
  ),
);

Future<void> _mount(
  WidgetTester tester,
  Future<List<RuntimeTunnel>> Function() load, {
  Size size = const Size(1000, 800),
}) async {
  windowActivity.wake();
  tester.view.physicalSize = size;
  tester.view.devicePixelRatio = 1;
  addTearDown(() async {
    await tester.pumpWidget(const SizedBox.shrink());
    windowActivity.wake();
    tester.view.resetPhysicalSize();
    tester.view.resetDevicePixelRatio();
  });
  await tester.pumpWidget(_app(load));
  await tester.pumpAndSettle();
}

String? _activeCount(WidgetTester tester) => tester
    .widget<Text>(find.byKey(const ValueKey('runtime-tunnel-active-count')))
    .data;

void main() {
  test('runtime JSON preserves failed status and owning session identity', () {
    final row = RuntimeTunnel.fromCoreJson({
      'id': 'synthetic-rule',
      'name': 'Synthetic proxy',
      'kind': 'dynamic',
      'bindAddr': '127.0.0.1',
      'bindPort': 41080,
      'host': '',
      'hostPort': 0,
      'active': false,
      'status': 'failed: synthetic address already in use',
      'session_handle': 42,
    });
    expect(row.active, isFalse);
    expect(row.status, 'failed: synthetic address already in use');
    expect(row.sessionHandle, 42);
    expect(row.bindPort, 41080);
  });

  testWidgets('same runtime row follows its lifecycle without appending cards', (
    tester,
  ) async {
    var rows = [_row()];
    Future<List<RuntimeTunnel>> load() async => rows;
    await _mount(tester, load);
    for (final state in [
      (false, 'starting'),
      (false, 'failed: synthetic address already in use'),
      (true, 'running'),
      (false, 'stopped'),
      (true, 'running'),
      (false, 'session disconnected: synthetic transport closed'),
    ]) {
      rows = [_row(active: state.$1, status: state.$2)];
      await tester.pump(const Duration(seconds: 2));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('runtime-tunnel-1-synthetic-rule')), findsOneWidget);
      expect(find.text('Synthetic proxy'), findsOneWidget);
      expect(find.text(state.$2), findsOneWidget);
      expect(_activeCount(tester), state.$1 ? '1 active' : '0 active');
    }
    expect(find.text('Start'), findsNothing, reason: 'the existing card is readonly');
    expect(find.text('Stop'), findsNothing);
  });

  testWidgets('only running rows count as active and dynamic target is SOCKS5', (
    tester,
  ) async {
    await _mount(tester, () async => [
      _row(active: true, status: 'running'),
      _row(session: 2, active: true, status: 'running'),
      _row(id: 'failed-rule', status: 'failed: synthetic bind error'),
    ]);
    expect(_activeCount(tester), '2 active');
    expect(find.text('Runtime tunnels'), findsOneWidget);
    expect(find.text('127.0.0.1:41080 → SOCKS5'), findsNWidgets(3));
    expect(find.textContaining('→ :0'), findsNothing);
    expect(find.byKey(const ValueKey('runtime-tunnel-1-synthetic-rule')), findsOneWidget);
    expect(find.byKey(const ValueKey('runtime-tunnel-2-synthetic-rule')), findsOneWidget);
    expect(find.text('failed: synthetic bind error'), findsOneWidget);
  });

  testWidgets('long errors remain complete at narrow and wide window widths', (
    tester,
  ) async {
    final error = 'failed: ${List.filled(20, 'synthetic address already in use').join(' ')}';
    await _mount(tester, () async => [_row(status: error)], size: const Size(560, 900));
    final status = find.byKey(const ValueKey('runtime-tunnel-status-1-synthetic-rule'));
    expect(tester.widget<Text>(status).data, error);
    expect(tester.widget<Text>(status).maxLines, isNull);
    expect(tester.getSize(status).height, greaterThan(30));
    expect(tester.takeException(), isNull);
    tester.view.physicalSize = const Size(1200, 900);
    await tester.pumpAndSettle();
    expect(tester.widget<Text>(status).data, error);
    expect(tester.widget<Text>(status).maxLines, isNull);
    expect(tester.takeException(), isNull);
  });

  testWidgets('slow reads serialize and coalesce ticks into a latest-state read', (
    tester,
  ) async {
    final requests = <Completer<List<RuntimeTunnel>>>[];
    Future<List<RuntimeTunnel>> load() {
      final request = Completer<List<RuntimeTunnel>>();
      requests.add(request);
      return request.future;
    }
    await _mount(tester, load);
    expect(requests.length, 1);
    await tester.pump(const Duration(seconds: 6));
    expect(requests.length, 1, reason: 'timer ticks cannot create overlapping reads');
    requests[0].complete([_row(active: true, status: 'running')]);
    await tester.pumpAndSettle();
    expect(requests.length, 2, reason: 'the last tick is not lost');
    requests[1].complete([_row(status: 'stopped')]);
    await tester.pumpAndSettle();
    expect(_activeCount(tester), '0 active');
    expect(find.text('stopped'), findsOneWidget);
  });

  testWidgets('a new session source cannot be overwritten by an old response', (
    tester,
  ) async {
    final oldResponse = Completer<List<RuntimeTunnel>>();
    final newResponse = Completer<List<RuntimeTunnel>>();
    var newCalls = 0;
    await _mount(tester, () => oldResponse.future);
    await tester.pumpWidget(_app(() {
      newCalls++;
      return newResponse.future;
    }));
    oldResponse.complete([_row(name: 'Old owner', active: true, status: 'running')]);
    await tester.pumpAndSettle();
    expect(newCalls, 1);
    expect(find.text('Old owner'), findsNothing);
    newResponse.complete([_row(session: 2, name: 'New owner', status: 'stopped')]);
    await tester.pumpAndSettle();
    expect(find.text('New owner'), findsOneWidget);
    expect(find.text('Old owner'), findsNothing);
    expect(find.byKey(const ValueKey('runtime-tunnel-2-synthetic-rule')), findsOneWidget);
  });

  testWidgets('an old source failure cannot mark a replacement source stale', (
    tester,
  ) async {
    final oldResponse = Completer<List<RuntimeTunnel>>();
    await _mount(tester, () => oldResponse.future);
    await tester.pumpWidget(_app(() async => [
      _row(session: 2, name: 'New owner', status: 'stopped'),
    ]));
    oldResponse.completeError(StateError('synthetic old source failure'));
    await tester.pumpAndSettle();
    expect(find.text('New owner'), findsOneWidget);
    expect(find.byKey(const ValueKey('runtime-tunnel-stale')), findsNothing);
    expect(_activeCount(tester), '0 active');
  });

  testWidgets('a pending read completing while minimized waits for wake to refresh', (
    tester,
  ) async {
    final first = Completer<List<RuntimeTunnel>>();
    var calls = 0;
    await _mount(tester, () {
      calls++;
      if (calls == 1) return first.future;
      return Future.value([_row(status: 'stopped')]);
    });
    await tester.pump(const Duration(seconds: 4));
    windowActivity.minimize();
    first.complete([_row(active: true, status: 'running')]);
    await tester.pumpAndSettle();
    expect(calls, 1);
    windowActivity.wake();
    await tester.pumpAndSettle();
    expect(calls, 2);
    expect(_activeCount(tester), '0 active');
  });

  testWidgets('a late response after dispose neither repaints nor polls again', (
    tester,
  ) async {
    final response = Completer<List<RuntimeTunnel>>();
    var calls = 0;
    await _mount(tester, () {
      calls++;
      return response.future;
    });
    await tester.pump(const Duration(seconds: 4));
    await tester.pumpWidget(const SizedBox.shrink());
    response.complete([_row(active: true, status: 'running')]);
    await tester.pump(const Duration(seconds: 4));
    expect(calls, 1);
    expect(tester.takeException(), isNull);
  });

  testWidgets('failed reads disclose stale state and retry on a later tick', (
    tester,
  ) async {
    var calls = 0;
    await _mount(tester, () async {
      calls++;
      if (calls == 2) throw StateError('synthetic bridge unavailable');
      return [_row(active: calls == 1, status: calls == 1 ? 'running' : 'stopped')];
    });
    expect(_activeCount(tester), '1 active');
    await tester.pump(const Duration(seconds: 2));
    await tester.pumpAndSettle();
    expect(calls, 2);
    expect(find.byKey(const ValueKey('runtime-tunnel-stale')), findsOneWidget);
    expect(_activeCount(tester), 'Status unavailable');
    await tester.pump(const Duration(milliseconds: 100));
    expect(calls, 2, reason: 'a failed read must not spin in immediate retries');
    await tester.pump(const Duration(seconds: 2));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('runtime-tunnel-stale')), findsNothing);
    expect(_activeCount(tester), '0 active');
  });

  testWidgets('minimizing pauses reads and waking refreshes the final state', (
    tester,
  ) async {
    var calls = 0;
    var active = true;
    await _mount(tester, () async {
      calls++;
      return [_row(active: active, status: active ? 'running' : 'stopped')];
    });
    windowActivity.minimize();
    active = false;
    await tester.pump(const Duration(seconds: 6));
    expect(calls, 1);
    windowActivity.wake();
    await tester.pumpAndSettle();
    expect(calls, 2);
    expect(_activeCount(tester), '0 active');
  });
}
