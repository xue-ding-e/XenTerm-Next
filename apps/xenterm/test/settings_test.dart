import 'dart:async';

import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/l10n/app_language.dart';
import 'package:xenterm_app/screens/settings/settings_screen.dart';
import 'package:xenterm_app/terminal/core_settings.dart';
import 'package:xenterm_app/theme/app_fonts.dart';
import 'package:xenterm_app/theme/app_theme.dart';
import 'package:xenterm_app/widgets/primitives.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 1200, height: 900, child: child)),
);

/// Pumps the settings screen at a real desktop window size (the app defaults
/// to 1280×720 and the minimum is 940×600) without any fixed inner width.
Future<void> _pumpAtWindow(
  WidgetTester tester,
  Size window, {
  required AppFonts fonts,
  required CoreSettingsController controller,
}) async {
  tester.view.physicalSize = window;
  tester.view.devicePixelRatio = 1.0;
  addTearDown(tester.view.resetPhysicalSize);
  addTearDown(tester.view.resetDevicePixelRatio);
  await tester.pumpWidget(
    ShadcnApp(
      theme: ThemeData(colorScheme: xtDarkScheme()),
      home: Scaffold(
        child: SettingsScreen(
          lightTheme: false,
          onThemeChanged: (_) {},
          fonts: fonts,
          settings: controller,
          saveSettings: (_) async => null,
        ),
      ),
    ),
  );
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('closed settings ignore a late download folder selection', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    final fonts = AppFonts();
    addTearDown(fonts.dispose);
    final controller = CoreSettingsController()
      ..settings = const CoreSettings()
      ..loaded = true;
    addTearDown(controller.dispose);
    final selection = Completer<String?>();
    final saved = <Map<String, dynamic>>[];
    var pickerCalls = 0;
    await tester.pumpWidget(
      _app(
        SettingsScreen(
          lightTheme: false,
          onThemeChanged: (_) {},
          fonts: fonts,
          settings: controller,
          pickDownloadDirectory: () {
            pickerCalls++;
            return selection.future;
          },
          saveSettings: (values) async {
            saved.add(Map<String, dynamic>.of(values));
            return null;
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('SFTP').last);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Choose…'));
    await tester.pump();
    expect(pickerCalls, 1);
    expect(selection.isCompleted, isFalse);

    // Unmount before completing the picker; no real dialog/profile is used.
    await tester.pumpWidget(_app(const SizedBox.shrink()));
    expect(find.byType(SettingsScreen), findsNothing);
    selection.complete('/synthetic/downloads');
    await tester.pumpAndSettle();

    expect(tester.takeException(), isNull);
    expect(saved, isEmpty);
    expect(controller.settings.downloadDir, isEmpty);
  });

  for (final chosen in <String?>['/synthetic/downloads', null, '']) {
    testWidgets(
      'mounted folder picker handles ${chosen == null
          ? 'cancel'
          : chosen.isEmpty
          ? 'empty'
          : 'selection'}',
      (tester) async {
        SharedPreferences.setMockInitialValues({});
        final fonts = AppFonts();
        addTearDown(fonts.dispose);
        final controller = CoreSettingsController()
          ..settings = const CoreSettings()
          ..loaded = true;
        addTearDown(controller.dispose);
        final selection = Completer<String?>();
        final saved = <Map<String, dynamic>>[];
        var pickerCalls = 0;
        await tester.pumpWidget(
          _app(
            SettingsScreen(
              lightTheme: false,
              onThemeChanged: (_) {},
              fonts: fonts,
              settings: controller,
              pickDownloadDirectory: () {
                pickerCalls++;
                return selection.future;
              },
              saveSettings: (values) async {
                saved.add(Map<String, dynamic>.of(values));
                return null;
              },
            ),
          ),
        );
        await tester.pumpAndSettle();
        await tester.tap(find.text('SFTP').last);
        await tester.pumpAndSettle();
        await tester.tap(find.text('Choose…'));
        await tester.pump();
        expect(pickerCalls, 1);
        expect(saved, isEmpty);
        selection.complete(chosen);
        await tester.pumpAndSettle();

        expect(tester.takeException(), isNull);
        expect(
          saved,
          chosen == null || chosen.isEmpty
              ? isEmpty
              : equals([
                  {'download_dir': chosen},
                ]),
        );
      },
    );
  }

  testWidgets(
    'language menu switches settings labels without changing values',
    (tester) async {
      SharedPreferences.setMockInitialValues({});
      final fonts = AppFonts();
      addTearDown(fonts.dispose);
      final controller = CoreSettingsController()
        ..settings = const CoreSettings()
        ..loaded = true;
      addTearDown(controller.dispose);
      var language = 'en';
      var savedLanguage = 'en';
      await tester.pumpWidget(
        ShadcnApp(
          theme: ThemeData(colorScheme: xtDarkScheme()),
          home: StatefulBuilder(
            builder: (context, setState) => AppLanguage(
              code: language,
              onChanged: (code) => setState(() {
                language = code;
                savedLanguage = code;
              }),
              child: Scaffold(
                child: SizedBox(
                  width: 1200,
                  height: 900,
                  child: SettingsScreen(
                    lightTheme: false,
                    onThemeChanged: (_) {},
                    fonts: fonts,
                    settings: controller,
                    saveSettings: (_) async => null,
                  ),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();

      expect(find.text('Language'), findsOneWidget);
      await tester.tap(find.text('English'));
      await tester.pumpAndSettle();
      await tester.tap(
        find.descendant(
          of: find.byType(XtMenuPopup),
          matching: find.text('简体中文'),
        ),
      );
      await tester.pumpAndSettle();

      expect(savedLanguage, 'zh');
      expect(find.text('终端设置'), findsOneWidget);
      expect(find.text('方块'), findsOneWidget);
      // The selection changed the labels, not the stored settings.
      expect(controller.settings.cursorStyle, 'block');
      expect(controller.settings.lineSpacing, 1.0);
    },
  );

  testWidgets('renderer settings save through the core callback', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    final fonts = AppFonts();
    addTearDown(fonts.dispose);
    final controller = CoreSettingsController()
      ..settings = const CoreSettings()
      ..loaded = true;
    addTearDown(controller.dispose);
    final saved = <Map<String, dynamic>>[];
    await tester.pumpWidget(
      _app(
        SettingsScreen(
          lightTheme: false,
          onThemeChanged: (_) {},
          fonts: fonts,
          settings: controller,
          saveSettings: (values) async {
            saved.add(values);
            return null;
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Terminal settings'), findsOneWidget);

    final paddingRow = tester.getRect(
      find.byKey(const ValueKey('setting-padding')),
    );
    await tester.tapAt(Offset(paddingRow.right - 20, paddingRow.center.dy));
    await tester.pumpAndSettle();
    expect(saved.last['padding'], false);

    await tester.tap(find.text('Block'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Bar'));
    await tester.pumpAndSettle();
    expect(saved.last['cursor_style'], 'bar');

    await tester.tap(find.text('Logs'));
    await tester.pumpAndSettle();
    expect(find.text('Session logs'), findsOneWidget);
    final logsRow = tester.getRect(
      find.byKey(const ValueKey('setting-session-logs')),
    );
    await tester.tapAt(Offset(logsRow.right - 20, logsRow.center.dy));
    await tester.pumpAndSettle();
    expect(saved.last['session_logs'], true);
    expect(find.textContaining('app data folder'), findsOneWidget);
  });

  testWidgets('every tab fits the default 1280×720 window', (tester) async {
    SharedPreferences.setMockInitialValues({});
    final fonts = AppFonts();
    addTearDown(fonts.dispose);
    final controller = CoreSettingsController()
      ..settings = const CoreSettings()
      ..loaded = true;
    addTearDown(controller.dispose);
    await _pumpAtWindow(
      tester,
      const Size(1280, 720),
      fonts: fonts,
      controller: controller,
    );

    const tabs = ['Terminal', 'SFTP', 'Logs', 'Shortcuts'];
    const keysPerTab = <String, List<String>>{
      'Terminal': ['setting-padding', 'setting-bold', 'terminal-minus'],
      'SFTP': ['setting-sftp-follow'],
      'Logs': ['setting-session-logs'],
      'Shortcuts': [],
    };
    for (final tab in tabs) {
      await tester.tap(find.text(tab).last);
      await tester.pumpAndSettle();
      expect(tester.takeException(), isNull, reason: 'tab $tab overflowed');
      for (final key in keysPerTab[tab]!) {
        final rect = tester.getRect(find.byKey(ValueKey(key)));
        expect(
          rect.right,
          lessThanOrEqualTo(1280),
          reason: '$key is clipped at the window edge on $tab',
        );
        expect(rect.left, greaterThanOrEqualTo(0), reason: key);
      }
      // The value column of every row (dropdown/stepper/switch) is visible.
      expect(
        tester.getRect(find.byType(XtCard).first).width,
        lessThanOrEqualTo(700),
        reason: 'the settings card column exceeds its 700px cap on $tab',
      );
    }
  });

  testWidgets('controls stay visible at the 940×600 minimum window', (
    tester,
  ) async {
    SharedPreferences.setMockInitialValues({});
    final fonts = AppFonts();
    addTearDown(fonts.dispose);
    final controller = CoreSettingsController()
      ..settings = const CoreSettings()
      ..loaded = true;
    addTearDown(controller.dispose);
    await _pumpAtWindow(
      tester,
      const Size(940, 600),
      fonts: fonts,
      controller: controller,
    );
    expect(tester.takeException(), isNull);
    final row = tester.getRect(find.byKey(const ValueKey('setting-padding')));
    expect(row.right, lessThanOrEqualTo(940));
    final plus = tester.getRect(find.byKey(const ValueKey('terminal-plus')));
    expect(plus.right, lessThanOrEqualTo(940));
    expect(plus.left, greaterThanOrEqualTo(0));
  });
}
