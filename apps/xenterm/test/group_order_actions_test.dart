import 'dart:async';
import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter_test/flutter_test.dart';
import 'package:flutter/services.dart';
import 'package:flutter/rendering.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/data/hosts.dart';
import 'package:xenterm_app/l10n/app_language.dart';
import 'package:xenterm_app/screens/hosts/hosts_screen.dart';
import 'package:xenterm_app/screens/hosts/host_card.dart';
import 'package:xenterm_app/screens/hosts/host_details_panel.dart';
import 'package:xenterm_app/theme/app_theme.dart';
import 'package:xenterm_app/widgets/primitives.dart';

Future<void> _viewport(WidgetTester tester) async {
  await tester.binding.setSurfaceSize(const Size(1400, 1000));
  addTearDown(() => tester.binding.setSurfaceSize(null));
}

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: RepaintBoundary(
    key: const ValueKey('group-test-capture'),
    child: Scaffold(child: child),
  ),
);

// Optional synthetic-widget evidence for the isolated validation workflow.
// Normal tests do not write files; no user sessions or host data are captured.
Future<void> _capture(WidgetTester tester, String name) async {
  final destination = Platform.environment['XENTERM_WIDGET_EVIDENCE_DIR'];
  if (destination == null || destination.isEmpty) return;
  final boundary = tester.renderObject<RenderRepaintBoundary>(
    find.byKey(const ValueKey('group-test-capture')),
  );
  await tester.runAsync(() async {
    final image = await boundary.toImage(pixelRatio: 1);
    try {
      final png = await image.toByteData(format: ui.ImageByteFormat.png);
      if (png == null) throw StateError('widget evidence encoding failed');
      final directory = Directory(destination);
      await directory.create(recursive: true);
      await File('${directory.path}/$name.png')
          .writeAsBytes(png.buffer.asUint8List(), flush: true);
    } finally {
      image.dispose();
    }
  });
}

const _initial = [
  HostGroup(name: 'Alpha', collapsed: true),
  HostGroup(name: 'Beta', collapsed: false),
  HostGroup(name: 'Zeta', collapsed: false),
];

Future<void> _manager(WidgetTester tester) async {
  await tester.tap(find.byIcon(LucideIcons.chevronDown).first);
  await tester.pumpAndSettle();
  await tester.tap(find.text('Manage groups'));
  await tester.pumpAndSettle();
}

void _expectOrder(
  WidgetTester tester,
  List<String> names, {
  bool manager = false,
}) {
  final prefix = manager ? 'group-order-row-' : 'group-header-';
  for (var i = 1; i < names.length; i++) {
    expect(
      tester.getTopLeft(find.byKey(ValueKey('$prefix${names[i - 1]}'))).dy,
      lessThan(
        tester.getTopLeft(find.byKey(ValueKey('$prefix${names[i]}'))).dy,
      ),
    );
  }
}

List<HostGroup> _ordered(List<HostGroup> current, List<String> order) => [
  for (final name in order) current.firstWhere((group) => group.name == name),
];

