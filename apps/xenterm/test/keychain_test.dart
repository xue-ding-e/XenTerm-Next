import 'package:flutter/gestures.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/screens/keychain/keychain_screen.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 1100, height: 760, child: child)),
);

void main() {
  testWidgets('keychain lists only metadata and its real state', (
    tester,
  ) async {
    await tester.pumpWidget(
      _app(
        KeychainScreen(
          loadKeys: () async => const [
            KeyInfo(
              sessionId: 'h1',
              sessionName: 'gateway',
              host: '10.0.0.2',
              user: 'root',
              source: 'file',
              path: 'id_ed25519',
              fullPath: '/home/root/.ssh/id_ed25519',
              keyType: 'ssh-ed25519',
              fingerprint: 'SHA256:abc',
              status: 'available',
            ),
            KeyInfo(
              sessionId: 'h2',
              sessionName: 'locked-host',
              host: '10.0.0.3',
              user: 'root',
              source: 'inline',
              path: '',
              fullPath: '',
              keyType: '',
              fingerprint: '',
              status: 'locked',
            ),
          ],
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('id_ed25519'), findsOneWidget);
    expect(find.text('ED25519'), findsOneWidget);
    expect(find.textContaining('SHA256:abc'), findsOneWidget);
    expect(find.text('Pasted key'), findsOneWidget);
    expect(find.textContaining('Encrypted key'), findsOneWidget);
  });

  testWidgets('a key can be repointed and removed inline', (tester) async {
    String? updatedOld;
    String? updatedNew;
    String? deleted;
    var keys = const [
      KeyInfo(
        sessionId: 'h1',
        sessionName: 'gateway',
        host: '10.0.0.2',
        user: 'root',
        source: 'file',
        path: 'id_ed25519',
        fullPath: '/home/root/.ssh/id_ed25519',
        keyType: 'ssh-ed25519',
        fingerprint: 'SHA256:abc',
        status: 'available',
      ),
    ];
    await tester.pumpWidget(
      _app(
        KeychainScreen(
          loadKeys: () async => keys,
          updateKey: ({required oldPath, required newPath}) async {
            updatedOld = oldPath;
            updatedNew = newPath;
            keys = const [];
            return const CoreWriteResult(ok: true);
          },
          deleteKey: (path) async {
            deleted = path;
            keys = const [];
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();

    final hover = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await hover.addPointer(location: Offset.zero);
    await tester.pump();
    await hover.moveTo(tester.getCenter(find.byKey(const ValueKey('key-h1'))));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('key-edit-h1')));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('key-path')),
      '/home/root/.ssh/id_new',
    );
    await tester.tap(find.byKey(const ValueKey('key-save')));
    await tester.pumpAndSettle();
    expect(
      (updatedOld, updatedNew),
      ('/home/root/.ssh/id_ed25519', '/home/root/.ssh/id_new'),
    );

    await hover.removePointer();
    await tester.pump();

    // Reload brings the key back above; remove it through the inline confirm.
    keys = const [
      KeyInfo(
        sessionId: 'h1',
        sessionName: 'gateway',
        host: '10.0.0.2',
        user: 'root',
        source: 'file',
        path: 'id_ed25519',
        fullPath: '/home/root/.ssh/id_ed25519',
        keyType: 'ssh-ed25519',
        fingerprint: 'SHA256:abc',
        status: 'available',
      ),
    ];
    await tester.pumpWidget(
      _app(
        KeychainScreen(
          key: UniqueKey(),
          loadKeys: () async => keys,
          deleteKey: (path) async {
            deleted = path;
            keys = const [];
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    final hover2 = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await hover2.addPointer(location: Offset.zero);
    await tester.pump();
    await hover2.moveTo(tester.getCenter(find.byKey(const ValueKey('key-h1'))));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('key-delete-h1')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('key-delete-confirm')), findsOne);
    await tester.tap(find.byKey(const ValueKey('key-delete-confirm')));
    await tester.pumpAndSettle();
    expect(deleted, '/home/root/.ssh/id_ed25519');
  });

  testWidgets('the empty keychain explains how keys appear', (tester) async {
    await tester.pumpWidget(
      _app(KeychainScreen(loadKeys: () async => const [])),
    );
    await tester.pumpAndSettle();
    expect(find.text('No SSH keys yet'), findsOneWidget);
  });
}
