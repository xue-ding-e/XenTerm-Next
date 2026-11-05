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

SftpEntry _updatedFile({
  String name = 'notes.txt',
  bool isDir = false,
  int size = 2048,
  int modified = 1760000000,
  int mode = 420,
}) => SftpEntry(
  name: name,
  path: '/home/me/notes.txt',
  isDir: isDir,
  size: size,
  modified: modified,
  mode: mode,
);

Future<void> _selectFile(WidgetTester tester) async {
  await tester.tap(find.byKey(const ValueKey('sftp-/home/me/notes.txt')));
  await tester.pump(const Duration(milliseconds: 400));
}

void main() {
  testWidgets('directory change clears the previous file selection', (
    tester,
  ) async {
    final downloaded = <SftpEntry>[];
    Widget browser(SftpSnapshot snapshot) =>
        _app(SftpBrowser(snapshot: snapshot, onDownload: downloaded.add));
    await tester.pumpWidget(browser(_snapshot));
    await tester.pumpAndSettle();
    await _selectFile(tester);
    expect(find.byIcon(LucideIcons.download), findsOneWidget);
    expect(find.byIcon(LucideIcons.trash2), findsOneWidget);

    await tester.pumpWidget(browser(const SftpSnapshot(path: '/other')));
    await tester.pumpAndSettle();
    expect(find.text('/other'), findsOneWidget);
    expect(
      find.byIcon(LucideIcons.download),
      findsNothing,
      reason: 'navigation kept a download action for the previous directory',
    );
    expect(find.byIcon(LucideIcons.trash2), findsNothing);
    expect(downloaded, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('directory change cancels a pending delete confirmation', (
    tester,
  ) async {
    final deleted = <SftpEntry>[];
    Widget browser(SftpSnapshot snapshot) =>
        _app(SftpBrowser(snapshot: snapshot, onDelete: deleted.add));
    await tester.pumpWidget(browser(_snapshot));
    await tester.pumpAndSettle();
    await _selectFile(tester);
    await tester.tap(find.byIcon(LucideIcons.trash2));
    await tester.pumpAndSettle();
    expect(find.textContaining('Delete “notes.txt”'), findsOneWidget);

    await tester.pumpWidget(browser(const SftpSnapshot(path: '/other')));
    await tester.pumpAndSettle();
    expect(find.textContaining('Delete “notes.txt”'), findsNothing);
    expect(find.text('Delete'), findsNothing);
    expect(deleted, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('directory change cancels a new folder draft', (tester) async {
    final created = <String>[];
    Widget browser(SftpSnapshot snapshot) =>
        _app(SftpBrowser(snapshot: snapshot, onCreateFolder: created.add));
    await tester.pumpWidget(browser(_snapshot));
    await tester.pumpAndSettle();
    await tester.tap(find.byIcon(LucideIcons.folderPlus));
    await tester.pumpAndSettle();
    final input = find.descendant(
      of: find.byKey(const ValueKey('sftp-new-folder-name')),
      matching: find.byType(EditableText),
    );
    await tester.enterText(input, 'old-directory-draft');

    await tester.pumpWidget(browser(const SftpSnapshot(path: '/other')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('sftp-new-folder-name')), findsNothing);
    expect(find.text('Create'), findsNothing);
    expect(created, isEmpty);
    await tester.tap(find.byIcon(LucideIcons.folderPlus));
    await tester.pumpAndSettle();
    expect(tester.widget<EditableText>(input).controller.text, isEmpty);
    expect(tester.takeException(), isNull);
  });

  testWidgets('same directory refresh keeps selection with fresh metadata', (
    tester,
  ) async {
    final downloaded = <SftpEntry>[];
    Widget browser(SftpSnapshot snapshot) =>
        _app(SftpBrowser(snapshot: snapshot, onDownload: downloaded.add));
    await tester.pumpWidget(browser(_snapshot));
    await tester.pumpAndSettle();
    await _selectFile(tester);
    final updated = _updatedFile(size: 4096, modified: 1760000010);
    await tester.pumpWidget(
      browser(SftpSnapshot(path: '/home/me', entries: [updated])),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.byIcon(LucideIcons.download));
    await tester.pumpAndSettle();
    expect(downloaded, hasLength(1));
    expect(identical(downloaded.single, updated), isTrue);
    expect(tester.takeException(), isNull);
  });

  for (final change in [
    'removed',
    'type',
    'name',
    'size',
    'modified',
    'mode',
  ]) {
    testWidgets(
      'same directory $change clears an outdated delete confirmation',
      (tester) async {
        final deleted = <SftpEntry>[];
        Widget browser(SftpSnapshot snapshot) =>
            _app(SftpBrowser(snapshot: snapshot, onDelete: deleted.add));
        await tester.pumpWidget(browser(_snapshot));
        await tester.pumpAndSettle();
        await _selectFile(tester);
        await tester.tap(find.byIcon(LucideIcons.trash2));
        await tester.pumpAndSettle();
        expect(find.textContaining('Delete “notes.txt”'), findsOneWidget);
        final entry = _updatedFile(
          name: change == 'name' ? 'updated-name.txt' : 'notes.txt',
          isDir: change == 'type',
          size: change == 'size' ? 4096 : 2048,
          modified: change == 'modified' ? 1760000010 : 1760000000,
          mode: change == 'mode' ? 384 : 420,
        );
        await tester.pumpWidget(
          browser(
            SftpSnapshot(
              path: '/home/me',
              entries: change == 'removed' ? const [] : [entry],
            ),
          ),
        );
        await tester.pumpAndSettle();
        expect(find.textContaining('Delete “notes.txt”'), findsNothing);
        expect(find.text('Delete'), findsNothing);
        if (change == 'removed' || change == 'type') {
          expect(find.byIcon(LucideIcons.download), findsNothing);
          expect(find.byIcon(LucideIcons.trash2), findsNothing);
        } else {
          expect(find.byIcon(LucideIcons.download), findsOneWidget);
        }
        expect(deleted, isEmpty);
        expect(tester.takeException(), isNull);
      },
    );
  }

  testWidgets('unchanged entry refresh preserves its delete confirmation', (
    tester,
  ) async {
    final deleted = <SftpEntry>[];
    Widget browser(SftpSnapshot snapshot) =>
        _app(SftpBrowser(snapshot: snapshot, onDelete: deleted.add));
    await tester.pumpWidget(browser(_snapshot));
    await tester.pumpAndSettle();
    await _selectFile(tester);
    await tester.tap(find.byIcon(LucideIcons.trash2));
    await tester.pumpAndSettle();
    final unchanged = _updatedFile();
    await tester.pumpWidget(
      browser(SftpSnapshot(path: '/home/me', entries: [unchanged])),
    );
    await tester.pumpAndSettle();
    expect(find.textContaining('Delete “notes.txt”'), findsOneWidget);
    await tester.tap(find.text('Delete'));
    await tester.pumpAndSettle();
    expect(deleted, hasLength(1));
    expect(identical(deleted.single, unchanged), isTrue);
    expect(tester.takeException(), isNull);
  });

  testWidgets('entering a folder clears its parent listing selection', (
    tester,
  ) async {
    String? opened;
    Widget browser(SftpSnapshot snapshot) => _app(
      SftpBrowser(snapshot: snapshot, onOpenDirectory: (path) => opened = path),
    );
    await tester.pumpWidget(browser(_snapshot));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('sftp-/home/me/logs')));
    await tester.pump(const Duration(milliseconds: 400));
    await tester.pumpAndSettle();
    expect(opened, '/home/me/logs');
    await tester.pumpWidget(browser(const SftpSnapshot(path: '/home/me/logs')));
    await tester.pumpAndSettle();
    expect(find.byIcon(LucideIcons.trash2), findsNothing);
    expect(tester.takeException(), isNull);
  });

  testWidgets('same directory refresh preserves a new folder draft', (
    tester,
  ) async {
    final created = <String>[];
    Widget browser(SftpSnapshot snapshot) =>
        _app(SftpBrowser(snapshot: snapshot, onCreateFolder: created.add));
    await tester.pumpWidget(browser(_snapshot));
    await tester.pumpAndSettle();
    await tester.tap(find.byIcon(LucideIcons.folderPlus));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('sftp-new-folder-name')),
        matching: find.byType(EditableText),
      ),
      'preserved-draft',
    );
    await tester.pumpWidget(
      browser(SftpSnapshot(path: '/home/me', entries: [_updatedFile()])),
    );
    await tester.pumpAndSettle();
    await tester.tap(find.text('Create'));
    await tester.pumpAndSettle();
    expect(created, ['preserved-draft']);
    expect(tester.takeException(), isNull);
  });

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
