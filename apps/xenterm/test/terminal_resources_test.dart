import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/screens/terminal/terminal_tools_panel.dart';
import 'package:xenterm_app/theme/app_fonts.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 300, height: 900, child: child)),
);

Map<String, dynamic> _sample() => {
  'cpu_sampled': true,
  'cpu_percent': 0.42,
  'mem_used_kib': 1024 * 1024,
  'mem_total_kib': 2048 * 1024,
  'swap_used_kib': 0,
  'swap_total_kib': 0,
  'user': 'demo',
  'net': [
    {'name': 'eth0', 'rx': 4096, 'tx': 2048},
  ],
  'disks': [
    {'path': '/', 'available': 600000, 'total': 1000000},
  ],
  'gpus': [
    {
      'name': 'Test GPU',
      'util_percent': 35.0,
      'mem_used_mib': 1024,
      'mem_total_mib': 8192,
      'temp_c': 51,
    },
  ],
};

Widget _panel({
  TerminalSessionStatus status = TerminalSessionStatus.connected,
  String? resourceState,
  Map<String, dynamic>? resource,
  String search = '',
  int matchCount = 0,
}) => TerminalToolsPanel(
  fonts: AppFonts(),
  status: status,
  resourceState: resourceState,
  resource: resource,
  search: search,
  matchCount: matchCount,
  onSearch: (_) {},
  onClear: () {},
  onCopy: () {},
  onReconnect: () {},
);

void main() {
  testWidgets('the system panel renders live resource samples', (tester) async {
    await tester.pumpWidget(
      _app(_panel(resourceState: 'available', resource: _sample())),
    );
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('terminal-resource-cpu')), findsOne);
    expect(find.text('42%'), findsOne);
    expect(find.text('1.0 GiB / 2.0 GiB'), findsOne);
    expect(find.text('40%'), findsOne);
    expect(find.text('35% · 51°C'), findsOne);
    expect(find.textContaining('matching'), findsNothing);
  });

  testWidgets('unsupported and waiting states are stated, not hidden', (
    tester,
  ) async {
    await tester.pumpWidget(
      _app(_panel(resourceState: 'unsupported')),
    );
    await tester.pumpAndSettle();
    expect(find.text('Not available on this host'), findsOne);

    await tester.pumpWidget(_app(_panel(resourceState: 'waiting')));
    await tester.pumpAndSettle();
    expect(find.text('Measuring…'), findsOne);
  });

  testWidgets('the panel derives its session status from the enum, not a '
      'full screen snapshot', (tester) async {
    await tester.pumpWidget(_app(_panel()));
    await tester.pumpAndSettle();
    expect(find.text('Connected'), findsOne);

    await tester.pumpWidget(
      _app(_panel(status: TerminalSessionStatus.disconnected)),
    );
    await tester.pumpAndSettle();
    expect(find.text('Disconnected'), findsOne);
    expect(find.text('Connected'), findsNothing);
  });

  testWidgets('the match counter pluralises and reflects the cached count', (
    tester,
  ) async {
    await tester.pumpWidget(_app(_panel(search: 'Err', matchCount: 1)));
    await tester.pumpAndSettle();
    expect(find.text('1 match in view'), findsOne);

    await tester.pumpWidget(_app(_panel(search: 'Err', matchCount: 7)));
    await tester.pumpAndSettle();
    expect(find.text('7 matches in view'), findsOne);

    // Empty query shows the shortcut hint, never a stale count.
    await tester.pumpWidget(_app(_panel(matchCount: 7)));
    await tester.pumpAndSettle();
    expect(find.text('Ctrl+Shift+F'), findsOne);
    expect(find.textContaining('in view'), findsNothing);
  });
}
