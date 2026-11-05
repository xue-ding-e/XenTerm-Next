import 'dart:async';

import 'package:flutter/gestures.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/data/hosts.dart';
import 'package:xenterm_app/screens/forwards/forwards_screen.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 1200, height: 800, child: child)),
);

const _host = Host(
  id: 'h1',
  label: 'gateway',
  address: '10.0.0.2',
  username: 'root',
  protocol: 'ssh',
);

const _rule = ForwardRule(
  sessionId: 'h1',
  sessionName: 'gateway',
  sessionKind: 'ssh',
  index: 0,
  kind: 'local',
  name: 'Web',
  bindAddr: '127.0.0.1',
  bindPort: 8080,
  host: '10.0.0.3',
  hostPort: 80,
);

ForwardRule _namedRule(String name) => ForwardRule(
  sessionId: _rule.sessionId,
  sessionName: _rule.sessionName,
  sessionKind: _rule.sessionKind,
  index: _rule.index,
  kind: _rule.kind,
  name: name,
  bindAddr: _rule.bindAddr,
  bindPort: _rule.bindPort,
  host: _rule.host,
  hostPort: _rule.hostPort,
);

/// Delay a real screen loader, without accessing any native bridge or profile.
class _DelayedRefresh {
  final String stage;
  final rules = Completer<List<ForwardRule>>();
  final hosts = Completer<List<Host>>();
  final jumps = Completer<List<JumpChain>>();
  final started = Completer<void>();
  int ruleCalls = 0;
  int hostCalls = 0;
  int jumpCalls = 0;

  _DelayedRefresh(this.stage);

  Future<List<ForwardRule>> loadRules() async {
    ruleCalls++;
    if (ruleCalls == 1 && stage == 'rules') {
      started.complete();
      return rules.future;
    }
    return [
      _namedRule(
        ruleCalls == 1 ? 'Earlier saved forward' : 'Latest saved forward',
      ),
    ];
  }

  Future<List<Host>> loadHosts() async {
    hostCalls++;
    if (hostCalls == 1 && stage == 'hosts') {
      started.complete();
      return hosts.future;
    }
    return const [_host];
  }

  Future<List<JumpChain>> loadJumps() async {
    jumpCalls++;
    if (jumpCalls == 1 && stage == 'jumps') {
      started.complete();
      return jumps.future;
    }
    return const [];
  }

  void complete({bool fail = false}) {
    switch (stage) {
      case 'rules':
        fail
            ? rules.completeError(StateError('stale refresh failed'))
            : rules.complete([_namedRule('Earlier saved forward')]);
      case 'hosts':
        fail
            ? hosts.completeError(StateError('stale refresh failed'))
            : hosts.complete(const [_host]);
      case 'jumps':
        fail
            ? jumps.completeError(StateError('stale refresh failed'))
            : jumps.complete(const []);
    }
  }

  Widget screen() => ForwardsScreen(
    loadForwards: loadRules,
    loadHosts: loadHosts,
    loadJumps: loadJumps,
    loadTunnels: () async => const [],
  );
}

