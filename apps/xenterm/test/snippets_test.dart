import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/screens/snippets/snippets_screen.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 1100, height: 760, child: child)),
);

void main() {
  const nginx = QuickCommand(
    index: 0,
    name: 'Reload nginx',
    command: 'sudo nginx -s reload',
    group: 'ops',
  );

  testWidgets('snippets load from the injected store and open on tap', (
    tester,
  ) async {
    await tester.pumpWidget(
      _app(
        SnippetsScreen(loadSnippets: () async => [nginx]),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Reload nginx'), findsOneWidget);
    expect(find.text('sudo nginx -s reload'), findsOneWidget);

    // This screen is the manager; running happens in the terminal sidebar.
    await tester.tap(find.byKey(const ValueKey('snippet-0')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('snippet-command')), findsOneWidget);
    expect(find.byKey(const ValueKey('snippet-run')), findsNothing);
  });

  testWidgets('a new snippet is written through the injected saver', (
    tester,
  ) async {
    final saved = <Map<String, Object?>>[];
    var current = <QuickCommand>[];
    await tester.pumpWidget(
      _app(
        SnippetsScreen(
          loadSnippets: () async => current,
          saveSnippet:
              ({
                required index,
                required name,
                required command,
                required group,
                required sendEnter,
              }) async {
                saved.add({
                  'index': index,
                  'name': name,
                  'command': command,
                  'group': group,
                  'sendEnter': sendEnter,
                });
                current = [
                  QuickCommand(
                    index: 0,
                    name: name,
                    command: command,
                    group: group,
                    sendEnter: sendEnter,
                  ),
                ];
                return const CoreWriteResult(ok: true, id: '0');
              },
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('New snippet'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('snippet-name')),
        matching: find.byType(EditableText),
      ),
      'Deploy',
    );
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('snippet-command')),
        matching: find.byType(EditableText),
      ),
      'git pull && deploy',
    );
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('snippet-group')),
        matching: find.byType(EditableText),
      ),
      'release',
    );
    await tester.tap(find.byKey(const ValueKey('snippet-save')));
    await tester.pumpAndSettle();
    expect(saved.single['index'], -1);
    expect(saved.single['name'], 'Deploy');
    expect(saved.single['command'], 'git pull && deploy');
    expect(saved.single['group'], 'release');
    expect(saved.single['sendEnter'], isTrue);
    expect(find.text('Deploy'), findsOneWidget);
  });

  testWidgets('the editor deletes an existing snippet through its callback', (
    tester,
  ) async {
    var deleted = 0;
    await tester.pumpWidget(
      _app(
        SnippetEditor(
          snippet: nginx,
          onSave: ({
            required index,
            required name,
            required command,
            required group,
            required sendEnter,
          }) async => null,
          onDelete: (snippet) async {
            deleted = snippet.index;
            return null;
          },
          onClose: () {},
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('snippet-delete')));
    await tester.pumpAndSettle();
    expect(deleted, 0);
  });
}
