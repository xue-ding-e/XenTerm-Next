import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart';
import 'package:xenterm_app/data/hosts.dart';
import 'package:xenterm_app/screens/terminal/sftp_view.dart';
import 'package:xenterm_app/screens/terminal/terminal_tools_panel.dart';
import 'package:xenterm_app/screens/terminal/terminal_view.dart';
import 'package:xenterm_app/shell/window_activity.dart';
import 'package:xenterm_app/theme/app_fonts.dart';
import 'package:xenterm_app/theme/app_theme.dart';

/// A fake native session so polling can be counted without the FFI bridge.
class _FakeTerminalSession implements TerminalSession {
  int versionValue = 1;
  int versionReads = 0;
  int snapshots = 0;
  int resizes = 0;
  int closes = 0;
  bool answered = false;

  @override
  int get version {
    versionReads++;
    return versionValue;
  }

  @override
  Map<String, dynamic>? snapshot() {
    snapshots++;
    return {
      'rows': [
        {
          'runs': [
            {
              'c': 4,
              't': 'test',
              's': {'fg': {'idx': 7}},
            },
          ],
          'text': 'test',
        },
      ],
      'cursor': [0, 4],
    };
  }

  @override
  bool send(List<int> bytes) => true;

  @override
  bool resize(int cols, int rows) {
    resizes++;
    return true;
  }

  @override
  bool answerPrompt(
    int promptId, {
    required bool accept,
    String? username,
    String? password,
    String? text,
  }) {
    answered = true;
    return true;
  }

  @override
  void close() {
    closes++;
  }
}

/// A fake SFTP worker so its polling can be counted without the FFI bridge.
class _FakeSftpSession implements SftpSession {
  int versionValue = 1;
  int versionReads = 0;
  int snapshots = 0;
  int closes = 0;
  int treeToggles = 0;

  static const _snapshot = SftpSnapshot(
    path: '/home/me',
    tree: [
      SftpTreeNode(
        path: '/',
        name: '/',
        depth: 0,
        expanded: false,
        hasChildren: true,
      ),
    ],
  );

  @override
  int get version {
    versionReads++;
    return versionValue;
  }

  @override
  SftpSnapshot? snapshot() {
    snapshots++;
    return _snapshot;
  }

  @override
  bool list(String path) => true;

  @override
  bool refresh() => true;

  @override
  bool toggleTree(String path) {
    treeToggles++;
    return true;
  }

  @override
  bool download(String remote, String localDir) => true;

  @override
  bool upload(String local) => true;

  @override
  bool createFolder(String name) => true;

  @override
  bool delete(String path) => true;

  @override
  bool answerPrompt(
    int promptId, {
    required bool accept,
    String? username,
    String? password,
    String? text,
  }) => true;

  @override
  void close() {
    closes++;
  }
}

Widget _host(TerminalView view) => ShadcnApp(
  theme: ThemeData(colorScheme: xtDarkScheme()),
  home: Scaffold(child: SizedBox(width: 900, height: 600, child: view)),
);

const _hostModel = Host(id: 'perf-test', label: 'Perf', address: 'localhost');

/// Drives the app-lifecycle channel the way the platform does.
Future<void> _setLifecycle(
  WidgetTester tester,
  AppLifecycleState state,
) async {
  final message = const StringCodec().encodeMessage(state.toString());
  await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
    'flutter/lifecycle',
    message,
    (_) {},
  );
}