void main() {
  for (final stage in ['rules', 'hosts', 'jumps']) {
    for (final fail in [false, true]) {
      testWidgets(
        'latest forward refresh ignores older $stage ${fail ? 'error' : 'success'}',
        (tester) async {
          final delayed = _DelayedRefresh(stage);
          await tester.pumpWidget(_app(delayed.screen()));
          await tester.pumpAndSettle();
          expect(delayed.started.isCompleted, isTrue);

          await tester.tap(find.byIcon(LucideIcons.refreshCw));
          await tester.pumpAndSettle();
          expect(find.text('Latest saved forward'), findsOneWidget);
          delayed.complete(fail: fail);
          await tester.pumpAndSettle();

          expect(
            find.text('Latest saved forward'),
            findsOneWidget,
            reason: 'an older refresh replaced the newest forwarding rules',
          );
          expect(find.text('Earlier saved forward'), findsNothing);
          expect(find.textContaining('stale refresh failed'), findsNothing);
          expect(delayed.ruleCalls, 2);
          // Stale reads stop before requesting another part of the snapshot.
          expect(delayed.hostCalls, stage == 'rules' ? 1 : 2);
          expect(delayed.jumpCalls, stage == 'jumps' ? 2 : 1);
          expect(tester.takeException(), isNull);
        },
      );
    }
  }

  testWidgets('older refresh cannot hide the newest refresh error', (
    tester,
  ) async {
    final first = Completer<List<ForwardRule>>();
    var calls = 0;
    await tester.pumpWidget(
      _app(
        ForwardsScreen(
          loadForwards: () {
            calls++;
            return calls == 1
                ? first.future
                : Future.error(StateError('newest refresh failed'));
          },
          loadHosts: () async => const [_host],
          loadJumps: () async => const [],
          loadTunnels: () async => const [],
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byIcon(LucideIcons.refreshCw));
    await tester.pumpAndSettle();
    expect(find.textContaining('newest refresh failed'), findsOneWidget);
    first.complete([_rule]);
    await tester.pumpAndSettle();
    expect(find.textContaining('newest refresh failed'), findsOneWidget);
    expect(find.text('Web'), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('older refresh cannot end the newest loading state', (
    tester,
  ) async {
    final first = Completer<List<ForwardRule>>();
    final second = Completer<List<ForwardRule>>();
    var calls = 0;
    await tester.pumpWidget(
      _app(
        ForwardsScreen(
          loadForwards: () => ++calls == 1 ? first.future : second.future,
          loadHosts: () async => const [_host],
          loadJumps: () async => const [],
          loadTunnels: () async => const [],
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byIcon(LucideIcons.refreshCw));
    await tester.pumpAndSettle();
    first.complete([_rule]);
    await tester.pumpAndSettle();
    expect(find.text('Loading port forwards…'), findsOneWidget);
    expect(find.text('Web'), findsNothing);
    second.complete([_namedRule('Latest saved forward')]);
    await tester.pumpAndSettle();
    expect(find.text('Latest saved forward'), findsOneWidget);
    expect(tester.takeException(), isNull);
  });

  testWidgets('closed forwards screen stops a pending refresh chain', (
    tester,
  ) async {
    final delayed = _DelayedRefresh('rules');
    await tester.pumpWidget(_app(delayed.screen()));
    await tester.pumpAndSettle();
    expect(delayed.started.isCompleted, isTrue);
    await tester.pumpWidget(_app(const SizedBox.shrink()));
    delayed.complete();
    await tester.pumpAndSettle();
    expect(delayed.hostCalls, 0);
    expect(delayed.jumpCalls, 0);
    expect(tester.takeException(), isNull);
  });

  testWidgets('forward rules load and can be deleted', (tester) async {
    var rules = <ForwardRule>[_rule];
    String? deleted;
    await tester.pumpWidget(
      _app(
        ForwardsScreen(
          loadForwards: () async => rules,
          loadTunnels: () async => const [],
          loadHosts: () async => const [_host],
          deleteForward: ({required sessionId, required index}) async {
            deleted = '$sessionId/$index';
            rules = [];
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Web'), findsOneWidget);
    expect(find.text('Local'), findsOneWidget);
    expect(find.textContaining('127.0.0.1:8080'), findsOneWidget);

    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer();
    await mouse.moveTo(
      tester.getCenter(find.byKey(const ValueKey('forward-h1-0'))),
    );
    await tester.pumpAndSettle();
    // The delete action appears only on hover; it is the second action.
    await tester.tap(find.byIcon(LucideIcons.trash2).last);
    await tester.pumpAndSettle();
    expect(deleted, 'h1/0');
    expect(find.text('No forwarding rules'), findsOneWidget);
  });

  testWidgets('a new rule is validated and saved through the bridge callback', (
    tester,
  ) async {
    final saved = <Map<String, Object?>>[];
    var rules = <ForwardRule>[];
    await tester.pumpWidget(
      _app(
        ForwardsScreen(
          loadForwards: () async => rules,
          loadTunnels: () async => const [],
          loadHosts: () async => const [_host],
          saveForward:
              ({
                required sessionId,
                required index,
                required kind,
                required name,
                required bindAddr,
                required bindPort,
                required host,
                required hostPort,
              }) async {
                saved.add({
                  'sessionId': sessionId,
                  'index': index,
                  'kind': kind,
                  'name': name,
                  'bindAddr': bindAddr,
                  'bindPort': bindPort,
                  'host': host,
                  'hostPort': hostPort,
                });
                rules = const [
                  ForwardRule(
                    sessionId: 'h1',
                    sessionName: 'gateway',
                    sessionKind: 'ssh',
                    index: 0,
                    kind: 'local',
                    name: 'DB',
                    bindAddr: '127.0.0.1',
                    bindPort: 15432,
                    host: '10.0.0.5',
                    hostPort: 5432,
                  ),
                ];
                return const CoreWriteResult(ok: true, id: '0');
              },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('New rule'));
    await tester.pumpAndSettle();
    TextField field(String key) => tester.widget<TextField>(
      find.descendant(
        of: find.byKey(ValueKey(key)),
        matching: find.byType(TextField),
      ),
    );
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('forward-name')),
        matching: find.byType(EditableText),
      ),
      'DB',
    );
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('forward-bind-port')),
        matching: find.byType(EditableText),
      ),
      '15432',
    );
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('forward-target-host')),
        matching: find.byType(EditableText),
      ),
      '10.0.0.5',
    );
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('forward-target-port')),
        matching: find.byType(EditableText),
      ),
      '5432',
    );
    expect(field('forward-bind-port'), isNotNull);
    await tester.tap(find.byKey(const ValueKey('forward-save')));
    await tester.pumpAndSettle();
    expect(saved.single['sessionId'], 'h1');
    expect(saved.single['kind'], 'local');
    expect(saved.single['bindPort'], '15432');
    expect(saved.single['host'], '10.0.0.5');
    expect(saved.single['hostPort'], '5432');
    expect(find.text('DB'), findsOneWidget);
  });
}
