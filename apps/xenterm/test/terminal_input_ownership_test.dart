import 'dart:async';
import 'dart:convert';

import 'package:flutter/scheduler.dart' show SchedulerBinding, SchedulerPhase;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart' as widgets show Navigator, RawDialogRoute;
import 'package:flutter_test/flutter_test.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:xenterm_app/bridge/native_bridge.dart' show QuickCommand;
import 'package:xenterm_app/data/hosts.dart';
import 'package:xenterm_app/screens/terminal/connection_prompt.dart';
import 'package:xenterm_app/screens/terminal/terminal_tools_panel.dart';
import 'package:xenterm_app/screens/terminal/terminal_view.dart';
import 'package:xenterm_app/shell/title_bar.dart';
import 'package:xenterm_app/shell/window_activity.dart';
import 'package:xenterm_app/terminal/terminal_command_bus.dart';
import 'package:xenterm_app/theme/app_fonts.dart';
import 'package:xenterm_app/theme/app_theme.dart';

class _RecordingSession implements TerminalSession {
  final List<int> sent = [];
  final List<int> answeredPromptIds = [];
  // Synthetic payloads only. Keep every argument to detect cross-session leaks.
  final List<Map<String, Object?>> answers = [];
  int snapshotCalls = 0;
  final snapshotPhases = <SchedulerPhase>[];
  int closes = 0;
  int _version = 1;
  Map<String, dynamic>? _prompt;
  String? readFailure;

  void setPrompt(Map<String, dynamic>? prompt) {
    _prompt = prompt;
    _version++;
  }

  @override
  int get version {
    if (readFailure == 'version') throw StateError('synthetic version failure');
    return _version;
  }

  @override
  Map<String, dynamic>? snapshot() {
    snapshotCalls++;
    snapshotPhases.add(SchedulerBinding.instance.schedulerPhase);
    if (readFailure == 'snapshot') throw StateError('synthetic snapshot failure');
    if (readFailure == 'empty') return null;
    return {
      'rows': [
        {
          'text': 'synthetic output',
          'runs': [
            {
              'c': 16,
              't': 'synthetic output',
              's': {'fg': {'idx': 7}},
            },
          ],
        },
      ],
      'cursor': [0, 0],
      if (_prompt != null) 'prompt': _prompt,
    };
  }

  @override
  bool send(List<int> bytes) {
    sent.addAll(bytes);
    return true;
  }

  @override
  bool resize(int cols, int rows) => true;

  @override
  bool answerPrompt(
    int promptId, {
    required bool accept,
    String? username,
    String? password,
    String? text,
  }) {
    answeredPromptIds.add(promptId);
    answers.add({
      'promptId': promptId,
      'accept': accept,
      'username': username,
      'password': password,
      'text': text,
    });
    return true;
  }

  @override
  void close() => closes++;
}

const _hosts = [
  Host(id: 'input-a', label: 'Input A', address: 'a.invalid'),
  Host(id: 'input-b', label: 'Input B', address: 'b.invalid'),
];

const _credentialPrompt = <String, dynamic>{
  'id': 41,
  'kind': 'credential',
  'need_user': false,
  'need_password': true,
};

/// Both panes exist from the first frame, as in the real shell. Switching a
/// titlebar tab must transfer ownership without tapping the terminal canvas.
class _Workspace extends StatefulWidget {
  const _Workspace({super.key, this.initialActive = 'input-a'});

  final String? initialActive;

  @override
  State<_Workspace> createState() => _WorkspaceState();
}

class _WorkspaceState extends State<_Workspace> {
  final fonts = AppFonts();
  final bus = TerminalCommandBus();
  final fieldFocus = FocusNode(debugLabel: 'unrelated-field');
  final sessions = [_RecordingSession(), _RecordingSession()];
  // Existing tests keep their original per-pane session. Reconnect cases
  // explicitly queue distinct replacements without changing other fixtures.
  final replacementSessions = <int, List<_RecordingSession>>{};
  final starts = <int>[0, 0];

  _RecordingSession startSession(int index) {
    starts[index]++;
    final queue = replacementSessions[index];
    if (queue != null && queue.isNotEmpty) return queue.removeAt(0);
    return sessions[index];
  }

  late String? selected = widget.initialActive;
  bool covered = false;

  void select(String id) => setState(() {
    selected = id;
    covered = false;
  });

  void cover() => setState(() => covered = true);

