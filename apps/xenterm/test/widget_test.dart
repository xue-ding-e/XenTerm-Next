import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:xenterm_app/data/hosts.dart';
import 'package:xenterm_app/main.dart';
import 'package:xenterm_app/theme/app_fonts.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  testWidgets('the shell mounts with the hosts screen showing', (tester) async {
    await tester.pumpWidget(XenTermApp(hostsLoader: () async => kSampleHosts));
    await tester.pumpAndSettle();
    expect(find.text('Home'), findsWidgets);
    // The toolbar's button and the grid's tail card both say it.
    expect(find.text('New host'), findsWidgets);
  });

  testWidgets('a host opens the terminal preview and can be closed', (
    tester,
  ) async {
    await tester.pumpWidget(XenTermApp(hostsLoader: () async => kSampleHosts));
    await tester.pumpAndSettle();
    // A plain card click connects immediately.
    await tester.tap(find.text('gateway-test02'));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('terminal-view')), findsOneWidget);
    // The desktop tools panel is live: search + session controls.
    expect(find.byKey(const ValueKey('terminal-search')), findsOneWidget);
    expect(find.byKey(const ValueKey('terminal-size-up')), findsOneWidget);
    // The session tab lives in the title bar; its × closes the terminal.
    await tester.tap(find.byKey(const ValueKey('tab-close-gw2')));
    await tester.pumpAndSettle();
    expect(find.text('New host'), findsWidgets);
  });

  testWidgets(
    'SFTP preview opens once, switches tabs, and closes independently',
    (tester) async {
      await tester.pumpWidget(
        XenTermApp(hostsLoader: () async => kSampleHosts),
      );
      await tester.pumpAndSettle();
      // The hover pencil opens the details drawer (edit path).
      await tester.tap(find.byKey(const ValueKey('edit-gw2')));
      await tester.pumpAndSettle();
      expect(find.text('Host Details'), findsOneWidget);
      await tester.tap(find.text('Connect').last);
      await tester.pumpAndSettle();

      expect(find.byKey(const ValueKey('terminal-view')), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('sftp-tab')));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('sftp-tab')), findsOneWidget);
      expect(find.text('SFTP · gateway-test02'), findsOneWidget);
      expect(find.text('SFTP · gateway-test02'), findsOneWidget);
      expect(find.byKey(const ValueKey('terminal-view')), findsNothing);

      await tester.tap(find.byKey(const ValueKey('sftp-tab')));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('sftp-tab')), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('tab-gw2')));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('terminal-view')), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('sftp-tab')));
      await tester.pumpAndSettle();
      expect(find.text('SFTP · gateway-test02'), findsOneWidget);

      await tester.tap(find.byKey(const ValueKey('tab-gw2')));
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('terminal-view')), findsOneWidget);
      // The session tab's × in the title bar ends the whole workspace.
      await tester.tap(find.byKey(const ValueKey('tab-close-gw2')));
      await tester.pumpAndSettle();
      expect(find.text('New host'), findsWidgets);
    },
  );

  testWidgets('font sizes are independent and survive navigation', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    final fonts = AppFonts();
    addTearDown(fonts.dispose);
    await tester.pumpWidget(
      XenTermApp(fonts: fonts, hostsLoader: () async => kSampleHosts),
    );
    await tester.tap(find.text('Settings'));
    await tester.pumpAndSettle();
    expect(find.text('Terminal settings'), findsOneWidget);
    expect(find.text('Font'), findsWidgets);
    expect(find.text('Text Size'), findsWidgets);
    final plus = find.byKey(const ValueKey('terminal-plus'));
    await tester.ensureVisible(plus);
    await tester.pumpAndSettle();
    await tester.tap(plus);
    await tester.pumpAndSettle();
    expect(fonts.terminalSize, 16);
    expect(fonts.uiSize, 14);
  });

  testWidgets('the saved UI language is restored on launch', (tester) async {
    SharedPreferences.setMockInitialValues({'xenterm.language': 'zh'});
    final fonts = AppFonts();
    addTearDown(fonts.dispose);
    await tester.pumpWidget(
      XenTermApp(fonts: fonts, hostsLoader: () async => kSampleHosts),
    );
    await tester.pumpAndSettle();
    // The navigation rail follows the persisted choice immediately.
    expect(find.text('首页'), findsWidgets);
    expect(find.text('密钥管理'), findsOneWidget);
    expect(find.text('设置'), findsOneWidget);
  });
}
