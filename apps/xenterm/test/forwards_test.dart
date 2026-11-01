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

void main() {
  testWidgets('forward rules load and can be deleted', (tester) async {
    var rules = <ForwardRule>[_rule];
    String? deleted;
    await tester.pumpWidget(
      _app(
        ForwardsScreen(
          loadForwards: () async => rules,
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