  @override
  void dispose() {
    fonts.dispose();
    bus.dispose();
    fieldFocus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Column(
    children: [
      TitleBar(
        openHosts: _hosts,
        activeHostId: selected,
        showSftp: covered,
        onSelectHost: select,
        onToggleSftp: cover,
      ),
      TextField(
        key: const ValueKey('unrelated-field'),
        focusNode: fieldFocus,
      ),
      Expanded(
        child: Stack(
          children: [
            for (var i = 0; i < _hosts.length; i++)
              Offstage(
                offstage: covered || selected != _hosts[i].id,
                child: TerminalView(
                  key: ValueKey(_hosts[i].id),
                  host: _hosts[i],
                  fonts: fonts,
                  commandBus: bus,
                  active: !covered && selected == _hosts[i].id,
                  sessionStarter: () => startSession(i),
                ),
              ),
          ],
        ),
      ),
    ],
  );
}

Future<_WorkspaceState> _mount(
  WidgetTester tester, {
  String? active = 'input-a',
}) async {
  final key = GlobalKey<_WorkspaceState>();
  await tester.pumpWidget(
    ShadcnApp(
      theme: ThemeData(colorScheme: xtDarkScheme()),
      home: Scaffold(child: _Workspace(key: key, initialActive: active)),
    ),
  );
  // Explicit frames, not pumpAndSettle: terminal polling is periodic.
  await tester.pump(const Duration(milliseconds: 1));
  await tester.pump();
  return key.currentState!;
}

Finder _pane(int index) => find.byKey(
  ValueKey(_hosts[index].id),
  skipOffstage: false,
);

FocusNode _terminalFocus(WidgetTester tester, int index) => tester
    .widgetList<Focus>(
      find.descendant(
        of: _pane(index),
        matching: find.byType(Focus, skipOffstage: false),
        skipOffstage: false,
      ),
    )
    .map((widget) => widget.focusNode)
    .whereType<FocusNode>()
    .singleWhere((node) => node.debugLabel == 'terminal');

TerminalToolsPanel _panel(WidgetTester tester, int index) =>
    tester.widget<TerminalToolsPanel>(
      find.descendant(
        of: _pane(index),
        matching: find.byType(TerminalToolsPanel, skipOffstage: false),
        skipOffstage: false,
      ),
    );

Finder _promptField(int index) => find.descendant(
  of: find.descendant(
    of: _pane(index),
    matching: find.byType(ConnectionPromptBanner, skipOffstage: false),
    skipOffstage: false,
  ),
  matching: find.byType(EditableText, skipOffstage: false),
  skipOffstage: false,
);

Future<FocusNode> _focusCredentialPrompt(
  WidgetTester tester,
  _WorkspaceState workspace,
) async {
  workspace.sessions[0].setPrompt(_credentialPrompt);
  await tester.pump(const Duration(milliseconds: 250));
  await tester.pump();
  final field = _promptField(0);
  await tester.tap(field);
  await tester.pump();
  final focus = tester.widget<EditableText>(field).focusNode;
  expect(focus.hasPrimaryFocus, isTrue);
  return focus;
}

Future<void> _switchTo(WidgetTester tester, int index) async {
  await tester.tap(find.text(_hosts[index].label));
  await tester.pump();
  await tester.pump();
}

Future<void> _typeX(WidgetTester tester, {bool repeat = false}) async {
  await tester.sendKeyDownEvent(LogicalKeyboardKey.keyX, character: 'x');
  if (repeat) {
    await tester.sendKeyRepeatEvent(LogicalKeyboardKey.keyX, character: 'x');
  }
  await tester.sendKeyUpEvent(LogicalKeyboardKey.keyX);
}

Future<void> _lifecycle(WidgetTester tester, AppLifecycleState state) async {
  await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
    'flutter/lifecycle',
    const StringCodec().encodeMessage(state.toString()),
    (_) {},
  );
  await tester.pump();
}

Future<void> _unmount(WidgetTester tester) async {
  await tester.pumpWidget(const SizedBox());
  await tester.pump();
}

// DRAFT: these regressions have not been compiled or run. Flutter 3.47.7
// AutomatedTestWidgetsFlutterBinding distinguishes pump() from
// pump(Duration.zero): only the latter elapses the fake timer queue before
// drawing. This intentionally exercises both reconnect frame orderings.
Finder _reconnectBanner() => find.descendant(
  of: _pane(0),
  matching: find.byType(ConnectionPromptBanner),
);

Finder _reconnectAction() => find.descendant(
  of: find.descendant(
    of: _pane(0),
    matching: find.byType(TerminalToolsPanel),
  ),
  matching: find.text('Reconnect'),
);

