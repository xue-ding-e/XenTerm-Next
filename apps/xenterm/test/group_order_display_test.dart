import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/data/hosts.dart';
import 'package:xenterm_app/screens/hosts/hosts_screen.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Future<void> _viewport(WidgetTester tester) async {
  await tester.binding.setSurfaceSize(const Size(1400, 1000));
  addTearDown(() => tester.binding.setSurfaceSize(null));
}

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: child),
);

Host _host(String id, String group) => Host(
  id: id,
  label: 'host-$id',
  address: 'example.invalid',
  port: 22,
  username: 'test',
  group: group,
  protocol: 'ssh',
);

double _headerY(WidgetTester tester, String name) =>
    tester.getTopLeft(find.byKey(ValueKey('group-header-$name'))).dy;

void main() {
  testWidgets('host sections preserve the core group display order', (
    tester,
  ) async {
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => const [],
          loadGroups: () async => const [
            HostGroup(name: 'Zeta', collapsed: false),
            HostGroup(name: 'Alpha', collapsed: false),
          ],
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(
      _headerY(tester, 'Zeta'),
      lessThan(_headerY(tester, 'Alpha')),
      reason: 'host sections discarded the core group display order',
    );
  });

  testWidgets('implicit groups append without resorting known groups', (
    tester,
  ) async {
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => [_host('b', 'Beta'), _host('a', 'Alpha')],
          loadGroups: () async => const [
            HostGroup(name: 'Zeta', collapsed: false),
          ],
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(_headerY(tester, 'Zeta'), lessThan(_headerY(tester, 'Alpha')));
    expect(_headerY(tester, 'Alpha'), lessThan(_headerY(tester, 'Beta')));
    expect(find.text('host-a'), findsOneWidget);
    expect(find.text('host-b'), findsOneWidget);
  });

  testWidgets('ASCII group aliases keep hosts under the canonical group', (
    tester,
  ) async {
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => [_host('a', 'ALPHA')],
          loadGroups: () async => const [
            HostGroup(name: 'Alpha', collapsed: false),
          ],
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('group-header-Alpha')), findsOneWidget);
    expect(find.byKey(const ValueKey('group-header-ALPHA')), findsNothing);
    expect(find.text('host-a'), findsOneWidget);
  });

  testWidgets('non ASCII group names stay distinct and Home stays first', (
    tester,
  ) async {
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => [
            _host('home', 'default'),
            _host('upper', 'Ä'),
            _host('lower', 'ä'),
          ],
          loadGroups: () async => const [
            HostGroup(name: 'ä', collapsed: false),
            HostGroup(name: 'Ä', collapsed: false),
          ],
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('host-upper'), findsOneWidget);
    expect(find.text('host-lower'), findsOneWidget);
    expect(
      tester.getTopLeft(find.text('host-home')).dy,
      lessThan(_headerY(tester, 'ä')),
    );
    expect(_headerY(tester, 'ä'), lessThan(_headerY(tester, 'Ä')));
    expect(find.byKey(const ValueKey('group-drag-default')), findsNothing);
    expect(find.byKey(const ValueKey('group-drag-system')), findsNothing);
  });
}
