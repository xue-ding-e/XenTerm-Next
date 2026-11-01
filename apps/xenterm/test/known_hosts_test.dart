import 'package:flutter/gestures.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/screens/known_hosts/known_hosts_screen.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 1100, height: 760, child: child)),
);

void main() {
  testWidgets('known hosts load real entries and can be forgotten', (
    tester,
  ) async {
    var hosts = <KnownHost>[
      const KnownHost(
        id: 'example.test:2222',
        keyType: 'ssh-ed25519',
        fingerprint: 'SHA256:abc123',
      ),
    ];
    String? deleted;
    await tester.pumpWidget(
      _app(
        KnownHostsScreen(
          loadHosts: () async => hosts,
          deleteHost: (id) async {
            deleted = id;
            hosts = [];
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('example.test:2222'), findsOneWidget);
    expect(find.text('ED25519'), findsOneWidget);
    expect(find.text('SHA256:abc123'), findsOneWidget);

    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer();
    await mouse.moveTo(
      tester.getCenter(
        find.byKey(const ValueKey('known-host-example.test:2222')),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(
      find.byKey(const ValueKey('known-host-remove-example.test:2222')),
    );
    await tester.pumpAndSettle();
    expect(deleted, 'example.test:2222');
    expect(find.text('No known hosts yet'), findsOneWidget);
  });

  testWidgets('the empty store shows an honest empty state', (tester) async {
    await tester.pumpWidget(
      _app(KnownHostsScreen(loadHosts: () async => const [])),
    );
    await tester.pumpAndSettle();
    expect(find.text('No known hosts yet'), findsOneWidget);
  });
}
