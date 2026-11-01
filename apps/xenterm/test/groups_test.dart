import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/data/hosts.dart';
import 'package:xenterm_app/screens/hosts/hosts_screen.dart';
import 'package:xenterm_app/theme/app_field.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 1400, height: 900, child: child)),
);

Host _host(String id, String label, {String group = ''}) => Host(
  id: id,
  label: label,
  address: '10.0.0.$id',
  port: 22,
  username: 'root',
  group: group,
  protocol: 'ssh',
);

void main() {
  testWidgets('groups render as folders and collapse through the core', (
    tester,
  ) async {
    final hosts = [
      _host('1', 'plain-host'),
      _host('2', 'db-primary', group: 'Production'),
    ];
    var groups = [
      const HostGroup(name: 'Production', collapsed: false),
      const HostGroup(name: 'Staging', collapsed: false),
    ];
    final toggles = <(String, bool)>[];
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => hosts,
          loadGroups: () async => groups,
          collapseGroup: (name, collapsed) async {
            toggles.add((name, collapsed));
            groups = [
              for (final group in groups)
                group.name == name
                    ? HostGroup(name: name, collapsed: collapsed)
                    : group,
            ];
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.byKey(const ValueKey('group-header-Production')), findsOne);
    expect(find.byKey(const ValueKey('group-header-Staging')), findsOne);
    expect(find.text('db-primary'), findsOne);
    expect(find.text('plain-host'), findsOne);

    await tester.tap(find.byKey(const ValueKey('group-header-Production')));
    await tester.pumpAndSettle();
    expect(toggles, [('Production', true)]);
    expect(find.text('db-primary'), findsNothing);
    expect(find.text('plain-host'), findsOne);

    await tester.tap(find.byKey(const ValueKey('group-header-Production')));
    await tester.pumpAndSettle();
    expect(toggles.last, ('Production', false));
    expect(find.text('db-primary'), findsOne);
  });

  testWidgets('New host ▾ > New group creates a persisted folder', (
    tester,
  ) async {
    var groups = <HostGroup>[];
    String? created;
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => [_host('1', 'solo')],
          loadGroups: () async => groups,
          createGroup: (name) async {
            created = name;
            groups = [HostGroup(name: name, collapsed: false)];
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.byIcon(LucideIcons.chevronDown).first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('New group'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byKey(const ValueKey('group-name')),
      'Production',
    );
    await tester.tap(find.byKey(const ValueKey('group-save')));
    await tester.pumpAndSettle();

    expect(created, 'Production');
    expect(find.byKey(const ValueKey('group-header-Production')), findsOne);
    expect(find.text('No hosts in this group yet.'), findsOne);
  });

  testWidgets('Import hosts reports the core result in a banner', (
    tester,
  ) async {
    var hosts = <Host>[];
    var imported = false;
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => hosts,
          loadGroups: () async => const <HostGroup>[],
          importHosts: () async {
            imported = true;
            hosts = [_host('9', 'from-ssh-config')];
            return const ImportResult(
              ok: true,
              added: 1,
              skipped: 0,
              message: 'Imported 1 host',
            );
          },
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.byIcon(LucideIcons.chevronDown).first);
    await tester.pumpAndSettle();
    await tester.tap(find.text('Import hosts'));
    await tester.pumpAndSettle();

    expect(imported, isTrue);
    expect(find.byKey(const ValueKey('import-banner')), findsOne);
    expect(find.text('Imported 1 host'), findsOne);
    expect(find.text('from-ssh-config'), findsOne);

    await tester.tap(find.byKey(const ValueKey('import-dismiss')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('import-banner')), findsNothing);
  });

  testWidgets('group rename and delete keep the store in sync', (tester) async {
    var groups = [const HostGroup(name: 'Production', collapsed: false)];
    String? renamedFrom;
    String? renamedTo;
    String? deleted;
    await tester.pumpWidget(
      _app(
        HostsScreen(
          loadHosts: () async => [_host('1', 'db', group: 'Production')],
          loadGroups: () async => groups,
          renameGroup: (old, next) async {
            renamedFrom = old;
            renamedTo = next;
            groups = [HostGroup(name: next, collapsed: false)];
            return const CoreWriteResult(ok: true);
          },
          deleteGroup: (name) async {
            deleted = name;
            groups = const [];
            return const CoreWriteResult(ok: true);
          },
        ),
      ),
    );
    await tester.pumpAndSettle();

    await tester.tap(find.byKey(const ValueKey('group-rename-Production')));
    await tester.pumpAndSettle();
    expect(
      tester
          .widget<XtField>(find.byKey(const ValueKey('group-name')))
          .controller
          ?.text,
      'Production',
    );
    await tester.enterText(find.byKey(const ValueKey('group-name')), 'Prod');
    await tester.tap(find.byKey(const ValueKey('group-save')));
    await tester.pumpAndSettle();
    expect((renamedFrom, renamedTo), ('Production', 'Prod'));
    expect(find.byKey(const ValueKey('group-header-Prod')), findsOne);

    await tester.tap(find.byKey(const ValueKey('group-delete-Prod')));
    await tester.pumpAndSettle();
    expect(find.textContaining('Delete “Prod”?'), findsOne);
    await tester.tap(find.byKey(const ValueKey('group-delete-confirm')));
    await tester.pumpAndSettle();
    expect(deleted, 'Prod');
    expect(find.byKey(const ValueKey('group-header-Prod')), findsNothing);
  });
}
