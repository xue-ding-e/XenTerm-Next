import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/screens/logs/logs_screen.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 1400, height: 800, child: child)),
);

SessionLogEntry _entry(String file, {int minutes = 0}) => SessionLogEntry(
  file: file,
  sessionId: 's1',
  name: 'gateway',
  host: '10.0.0.2',
  user: 'root',
  startedMs: DateTime(2026, 10, 18, 12, minutes).millisecondsSinceEpoch,
  size: 2048,
);

void main() {
  testWidgets('logs list, open and delete real transcripts', (tester) async {
    var logs = <SessionLogEntry>[_entry('a.jsonl')];
    String? opened;
    String? deleted;
    await tester.pumpWidget(
      _app(
        LogsScreen(
          loadLogs: () async => logs,
          readLog: (file) async {
            opened = file;
            return const SessionLogContent(
              text:
                  r'PS C:\Users\me> echo hi'
                  '\nhi\n',
              meta: {'name': 'gateway'},
            );
          },
          deleteLog: (file) async {
            deleted = file;
            logs = [];
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('2026-10-18 12:00'), findsOneWidget);
    expect(find.textContaining('gateway'), findsOneWidget);
    expect(find.textContaining('root'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('log-a.jsonl')));
    await tester.pumpAndSettle();
    expect(opened, 'a.jsonl');
    expect(find.textContaining('echo hi'), findsOneWidget);

    await tester.tap(find.byKey(const ValueKey('log-delete')));
    await tester.pumpAndSettle();
    expect(deleted, 'a.jsonl');
    expect(find.text('No session logs yet'), findsOneWidget);
  });

  testWidgets('empty logs point at the settings switch', (tester) async {
    await tester.pumpWidget(_app(LogsScreen(loadLogs: () async => const [])));
    await tester.pumpAndSettle();
    expect(find.text('No session logs yet'), findsOneWidget);
    expect(find.textContaining('Save session logs'), findsOneWidget);
  });
}
