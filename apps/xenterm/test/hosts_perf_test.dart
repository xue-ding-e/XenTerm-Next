import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/data/hosts.dart';
import 'package:xenterm_app/screens/hosts/host_card.dart';
import 'package:xenterm_app/screens/hosts/host_details_panel.dart';
import 'package:xenterm_app/screens/hosts/hosts_screen.dart';
import 'package:xenterm_app/theme/app_field.dart';
import 'package:xenterm_app/theme/app_theme.dart';

/// Frame-work regressions for the hosts chrome: the drawer animation must not
/// relayout the grid, typing must not rebuild the header, and the animated
/// pieces must sit in their own repaint layers.
Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 1400, height: 900, child: child)),
);

const _hosts = <Host>[
  Host(id: 'h1', label: 'alpha', address: '10.0.0.1'),
  Host(id: 'h2', label: 'beta', address: '10.0.0.2'),
];

void main() {
  testWidgets('the details drawer slides over the grid instead of reflowing it', (
    tester,
  ) async {
    await tester.pumpWidget(_app(HostsScreen(loadHosts: () async => _hosts)));
    await tester.pumpAndSettle();
    final before = tester.getRect(find.byType(HostCard).first);

    // The hover pencil opens the drawer; inspect the grid halfway through the
    // 200ms slide.
    await tester.tap(find.byKey(const ValueKey('edit-h1')));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 90));
    final mid = tester.getRect(find.byType(HostCard).first);
    expect(mid.width, moreOrLessEquals(before.width));
    expect(mid.left, moreOrLessEquals(before.left));

    await tester.pumpAndSettle();
    final open = tester.getRect(find.byType(HostCard).first);
    expect(open.width, moreOrLessEquals(before.width),
        reason: 'the open drawer must not squeeze the card grid');
    final panel = tester.getRect(find.byType(HostDetailsPanel));
    final page = tester.getRect(find.byType(HostsScreen));
    expect(panel.width, 340);
    expect(panel.right, moreOrLessEquals(page.right),
        reason: 'the panel overlays the right edge of the page');

    // Closing is a reverse transform as well.
    await tester.tap(find.byIcon(LucideIcons.arrowRightToLine));
    await tester.pumpAndSettle();
    expect(
      tester.getRect(find.byType(HostCard).first).width,
      moreOrLessEquals(before.width),
    );
    expect(tester.takeException(), isNull);
  });

  testWidgets('typing filters after the debounce without rebuilding the header', (
    tester,
  ) async {
    await tester.pumpWidget(_app(HostsScreen(loadHosts: () async => _hosts)));
    await tester.pumpAndSettle();
    expect(find.text('alpha'), findsOneWidget);
    expect(find.text('beta'), findsOneWidget);
    final fieldBefore = tester.widget<XtField>(
      find.byKey(const ValueKey('host-search')),
    );

    await tester.enterText(find.byKey(const ValueKey('host-search')), 'alp');
    await tester.pump(const Duration(milliseconds: 30));
    // Inside the debounce window the results are still unfiltered.
    expect(find.text('beta'), findsOneWidget);
    final fieldDuring = tester.widget<XtField>(
      find.byKey(const ValueKey('host-search')),
    );
    expect(identical(fieldBefore, fieldDuring), isTrue,
        reason: 'a keystroke must not rebuild HostsScreen');
    expect(identical(fieldBefore, fieldDuring), isTrue);

    await tester.pump(const Duration(milliseconds: 120));
    // The debounced query rebuilds only the results area.
    expect(find.text('beta'), findsNothing);
    expect(find.text('alpha'), findsOneWidget);
    final fieldAfter = tester.widget<XtField>(
      find.byKey(const ValueKey('host-search')),
    );
    expect(identical(fieldBefore, fieldAfter), isTrue,
        reason: 'the header stays put while results update');
  });

  testWidgets('animated chrome sits in its own repaint layer', (tester) async {
    await tester.pumpWidget(_app(HostsScreen(loadHosts: () async => _hosts)));
    await tester.pumpAndSettle();
    expect(
      find.ancestor(
        of: find.byType(HostCard).first,
        matching: find.byType(RepaintBoundary),
      ),
      findsWidgets,
      reason: 'one card hover must not repaint the grid',
    );
    expect(
      find.ancestor(
        of: find.byType(GridView),
        matching: find.byType(RepaintBoundary),
      ),
      findsWidgets,
      reason: 'the results grid should be an isolated layer',
    );
  });
}