void main() {
  testWidgets('group manager moves groups and disables the end buttons', (
    tester,
  ) async {
    var groups = List<HostGroup>.of(_initial);
    final calls = <(List<String>, List<String>)>[];
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => const [],
          loadGroups: () async => groups,
          reorderGroups: (expected, order) async {
            calls.add((expected, order));
            groups = _ordered(groups, order);
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _manager(tester);
    expect(
      tester
          .widget<XtIconButton>(find.byKey(const ValueKey('group-up-Alpha')))
          .onPressed,
      isNull,
    );
    expect(
      tester
          .widget<XtIconButton>(find.byKey(const ValueKey('group-down-Zeta')))
          .onPressed,
      isNull,
    );
    await tester.tap(find.byKey(const ValueKey('group-down-Alpha')));
    await tester.pumpAndSettle();
    expect(calls.single.$1, ['Alpha', 'Beta', 'Zeta']);
    expect(calls.single.$2, ['Beta', 'Alpha', 'Zeta']);
    _expectOrder(tester, ['Beta', 'Alpha', 'Zeta'], manager: true);
    await _capture(tester, 'group-manager-reordered');
    _expectOrder(tester, ['Beta', 'Alpha', 'Zeta']);
    expect(
      groups.firstWhere((group) => group.name == 'Alpha').collapsed,
      isTrue,
    );
    await tester.tap(find.byKey(const ValueKey('group-up-Alpha')));
    await tester.pumpAndSettle();
    _expectOrder(tester, ['Alpha', 'Beta', 'Zeta']);
    expect(calls, hasLength(2));
  });

  testWidgets('group order buttons support Enter and Space', (tester) async {
    var groups = List<HostGroup>.of(_initial);
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => const [],
          loadGroups: () async => groups,
          reorderGroups: (expected, order) async {
            groups = _ordered(groups, order);
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _manager(tester);
    Focus.of(tester.element(find.byKey(const ValueKey('group-down-Alpha'))))
        .requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.enter);
    await tester.pumpAndSettle();
    _expectOrder(tester, ['Beta', 'Alpha', 'Zeta']);
    Focus.of(tester.element(find.byKey(const ValueKey('group-up-Alpha'))))
        .requestFocus();
    await tester.pump();
    await tester.sendKeyEvent(LogicalKeyboardKey.space);
    await tester.pumpAndSettle();
    _expectOrder(tester, ['Alpha', 'Beta', 'Zeta']);
  });

  testWidgets('group handles drag first to last and last to first', (
    tester,
  ) async {
    var groups = List<HostGroup>.of(_initial);
    final calls = <List<String>>[];
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => const [],
          loadGroups: () async => groups,
          reorderGroups: (expected, order) async {
            calls.add(order);
            groups = _ordered(groups, order);
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    final first = tester.getCenter(
      find.byKey(const ValueKey('group-drag-Alpha')),
    );
    final last = tester.getCenter(
      find.byKey(const ValueKey('group-drop-Zeta')),
    );
    await tester.dragFrom(first, last - first);
    await tester.pumpAndSettle();
    expect(calls.single, ['Beta', 'Zeta', 'Alpha']);
    _expectOrder(tester, ['Beta', 'Zeta', 'Alpha']);
    await _capture(tester, 'group-list-drag-reordered');
    final from = tester.getCenter(
      find.byKey(const ValueKey('group-drag-Alpha')),
    );
    final to = tester.getCenter(find.byKey(const ValueKey('group-drop-Beta')));
    await tester.dragFrom(from, to - from);
    await tester.pumpAndSettle();
    expect(calls.last, ['Alpha', 'Beta', 'Zeta']);
    _expectOrder(tester, ['Alpha', 'Beta', 'Zeta']);
  });

  testWidgets(
    'pending group order blocks repeated sorting and group mutations',
    (tester) async {
      var groups = List<HostGroup>.of(_initial);
      final result = Completer<CoreWriteResult>();
      var calls = 0;
      await _viewport(tester);
      await tester.pumpWidget(
        _app(
          HostsScreen(
            loadHosts: () async => const [],
            loadGroups: () async => groups,
            reorderGroups: (expected, order) {
              calls++;
              return result.future;
            },
          ),
        ),
      );
      await tester.pumpAndSettle();
      await _manager(tester);
      await tester.tap(find.byKey(const ValueKey('group-down-Alpha')));
      await tester.pump();
      expect(find.text('Saving group order…'), findsOneWidget);
      expect(
        tester
            .widget<XtIconButton>(find.byKey(const ValueKey('group-down-Beta')))
            .onPressed,
        isNull,
      );
      expect(
        tester
            .widget<XtIconButton>(
              find.byKey(const ValueKey('group-rename-Alpha')),
            )
            .onPressed,
        isNull,
      );
      expect(
        tester
            .widget<XtIconButton>(
              find.byKey(const ValueKey('group-delete-Alpha')),
            )
            .onPressed,
        isNull,
      );
      _expectOrder(tester, ['Alpha', 'Beta', 'Zeta']);
      groups = _ordered(groups, ['Beta', 'Alpha', 'Zeta']);
      result.complete(const CoreWriteResult(ok: true));
      await tester.pumpAndSettle();
      expect(calls, 1);
      _expectOrder(tester, ['Beta', 'Alpha', 'Zeta']);
    },
  );

  for (final throws in [false, true]) {
    testWidgets(
      'failed group reorder ${throws ? 'exception' : 'response'} keeps confirmed order',
      (tester) async {
        var loads = 0;
        await _viewport(tester);
        await tester.pumpWidget(
          _app(
            HostsScreen(
              loadHosts: () async => const [],
              loadGroups: () async {
                loads++;
                return _initial;
              },
              reorderGroups: (expected, order) async {
                if (throws) throw StateError('disk conflict');
                return const CoreWriteResult(ok: false, error: 'disk conflict');
              },
            ),
          ),
        );
        await tester.pumpAndSettle();
        await _manager(tester);
        await tester.tap(find.byKey(const ValueKey('group-down-Alpha')));
        await tester.pumpAndSettle();
        expect(loads, 2);
        expect(find.textContaining('disk conflict'), findsOneWidget);
        _expectOrder(tester, ['Alpha', 'Beta', 'Zeta']);
        expect(
          tester
              .widget<XtIconButton>(
                find.byKey(const ValueKey('group-down-Alpha')),
              )
              .onPressed,
          isNotNull,
        );
      },
    );
  }

  testWidgets('disposed group manager ignores a late reorder response', (
    tester,
  ) async {
    final result = Completer<CoreWriteResult>();
    var loads = 0;
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => const [],
          loadGroups: () async {
            loads++;
            return _initial;
          },
          reorderGroups: (expected, order) => result.future,
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _manager(tester);
    await tester.tap(find.byKey(const ValueKey('group-down-Alpha')));
    await tester.pump();
    await _viewport(tester);
    await tester.pumpWidget(const SizedBox.shrink());
    result.complete(const CoreWriteResult(ok: false, error: 'late failure'));
    await tester.pump();
    expect(loads, 1);
    expect(tester.takeException(), isNull);
  });

  testWidgets('search hides drag handles while manager reorders all groups', (
    tester,
  ) async {
    var groups = List<HostGroup>.of(_initial);
    List<String>? expectedNames;
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => const <Host>[],
          loadGroups: () async => groups,
          reorderGroups: (expected, order) async {
            expectedNames = expected;
            groups = _ordered(groups, order);
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.enterText(find.byKey(const ValueKey('host-search')), 'Alpha');
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('group-drag-Alpha')), findsNothing);
    await _manager(tester);
    await tester.tap(find.byKey(const ValueKey('group-down-Alpha')));
    await tester.pumpAndSettle();
    expect(expectedNames, ['Alpha', 'Beta', 'Zeta']);
    await tester.enterText(find.byKey(const ValueKey('host-search')), '');
    await tester.pumpAndSettle();
    _expectOrder(tester, ['Beta', 'Alpha', 'Zeta']);
  });

  testWidgets('empty group manager has no reorder action', (tester) async {
    var calls = 0;
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => const [],
          loadGroups: () async => const [],
          reorderGroups: (expected, order) async {
            calls++;
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await _manager(tester);
    expect(find.text('No groups to reorder'), findsOneWidget);
    expect(calls, 0);
  });

  for (final staleError in [false, true]) {
    testWidgets(
      'old group reload ${staleError ? 'error' : 'success'} cannot replace a saved order',
      (tester) async {
        var groups = List<HostGroup>.of(_initial);
        final older = Completer<List<HostGroup>>();
        var loads = 0;
        const host = Host(
          id: 'one',
          label: 'host-one',
          address: 'example.invalid',
          port: 22,
          username: 'test',
          group: 'Beta',
          protocol: 'ssh',
        );
        await _viewport(tester);
        await tester.pumpWidget(
          _app(
            HostsScreen(
              loadHosts: () async => const [host],
              loadGroups: () {
                loads++;
                return loads == 2 ? older.future : Future.value(groups);
              },
              deleteSession: (_) async => const CoreWriteResult(ok: true),
              reorderGroups: (expected, order) async {
                groups = _ordered(groups, order);
                return const CoreWriteResult(ok: true);
              },
            ),
          ),
        );
        await tester.pumpAndSettle();
        await _manager(tester);
        // Use the real drawer callback to leave an earlier host-mutation reload
        // pending, while a later group-order write gets a newer snapshot.
        tester.widget<HostCard>(find.byType(HostCard).first).onEdit();
        await tester.pumpAndSettle();
        final pendingDelete = tester
            .widget<HostDetailsPanel>(find.byType(HostDetailsPanel))
            .onDelete(host);
        await tester.pumpAndSettle();
        expect(loads, 2);
        await tester.tap(find.byKey(const ValueKey('group-down-Alpha')));
        await tester.pumpAndSettle();
        expect(loads, 3);
        _expectOrder(tester, ['Beta', 'Alpha', 'Zeta'], manager: true);
        if (staleError) {
          older.completeError(StateError('obsolete group load'));
        } else {
          older.complete(_initial);
        }
        await pendingDelete;
        await tester.pumpAndSettle();
        _expectOrder(tester, ['Beta', 'Alpha', 'Zeta'], manager: true);
        expect(find.textContaining('obsolete group load'), findsNothing);
        expect(tester.takeException(), isNull);
      },
    );
  }

  testWidgets('cancelled and self group drags do not write an order', (
    tester,
  ) async {
    var calls = 0;
    await _viewport(tester);
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => const [],
          loadGroups: () async => _initial,
          reorderGroups: (expected, order) async {
            calls++;
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();
    final handle = tester.getCenter(
      find.byKey(const ValueKey('group-drag-Alpha')),
    );
    await tester.dragFrom(handle, const Offset(70, 0));
    await tester.pumpAndSettle();
    await tester.dragFrom(handle, Offset(10, 10) - handle);
    await tester.pumpAndSettle();
    expect(calls, 0);
    _expectOrder(tester, ['Alpha', 'Beta', 'Zeta']);
  });

  test('group order controls have Simplified Chinese labels', () {
    for (final text in [
      'Manage groups',
      'Move group up',
      'Move group down',
      'Drag to reorder group',
      'Saving group order…',
      'No groups to reorder',
      'Could not reorder groups',
    ]) {
      expect(AppLanguage.translate('zh', text), isNot(text));
    }
  });
}
