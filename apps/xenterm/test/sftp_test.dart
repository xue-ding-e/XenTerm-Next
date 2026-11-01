import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/screens/terminal/sftp_view.dart';
import 'package:xenterm_app/theme/app_theme.dart';

Widget _app(Widget child) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 1000, height: 700, child: child)),
);

const _snapshot = SftpSnapshot(
  path: '/home/me',
  entries: [
    SftpEntry(
      name: 'logs',
      path: '/home/me/logs',
      isDir: true,
      size: 0,
      modified: 0,
      mode: 0,
    ),
    SftpEntry(
      name: 'notes.txt',
      path: '/home/me/notes.txt',
      isDir: false,
      size: 2048,
      modified: 1760000000,
      mode: 420, // 0o644
    ),
  ],
);

void main() {
  testWidgets('the browser lists a real snapshot and navigates on click', (
    tester,
  ) async {
    String? opened;
    await tester.pumpWidget(
      _app(
        SftpBrowser(
          snapshot: _snapshot,
          onOpenDirectory: (path) => opened = path,
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('/home/me'), findsOneWidget);
    expect(find.text('logs'), findsOneWidget);
    expect(find.text('notes.txt'), findsOneWidget);
    expect(find.text('2.0 KB'), findsOneWidget);
    expect(find.text('rw-r--r--'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('sftp-/home/me/logs')));
    await tester.pump(const Duration(milliseconds: 350));
    await tester.tap(find.byKey(const ValueKey('sftp-/home/me/logs')));
    await tester.pumpAndSettle();
    expect(opened, '/home/me/logs');
  });

  testWidgets('selection drives download and delete commands', (tester) async {
    SftpEntry? downloaded;
    SftpEntry? deleted;
    await tester.pumpWidget(
      _app(
        SftpBrowser(
          snapshot: _snapshot,
          onDownload: (entry) => downloaded = entry,
          onDelete: (entry) => deleted = entry,
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('sftp-/home/me/notes.txt')));
    await tester.pump(const Duration(milliseconds: 400));
    await tester.tap(find.byIcon(LucideIcons.download));
    await tester.pumpAndSettle();
    expect(downloaded?.path, '/home/me/notes.txt');

    await tester.tap(find.byIcon(LucideIcons.trash2));
    await tester.pumpAndSettle();
    expect(find.textContaining('Delete “notes.txt”'), findsOneWidget);
    await tester.tap(find.text('Delete'));
    await tester.pumpAndSettle();
    expect(deleted?.path, '/home/me/notes.txt');
  });

  testWidgets('new folder creates through the callback', (tester) async {
    String? created;
    await tester.pumpWidget(
      _app(
        SftpBrowser(
          snapshot: _snapshot,
          onCreateFolder: (name) => created = name,
        ),
      ),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byIcon(LucideIcons.folderPlus));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('sftp-new-folder-name')),
        matching: find.byType(EditableText),
      ),
      'backups',
    );
    await tester.tap(find.text('Create'));
    await tester.pumpAndSettle();
    expect(created, 'backups');
  });

  testWidgets('the folder tree expands and opens directories', (tester) async {
    String? opened;
    String? toggled;
    await tester.pumpWidget(
      _app(
        SftpTree(
          nodes: const [
            SftpTreeNode(
              path: '/',
              name: '/',
              depth: 0,
              expanded: true,
              hasChildren: true,
            ),
            SftpTreeNode(
              path: '/docs',
              name: 'docs',
              depth: 1,
              expanded: false,
              hasChildren: true,
            ),
          ],
          onOpen: (path) => opened = path,
          onToggle: (path) => toggled = path,
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('docs'), findsOneWidget);
    await tester.tap(find.text('docs'));
    await tester.pumpAndSettle();
    expect(opened, '/docs');
    await tester.tap(find.byIcon(LucideIcons.chevronRight));
    await tester.pumpAndSettle();
    expect(toggled, '/docs');
  });

  testWidgets('errors and empty folders are explicit', (tester) async {
    await tester.pumpWidget(
      _app(
        const SftpBrowser(
          snapshot: SftpSnapshot(path: '/root', error: 'Permission denied'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('Permission denied'), findsOneWidget);
  });
}