Finder _continueAction() => find.descendant(
  of: _reconnectBanner(),
  matching: find.text('Continue'),
);

Map<String, dynamic> _reconnectPrompt(String kind) => {
  'id': 1,
  'kind': kind,
  if (kind == 'credential') ...{
    'need_user': true,
    'need_password': true,
    'user': '',
  } else ...{
    'prompt': 'Synthetic verification challenge',
    'echo': false,
  },
};

Future<void> _enterReconnectFields(
  WidgetTester tester,
  String kind,
  String marker,
) async {
  final fields = _promptField(0);
  expect(fields, findsNWidgets(kind == 'credential' ? 2 : 1));
  if (kind == 'credential') {
    await tester.enterText(fields.at(0), 'synthetic-$marker-user');
    await tester.enterText(fields.at(1), 'synthetic-$marker-password');
  } else {
    await tester.enterText(fields.at(0), 'synthetic-$marker-code');
  }
}

Map<String, Object?> _reconnectAnswer(String kind, String marker) => {
  'promptId': 1,
  'accept': true,
  'username': kind == 'credential' ? 'synthetic-$marker-user' : '',
  'password': kind == 'credential' ? 'synthetic-$marker-password' : '',
  'text': kind == 'credential' ? '' : 'synthetic-$marker-code',
};