void main() {
  // [windowActivity] is process-wide; each test ends with polling enabled so a
  // simulated window transition cannot silence the next test.
  tearDown(windowActivity.wake);

  testWidgets('an inactive pane never polls and activating it pulls at once', (
    tester,
  ) async {
    final session = _FakeTerminalSession();
    final fonts = AppFonts();
    addTearDown(fonts.dispose);
    var active = false;

    Widget build() => _host(
      TerminalView(
        host: _hostModel,
        fonts: fonts,
        active: active,
        sessionStarter: () => session,
      ),
    );

    await tester.pumpWidget(build());
    await tester.pump(const Duration(seconds: 2));
    expect(session.versionReads, 0, reason: 'hidden pane must not poll');
    expect(session.snapshots, 0);
    expect(session.resizes, 1, reason: 'layout still reports its size once');

    active = true;
    await tester.pumpWidget(build());
    // A zero-delay timer only runs when fake time advances.
    await tester.pump(const Duration(milliseconds: 1));
    expect(session.versionReads, greaterThan(0));
    expect(session.snapshots, 1, reason: 'activation pulls immediately');

    final reads = session.versionReads;
    await tester.pump(const Duration(seconds: 1));
    expect(session.versionReads, greaterThan(reads));

    // Hiding it again (tab switch / Home) cancels the timer.
    active = false;
    await tester.pumpWidget(build());
    await tester.pump(const Duration(milliseconds: 1));
    final paused = session.versionReads;
    await tester.pump(const Duration(seconds: 2));
    expect(session.versionReads, paused, reason: 'hidden pane must stop');

    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(milliseconds: 1));
    expect(session.closes, 1, reason: 'unmount closes the session once');
  });

  testWidgets('a non-resumed app window pauses polling until it resumes', (
    tester,
  ) async {
    final session = _FakeTerminalSession();
    final fonts = AppFonts();
    addTearDown(fonts.dispose);

    await tester.pumpWidget(
      _host(
        TerminalView(
          host: _hostModel,
          fonts: fonts,
          sessionStarter: () => session,
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 300));
    expect(session.versionReads, greaterThan(1));

    await _setLifecycle(tester, AppLifecycleState.inactive);
    await tester.pump(const Duration(milliseconds: 1));
    final paused = session.versionReads;
    await tester.pump(const Duration(seconds: 2));
    expect(
      session.versionReads,
      paused,
      reason: 'minimised/unfocused window must not poll',
    );

    await _setLifecycle(tester, AppLifecycleState.resumed);
    await tester.pump(const Duration(milliseconds: 1));
    await tester.pump(const Duration(milliseconds: 200));
    expect(session.versionReads, greaterThan(paused));

    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(milliseconds: 1));
  });

  testWidgets('a tick paints the canvas without rebuilding the tools panel', (
    tester,
  ) async {
    final session = _FakeTerminalSession();
    final fonts = AppFonts();
    addTearDown(fonts.dispose);

    await tester.pumpWidget(
      _host(
        TerminalView(
          host: _hostModel,
          fonts: fonts,
          sessionStarter: () => session,
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 1));
    final panelBefore = tester.widget<TerminalToolsPanel>(
      find.byType(TerminalToolsPanel),
    );

    // New output: the version advances and the canvas must repaint.
    session.versionValue = 2;
    await tester.pump(const Duration(milliseconds: 100));
    expect(session.snapshots, greaterThan(1));

    final canvas = tester.widget<CustomPaint>(
      find.byKey(const ValueKey('terminal-canvas')),
    );
    expect(canvas.painter, isA<TerminalPainter>());
    // The panel is still the same throttled widget instance: ticks do not
    // rebuild it (its parent rebuild would have produced a new widget).
    final panelAfter = tester.widget<TerminalToolsPanel>(
      find.byType(TerminalToolsPanel),
    );
    expect(identical(panelBefore, panelAfter), isTrue);

    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(milliseconds: 1));
  });

  testWidgets('glyph painters are reused and the cache stays bounded', (
    tester,
  ) async {
    final cache = TerminalTextCache();
    final first = cache.obtain(
      text: 'cached run',
      fontFamily: 'Consolas',
      fontSize: 15,
      color: const Color(0xFFDDDDDD),
      bold: false,
    );
    final second = cache.obtain(
      text: 'cached run',
      fontFamily: 'Consolas',
      fontSize: 15,
      color: const Color(0xFFDDDDDD),
      bold: false,
    );
    expect(identical(first, second), isTrue, reason: 'no re-layout per frame');
    expect(cache.length, 1);

    // Styling participates in the key.
    cache.obtain(
      text: 'cached run',
      fontFamily: 'Consolas',
      fontSize: 15,
      color: const Color(0xFFDDDDDD),
      bold: true,
    );
    expect(cache.length, 2);

    for (var i = 0; i < TerminalTextCache.maxEntries + 40; i++) {
      cache.obtain(
        text: 'scroll-$i',
        fontFamily: 'Consolas',
        fontSize: 15,
        color: const Color(0xFFDDDDDD),
        bold: false,
      );
    }
    expect(cache.length, lessThanOrEqualTo(TerminalTextCache.maxEntries));
    cache.dispose();
  });

  test('shouldRepaint compares the snapshot identity and the paint config', () {
    final cache = TerminalTextCache();
    Map<String, dynamic> screen() => {
      'rows': [
        {
          'runs': [
            {
              'c': 4,
              't': 'test',
              's': {'fg': {'idx': 7}},
            },
          ],
          'text': 'test',
        },
      ],
      'cursor': [0, 1],
    };
    final frame = screen();
    TerminalPainter painter({
      Map<String, dynamic>? snapshot,
      String search = '',
      double fontSize = 15,
    }) => TerminalPainter(
      screen: snapshot,
      search: search,
      bold: false,
      cursorStyle: 'block',
      fontFamily: 'Consolas',
      fontSize: fontSize,
      cellWidth: 9,
      lineHeight: 18,
      textCache: cache,
    );

    expect(
      painter(snapshot: frame).shouldRepaint(painter(snapshot: frame)),
      isFalse,
    );
    expect(
      painter(snapshot: frame).shouldRepaint(painter(snapshot: screen())),
      isTrue,
    );
    expect(
      painter(
        snapshot: frame,
      ).shouldRepaint(painter(snapshot: frame, search: 'x')),
      isTrue,
    );
    expect(
      painter(
        snapshot: frame,
      ).shouldRepaint(painter(snapshot: frame, fontSize: 16)),
      isTrue,
    );
    cache.dispose();
  });

  testWidgets('a hidden SFTP view never polls and resumes when shown', (
    tester,
  ) async {
    final session = _FakeSftpSession();
    var active = false;

    Widget build() => ShadcnApp(
      theme: ThemeData(colorScheme: xtDarkScheme()),
      home: Scaffold(
        child: SizedBox(
          width: 1000,
          height: 700,
          child: SftpView(
            host: const Host(
              id: 'sftp-host',
              label: 'SFTP host',
              address: 'example.test',
              protocol: 'ssh',
            ),
            active: active,
            sessionStarter: () => session,
          ),
        ),
      ),
    );

    await tester.pumpWidget(build());
    await tester.pump(const Duration(seconds: 2));
    expect(session.versionReads, 0, reason: 'hidden SFTP view must not poll');
    expect(session.snapshots, 0);

    active = true;
    await tester.pumpWidget(build());
    await tester.pump(const Duration(milliseconds: 1));
    expect(session.versionReads, greaterThan(0));
    expect(session.snapshots, 1);

    final reads = session.versionReads;
    await tester.pump(const Duration(seconds: 1));
    expect(session.versionReads, greaterThan(reads));

    active = false;
    await tester.pumpWidget(build());
    await tester.pump(const Duration(milliseconds: 1));
    final paused = session.versionReads;
    await tester.pump(const Duration(seconds: 2));
    expect(session.versionReads, paused, reason: 'hidden SFTP view must stop');

    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(milliseconds: 1));
    expect(session.closes, 1);
  });

  testWidgets('a non-resumed window pauses SFTP polling too', (tester) async {
    final session = _FakeSftpSession();
    await tester.pumpWidget(
      ShadcnApp(
        theme: ThemeData(colorScheme: xtDarkScheme()),
        home: Scaffold(
          child: SizedBox(
            width: 1000,
            height: 700,
            child: SftpView(
              host: const Host(
                id: 'sftp-host',
                label: 'SFTP host',
                address: 'example.test',
                protocol: 'ssh',
              ),
              sessionStarter: () => session,
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 300));
    expect(session.versionReads, greaterThan(1));

    await _setLifecycle(tester, AppLifecycleState.inactive);
    await tester.pump(const Duration(milliseconds: 1));
    final paused = session.versionReads;
    await tester.pump(const Duration(seconds: 2));
    expect(session.versionReads, paused);

    await _setLifecycle(tester, AppLifecycleState.resumed);
    await tester.pump(const Duration(milliseconds: 1));
    await tester.pump(const Duration(milliseconds: 300));
    expect(session.versionReads, greaterThan(paused));

    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(milliseconds: 1));
  });

  testWidgets('a native window transition pauses terminal polling until it '
      'settles', (tester) async {
    final session = _FakeTerminalSession();
    final fonts = AppFonts();
    addTearDown(fonts.dispose);
    await tester.pumpWidget(
      _host(
        TerminalView(
          host: _hostModel,
          fonts: fonts,
          sessionStarter: () => session,
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 300));
    expect(session.versionReads, greaterThan(1));

    // Live resize / zoom: the whole surface repaints per frame, so polling
    // must stop for the duration.
    windowActivity.resize();
    await tester.pump(const Duration(milliseconds: 1));
    expect(windowActivity.isIdle, isFalse);
    final paused = session.versionReads;
    await tester.pump(const Duration(milliseconds: 100));
    expect(session.versionReads, paused, reason: 'no poll during the zoom');

    // The settle delay expires by itself and the first tick catches up.
    await tester.pump(const Duration(milliseconds: 400));
    expect(windowActivity.isIdle, isTrue);
    expect(session.versionReads, greaterThan(paused));

    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(milliseconds: 1));
  });

  testWidgets('a minimized window stays paused until it is restored', (
    tester,
  ) async {
    final session = _FakeTerminalSession();
    final fonts = AppFonts();
    addTearDown(fonts.dispose);
    await tester.pumpWidget(
      _host(
        TerminalView(
          host: _hostModel,
          fonts: fonts,
          sessionStarter: () => session,
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 300));
    expect(session.versionReads, greaterThan(1));

    windowActivity.minimize();
    await tester.pump(const Duration(milliseconds: 1));
    final paused = session.versionReads;
    await tester.pump(const Duration(seconds: 2));
    expect(
      session.versionReads,
      paused,
      reason: 'minimized windows must not keep polling behind the Dock',
    );

    // Restore: resume after one settle delay, like any other window animation.
    windowActivity.restore();
    await tester.pump(const Duration(milliseconds: 1));
    await tester.pump(const Duration(milliseconds: 400));
    expect(windowActivity.isIdle, isTrue);
    expect(session.versionReads, greaterThan(paused));

    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(milliseconds: 1));
  });

  testWidgets('a native window transition pauses SFTP polling too', (
    tester,
  ) async {
    final session = _FakeSftpSession();
    await tester.pumpWidget(
      ShadcnApp(
        theme: ThemeData(colorScheme: xtDarkScheme()),
        home: Scaffold(
          child: SizedBox(
            width: 1000,
            height: 700,
            child: SftpView(
              host: const Host(
                id: 'sftp-host',
                label: 'SFTP host',
                address: 'example.test',
                protocol: 'ssh',
              ),
              sessionStarter: () => session,
            ),
          ),
        ),
      ),
    );
    await tester.pump(const Duration(milliseconds: 300));
    expect(session.versionReads, greaterThan(1));

    windowActivity.minimize();
    await tester.pump(const Duration(milliseconds: 1));
    final paused = session.versionReads;
    await tester.pump(const Duration(seconds: 2));
    expect(session.versionReads, paused);

    windowActivity.wake();
    await tester.pump(const Duration(milliseconds: 1));
    await tester.pump(const Duration(milliseconds: 300));
    expect(session.versionReads, greaterThan(paused));

    await tester.pumpWidget(const SizedBox());
    await tester.pump(const Duration(milliseconds: 1));
  });
}