void main() {
  tearDown(windowActivity.wake);

  for (final kind in ['credential', 'mfa']) {
    for (final blankFrame in [false, true]) {
      testWidgets(
        'reconnect $kind same-id prompt clears fields ${blankFrame ? "after blank frame" : "before first frame"}',
        (tester) async {
          expect(tester.binding, isA<AutomatedTestWidgetsFlutterBinding>());
          final workspace = await _mount(tester);
          addTearDown(() => _unmount(tester));
          final a = workspace.sessions[0];
          final b = _RecordingSession()..setPrompt(_reconnectPrompt(kind));
          expect(b, isNot(same(a)));
          workspace.replacementSessions[0] = [b];
          a.setPrompt(_reconnectPrompt(kind));
          await tester.pump(const Duration(milliseconds: 250));
          await tester.pump();
          expect(_reconnectBanner(), findsOneWidget);
          expect(tester.widget<ConnectionPromptBanner>(
            _reconnectBanner(),
          ).prompt['id'], 1);
          await _enterReconnectFields(tester, kind, 'old');
          final oldTexts = tester.widgetList<EditableText>(
            _promptField(0),
          ).map((field) => field.controller.text).toList();
          expect(oldTexts, kind == 'credential'
              ? ['synthetic-old-user', 'synthetic-old-password']
              : ['synthetic-old-code']);
          final oldReads = a.snapshotCalls;
          expect(oldReads, greaterThan(0));
          expect(b.snapshotCalls, 0);
          expect(_reconnectAction().hitTestable(), findsOneWidget);

          await tester.tap(_reconnectAction());
          // A genuine toolbar tap must synchronously start B, before a frame.
          expect(workspace.starts[0], 2);
          expect(workspace.replacementSessions[0], isEmpty);
          expect(a.closes, 1);
          expect(b.closes, 0);
          expect(b.snapshotCalls, 0);
          expect(a.answers, isEmpty);
          expect(b.answers, isEmpty);
          if (blankFrame) {
            // Do not advance fake time: render null before the zero-delay poll.
            await tester.pump();
            // The frame drew null. Its focus post-frame callback then reads B
            // without rebuilding the banner until the next frame.
            expect(_reconnectBanner(), findsNothing);
            expect(b.snapshotCalls, 1);
            expect(b.snapshotPhases, [SchedulerPhase.postFrameCallbacks]);
          }
          // Run the zero-delay poll before drawing this frame. In the direct
          // case there has been NO intervening null-banner frame.
          await tester.pump(Duration.zero);
          expect(b.snapshotCalls, 1);
          expect(b.snapshotPhases, [blankFrame
              ? SchedulerPhase.postFrameCallbacks : SchedulerPhase.idle]);
          expect(a.snapshotCalls, oldReads);
          expect(_reconnectBanner(), findsOneWidget);
          expect(tester.widget<ConnectionPromptBanner>(
            _reconnectBanner(),
          ).prompt, equals(_reconnectPrompt(kind)));
          final fields = _promptField(0);
          expect(fields, findsNWidgets(kind == 'credential' ? 2 : 1));
          expect(tester.widgetList<EditableText>(fields)
              .map((field) => field.controller.text),
              everyElement(isEmpty));

          await _enterReconnectFields(tester, kind, 'new');
          expect(_continueAction().hitTestable(), findsOneWidget);
          await tester.tap(_continueAction());
          expect(a.answers, isEmpty);
          expect(b.answers, [_reconnectAnswer(kind, 'new')]);
          expect(workspace.sessions[1].answers, isEmpty);
          await _unmount(tester);
          expect(a.closes, 1);
          expect(b.closes, 1);
        },
      );
    }
  }

  testWidgets(
    'reconnect rejects old Continue pointer-up before rebuild',
    (tester) async {
      expect(tester.binding, isA<AutomatedTestWidgetsFlutterBinding>());
      final workspace = await _mount(tester);
      addTearDown(() => _unmount(tester));
      final a = workspace.sessions[0];
      final b = _RecordingSession()..setPrompt(_reconnectPrompt('credential'));
      workspace.replacementSessions[0] = [b];
      a.setPrompt(_reconnectPrompt('credential'));
      await tester.pump(const Duration(milliseconds: 250));
      await tester.pump();
      await _enterReconnectFields(tester, 'credential', 'old');
      expect(tester.widgetList<EditableText>(_promptField(0))
          .map((field) => field.controller.text),
          ['synthetic-old-user', 'synthetic-old-password']);
      expect(_continueAction().hitTestable(), findsOneWidget);
      expect(_reconnectAction().hitTestable(), findsOneWidget);
      final oldBanner = tester.state(_reconnectBanner());
      final oldReads = a.snapshotCalls;
      final held = await tester.startGesture(
        tester.getCenter(_continueAction()),
        pointer: 71,
      );
      var pointerDown = true;
      addTearDown(() async {
        if (pointerDown) await held.cancel();
      });
      // Supported pointer dispatch, not a captured/direct onAnswer invocation.
      // Neither this tap nor up() pumps. Two different controls/pointers are
      // necessary so reconnect can finish while the old Continue is held.
      await tester.tap(_reconnectAction(), pointer: 72);
      expect(workspace.starts[0], 2);
      expect(workspace.replacementSessions[0], isEmpty);
      expect(a.closes, 1);
      expect(b.closes, 0);
      expect(b.snapshotCalls, 0);
      expect(tester.state(_reconnectBanner()), same(oldBanner));
      expect(a.answers, isEmpty);
      expect(b.answers, isEmpty);
      await held.up();
      pointerDown = false;
      expect(b.snapshotCalls, 0);
      expect(a.snapshotCalls, oldReads);
      expect(a.answers, isEmpty);
      expect(b.answers, isEmpty,
        reason: 'A held Continue must not answer newly started session B.');
      // Flush B normally and show that a fresh answer is still accepted.
      await tester.pump(Duration.zero);
      expect(b.snapshotCalls, 1);
      expect(tester.widget<ConnectionPromptBanner>(
        _reconnectBanner(),
      ).prompt, equals(_reconnectPrompt('credential')));
      await _enterReconnectFields(tester, 'credential', 'new');
      await tester.tap(_continueAction());
      expect(a.answers, isEmpty);
      expect(b.answers, [_reconnectAnswer('credential', 'new')]);
      expect(workspace.sessions[1].answers, isEmpty);
      await _unmount(tester);
      expect(a.closes, 1);
      expect(b.closes, 1);
    },
  );


  for (final first in [0, 1]) {
    testWidgets('existing tab ${1 - first} takes input from tab $first', (
      tester,
    ) async {
      final workspace = await _mount(tester, active: _hosts[first].id);
      final oldState = tester.state(_pane(first));
      final nextState = tester.state(_pane(1 - first));
      // Establish the same starting owner on both baseline and fixed code:
      // the old hidden pane can steal focus during its initial mount. The
      // regression below must fail after the titlebar click, not before it.
      await tester.tap(
        find.descendant(
          of: _pane(first),
          matching: find.byKey(const ValueKey('terminal-canvas')),
        ),
      );
      await tester.pump();
      await tester.pump();
      expect(_terminalFocus(tester, first).hasPrimaryFocus, isTrue);
      await _typeX(tester);
      expect(workspace.sessions[first].sent, [120]);
      for (final session in workspace.sessions) {
        session.sent.clear();
      }

      await _switchTo(tester, 1 - first);
      expect(tester.state(_pane(first)), same(oldState));
      expect(tester.state(_pane(1 - first)), same(nextState));
      await _typeX(tester, repeat: true);
      await tester.sendKeyDownEvent(LogicalKeyboardKey.enter);
      await tester.sendKeyUpEvent(LogicalKeyboardKey.enter);
      expect(workspace.sessions[first].sent, isEmpty);
      expect(workspace.sessions[1 - first].sent, [120, 120, 13]);
      await _unmount(tester);
      expect(workspace.sessions.map((session) => session.closes), [1, 1]);
    });
  }

  testWidgets('covered workspace rejects even residual terminal focus', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    final staleFocus = _terminalFocus(tester, 0);
    await tester.tap(find.byKey(const ValueKey('sftp-tab')));
    await tester.pump();
    // Deliberately restore stale focus: visibility must be a hard guard,
    // independent of whether Flutter has already applied the unfocus request.
    staleFocus.requestFocus();
    await tester.pump();
    expect(staleFocus.hasPrimaryFocus, isTrue);
    await _typeX(tester, repeat: true);
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);
    await _switchTo(tester, 1);
    await _typeX(tester);
    expect(workspace.sessions[1].sent, [120]);
    await _unmount(tester);
  });

  testWidgets('initially inactive panes neither autofocus nor accept input', (
    tester,
  ) async {
    final workspace = await _mount(tester, active: null);
    expect(_terminalFocus(tester, 0).hasPrimaryFocus, isFalse);
    expect(_terminalFocus(tester, 1).hasPrimaryFocus, isFalse);
    _terminalFocus(tester, 1).requestFocus();
    await tester.pump();
    await _typeX(tester, repeat: true);
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
  });

  testWidgets('inactive lifecycle blocks hardware input until resumed', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    addTearDown(() => _lifecycle(tester, AppLifecycleState.resumed));
    await _lifecycle(tester, AppLifecycleState.inactive);
    _terminalFocus(tester, 0).requestFocus();
    await tester.pump();
    await _typeX(tester, repeat: true);
    expect(workspace.sessions[0].sent, isEmpty);
    await _lifecycle(tester, AppLifecycleState.resumed);
    await _typeX(tester);
    expect(workspace.sessions[0].sent, [120]);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
  });

  testWidgets('mounting while inactive restores input on first resume', (
    tester,
  ) async {
    addTearDown(() => _lifecycle(tester, AppLifecycleState.resumed));
    await _lifecycle(tester, AppLifecycleState.inactive);
    final workspace = await _mount(tester);
    expect(_terminalFocus(tester, 0).hasPrimaryFocus, isFalse);
    await _typeX(tester);
    expect(workspace.sessions[0].sent, isEmpty);
    await _lifecycle(tester, AppLifecycleState.resumed);
    await tester.pump();
    await _typeX(tester);
    expect(workspace.sessions[0].sent, [120]);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
  });

  testWidgets('resuming leaves an existing search field focused', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    addTearDown(() => _lifecycle(tester, AppLifecycleState.resumed));
    await tester.tap(find.byKey(const ValueKey('terminal-search')));
    await tester.pump();
    final searchFocus = _panel(tester, 0).searchFocus!;
    expect(searchFocus.hasFocus, isTrue);
    await _lifecycle(tester, AppLifecycleState.inactive);
    await _lifecycle(tester, AppLifecycleState.resumed);
    await tester.pump();
    expect(searchFocus.hasFocus, isTrue);
    await _typeX(tester);
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
  });

  testWidgets('search typing never reaches either PTY', (tester) async {
    final workspace = await _mount(tester);
    await tester.tap(find.byKey(const ValueKey('terminal-search')));
    await tester.pump();
    expect(_panel(tester, 0).searchFocus!.hasFocus, isTrue);
    await _typeX(tester, repeat: true);
    await tester.enterText(
      find.descendant(
        of: find.byKey(const ValueKey('terminal-search')),
        matching: find.byType(EditableText),
      ),
      'needle',
    );
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
  });

  testWidgets('explicit background commands preserve visible tab ownership', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    await _switchTo(tester, 1);
    workspace.bus.send(_hosts[0].id, 'echo synthetic');
    await tester.pump();
    await tester.pump();
    expect(workspace.sessions[0].sent, utf8.encode('echo synthetic\r'));
    expect(workspace.sessions[1].sent, isEmpty);
    await _typeX(tester);
    expect(workspace.sessions[0].sent, utf8.encode('echo synthetic\r'));
    expect(workspace.sessions[1].sent, [120]);
    await _unmount(tester);
  });

  testWidgets('same-frame field request beats pending terminal restoration', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    _panel(tester, 0).onClear();
    // Do not pump between these calls: the field is not primary yet when
    // terminal restoration is queued.
    workspace.fieldFocus.requestFocus();
    await tester.pump();
    await tester.pump();
    expect(workspace.fieldFocus.hasPrimaryFocus, isTrue);
    await _typeX(tester);
    expect(workspace.sessions[0].sent, [12]);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
  });

  for (final switchTab in [true, false]) {
    testWidgets('delayed copy does not steal focus after '
        '${switchTab ? 'switching tabs' : 'clicking a field'}', (tester) async {
      final workspace = await _mount(tester);
      final clipboard = Completer<void>();
      var copyStarted = false;
      tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
        SystemChannels.platform,
        (call) async {
          if (call.method == 'Clipboard.setData') {
            copyStarted = true;
            await clipboard.future;
          }
          return null;
        },
      );
      addTearDown(() {
        if (!clipboard.isCompleted) clipboard.complete();
        tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
          SystemChannels.platform,
          null,
        );
      });
      _panel(tester, 0).onCopy();
      await tester.pump();
      expect(copyStarted, isTrue);
      if (switchTab) {
        await _switchTo(tester, 1);
      } else {
        await tester.tap(find.byKey(const ValueKey('unrelated-field')));
        await tester.pump();
      }
      clipboard.complete();
      await tester.pump();
      await tester.pump();
      expect(
        switchTab
            ? _terminalFocus(tester, 1).hasPrimaryFocus
            : workspace.fieldFocus.hasPrimaryFocus,
        isTrue,
      );
      await _typeX(tester);
      expect(workspace.sessions[0].sent, isEmpty);
      expect(workspace.sessions[1].sent, switchTab ? [120] : isEmpty);
      await _unmount(tester);
    });
  }

  for (final routeRequestsFocus in [true, false]) {
    testWidgets('modal route blocks queued terminal restoration '
        '(requestFocus=$routeRequestsFocus)', (tester) async {
      final workspace = await _mount(tester);
      final modalFocus = FocusNode(debugLabel: 'modal-input');
      final navigator = widgets.Navigator.of(workspace.context);
      final route = widgets.RawDialogRoute<void>(
        requestFocus: routeRequestsFocus,
        transitionDuration: Duration.zero,
        pageBuilder: (context, animation, secondaryAnimation) => Center(
          child: SizedBox(
            width: 240,
            height: 80,
            child: TextField(
              focusNode: modalFocus,
              autofocus: routeRequestsFocus,
            ),
          ),
        ),
      );
      try {
        _panel(tester, 0).onClear();
        // Keep active=true: this tests a real covering route, not the separate
        // workspace/SFTP visibility guard. Release residual focus before the
        // queued restoration; the no-focus route must not revive this pane.
        _terminalFocus(tester, 0).unfocus();
        unawaited(navigator.push<void>(route));
        await tester.pump();
        await tester.pump();
        expect(route.isCurrent, isTrue);
        expect(workspace.covered, isFalse);
        expect(_terminalFocus(tester, 0).hasPrimaryFocus, isFalse);
        if (routeRequestsFocus) {
          expect(modalFocus.hasPrimaryFocus, isTrue);
        }
        await _typeX(tester, repeat: true);
        expect(workspace.sessions[0].sent, [12]);
        expect(workspace.sessions[1].sent, isEmpty);

        navigator.pop();
        await tester.pump();
        await tester.pump();
        // An explicit terminal action after dismissal may restore input.
        // Do not assert automatic restoration: that is navigator policy.
        _panel(tester, 0).onClear();
        await tester.pump();
        await tester.pump();
        expect(_terminalFocus(tester, 0).hasPrimaryFocus, isTrue);
        await _typeX(tester);
        expect(workspace.sessions[0].sent, [12, 12, 120]);
        expect(workspace.sessions[1].sent, isEmpty);
      } finally {
        await _unmount(tester);
        modalFocus.dispose();
      }
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('inline credential field keeps its identity across A B A', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    final fieldFocus = await _focusCredentialPrompt(tester, workspace);
    await _switchTo(tester, 1);
    expect(fieldFocus.hasPrimaryFocus, isFalse);
    expect(_terminalFocus(tester, 1).hasPrimaryFocus, isTrue);

    await _switchTo(tester, 0);
    expect(tester.widget<EditableText>(_promptField(0)).focusNode, same(fieldFocus));
    expect(FocusManager.instance.primaryFocus, same(fieldFocus));
    await _typeX(tester, repeat: true);
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
    expect(tester.takeException(), isNull);
  });

  testWidgets('pending prompt blocks residual focus and ordinary send paths', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    await _focusCredentialPrompt(tester, workspace);
    final rootFocus = _terminalFocus(tester, 0);
    rootFocus.requestFocus();
    await tester.pump();
    expect(rootFocus.hasPrimaryFocus, isTrue);
    await _typeX(tester, repeat: true);
    workspace.bus.send(_hosts[0].id, 'echo synthetic');
    _panel(tester, 0).onClear();
    _panel(tester, 0).onRunSnippet!(
      const QuickCommand(index: 0, name: 'Synthetic', command: 'echo fixture'),
    );
    await tester.pump();
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);

    // The dedicated authentication path remains available; no actual
    // credentials or text-input injection are needed for this regression.
    await tester.tap(find.text('Continue'));
    await tester.pump();
    expect(workspace.sessions[0].answeredPromptIds, [41]);
    expect(workspace.sessions[1].answeredPromptIds, isEmpty);
    expect(workspace.sessions[0].sent, isEmpty);
    await _unmount(tester);
  });

  testWidgets('new hidden prompt blocks commands without disabling background input', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    await _switchTo(tester, 1);
    workspace.sessions[0].setPrompt(_credentialPrompt);
    // No pump or activation: A still has its cached no-prompt snapshot.
    workspace.bus.send(_hosts[0].id, 'echo blocked');
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);
    expect(_terminalFocus(tester, 1).hasPrimaryFocus, isTrue);

    workspace.sessions[0].setPrompt(null);
    // Background commands still work once the same one-off read confirms
    // there is no prompt. No background polling or tab activation is needed.
    workspace.bus.send(_hosts[0].id, 'echo synthetic');
    expect(workspace.sessions[0].sent, utf8.encode('echo synthetic\r'));
    await _typeX(tester, repeat: true);
    expect(workspace.sessions[0].sent, utf8.encode('echo synthetic\r'));
    expect(workspace.sessions[1].sent, [120, 120]);
    await _unmount(tester);
  });

  for (final action in ['clear', 'snippet', 'hardware']) {
    testWidgets('new unpolled prompt blocks ordinary $action input', (tester) async {
      final workspace = await _mount(tester);
      expect(_terminalFocus(tester, 0).hasPrimaryFocus, isTrue);
      workspace.sessions[0].setPrompt(_credentialPrompt);
      expect(find.byType(ConnectionPromptBanner), findsNothing);
      // Invoke the real input path before pumping the prompt onto the screen.
      if (action == 'clear') {
        _panel(tester, 0).onClear();
      } else if (action == 'snippet') {
        _panel(tester, 0).onRunSnippet!(
          const QuickCommand(index: 0, name: 'Synthetic', command: 'echo fixture'),
        );
      } else {
        await _typeX(tester, repeat: true);
      }
      expect(workspace.sessions[0].sent, isEmpty);
      expect(workspace.sessions[1].sent, isEmpty);
      await tester.pump();
      await tester.pump();
      expect(_promptField(0), findsOneWidget);
      await _typeX(tester, repeat: true);
      expect(workspace.sessions[0].sent, isEmpty);
      expect(workspace.sessions[1].sent, isEmpty);
      await _unmount(tester);
    });
  }

  testWidgets('prompt replaced while hidden never restores the old field', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    final oldFocus = await _focusCredentialPrompt(tester, workspace);
    await _switchTo(tester, 1);
    workspace.sessions[0].setPrompt({..._credentialPrompt, 'id': 42});
    // Do not poll the hidden pane first: activation must observe the newer
    // snapshot before it considers the old prompt's saved focus node.
    await _switchTo(tester, 0);
    final newFocus = tester.widget<EditableText>(_promptField(0)).focusNode;
    expect(newFocus, isNot(same(oldFocus)));
    expect(newFocus.hasPrimaryFocus, isFalse);
    expect(_terminalFocus(tester, 0).hasPrimaryFocus, isFalse);
    await _typeX(tester, repeat: true);
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);

    // Once selected by the user, the replacement is the only restorable field.
    await tester.tap(_promptField(0));
    await tester.pump();
    expect(newFocus.hasPrimaryFocus, isTrue);
    await _switchTo(tester, 1);
    await _switchTo(tester, 0);
    expect(FocusManager.instance.primaryFocus, same(newFocus));
    await _unmount(tester);
    expect(tester.takeException(), isNull);
  });

  testWidgets('unfocusable saved prompt field does not fall back to PTY', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    final fieldFocus = await _focusCredentialPrompt(tester, workspace);
    await _switchTo(tester, 1);
    fieldFocus.canRequestFocus = false;
    await _switchTo(tester, 0);
    expect(fieldFocus.hasPrimaryFocus, isFalse);
    expect(_terminalFocus(tester, 0).hasPrimaryFocus, isFalse);
    await _typeX(tester, repeat: true);
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
    expect(tester.takeException(), isNull);
  });

  testWidgets('competing field wins over queued inline prompt restoration', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    final fieldFocus = await _focusCredentialPrompt(tester, workspace);
    await _switchTo(tester, 1);
    await tester.tap(find.text('Input A'));
    // Register before activation builds and queues its own post-frame request.
    // The request is pending, not primary, when restoration begins.
    WidgetsBinding.instance.addPostFrameCallback((_) {
      workspace.fieldFocus.requestFocus();
    });
    await tester.pump();
    await tester.pump();
    expect(workspace.fieldFocus.hasPrimaryFocus, isTrue);
    expect(fieldFocus.hasPrimaryFocus, isFalse);
    expect(_terminalFocus(tester, 0).hasPrimaryFocus, isFalse);
    await _typeX(tester, repeat: true);
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
  });

  testWidgets('new hidden prompt without a saved field never focuses PTY', (
    tester,
  ) async {
    final workspace = await _mount(tester, active: _hosts[1].id);
    workspace.sessions[0].setPrompt(_credentialPrompt);
    await _switchTo(tester, 0);
    expect(_promptField(0), findsOneWidget);
    expect(_terminalFocus(tester, 0).hasPrimaryFocus, isFalse);
    await _typeX(tester, repeat: true);
    expect(workspace.sessions[0].sent, isEmpty);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
  });

  testWidgets('resolved hidden prompt releases its field for terminal input', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    await _focusCredentialPrompt(tester, workspace);
    await _switchTo(tester, 1);
    workspace.sessions[0].setPrompt(null);
    await _switchTo(tester, 0);
    expect(_promptField(0), findsNothing);
    expect(_terminalFocus(tester, 0).hasPrimaryFocus, isTrue);
    await _typeX(tester, repeat: true);
    expect(workspace.sessions[0].sent, [120, 120]);
    expect(workspace.sessions[1].sent, isEmpty);
    await _unmount(tester);
    expect(tester.takeException(), isNull);
  });

  for (final failure in ['version', 'snapshot', 'empty']) {
    testWidgets('activation read failure ($failure) blocks input until refreshed', (
      tester,
    ) async {
      final workspace = await _mount(tester);
      await _switchTo(tester, 1);
      workspace.sessions[0].setPrompt(_credentialPrompt);
      workspace.sessions[0].readFailure = failure;
      await _switchTo(tester, 0);
      expect(_terminalFocus(tester, 0).hasPrimaryFocus, isFalse);
      // The cached snapshot has no prompt. Even forced residual root focus
      // cannot send before a successful read establishes the current state.
      _terminalFocus(tester, 0).requestFocus();
      await tester.pump();
      expect(_terminalFocus(tester, 0).hasPrimaryFocus, isTrue);
      await _typeX(tester, repeat: true);
      workspace.bus.send(_hosts[0].id, 'echo synthetic');
      await tester.pump();
      expect(workspace.sessions[0].sent, isEmpty);
      expect(workspace.sessions[1].sent, isEmpty);
      expect(tester.takeException(), isNull);

      // Do not bump the version again: a failed/null snapshot must be retried
      // by the existing timer rather than treated as an already-read version.
      workspace.sessions[0].readFailure = null;
      await tester.pump(const Duration(milliseconds: 250));
      await tester.pump();
      expect(_promptField(0), findsOneWidget);
      await tester.tap(_promptField(0));
      await tester.pump();
      expect(
        tester.widget<EditableText>(_promptField(0)).focusNode.hasPrimaryFocus,
        isTrue,
      );
      await _typeX(tester, repeat: true);
      expect(workspace.sessions[0].sent, isEmpty);
      expect(workspace.sessions[1].sent, isEmpty);
      await _unmount(tester);
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('rapid A B A routes input and disposal cancels pending focus', (
    tester,
  ) async {
    final workspace = await _mount(tester);
    await tester.tap(find.text('Input B'));
    await tester.pump();
    await tester.tap(find.text('Input A'));
    await tester.pump();
    await tester.pump();
    await _typeX(tester);
    expect(workspace.sessions[0].sent, [120]);
    expect(workspace.sessions[1].sent, isEmpty);
    _panel(tester, 0).onClear();
    await _unmount(tester);
    expect(tester.takeException(), isNull);
    expect(workspace.sessions.map((session) => session.closes), [1, 1]);
  });
}
