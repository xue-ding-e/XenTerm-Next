/// The live terminal pane: a rio-vt screen streamed from the Rust core over
/// the FFI bridge, painted as styled runs, with real key input, resize, prompt
/// handling (host key / credentials / MFA) and a reconnect action.
///
/// The bridge exposes a cheap version counter, so the pane only pays for a
/// snapshot when the screen actually changed. The poll runs while the pane is
/// the visible foreground workspace of a resumed app; hidden tabs, the Home
/// screen, a minimised/unfocused window and the native window's own
/// resize/zoom/minimize animation ([windowActivity]) cancel the timer entirely.
/// When active, the pane starts at a fast cadence for interactive output and
/// backs off to a slow one while the screen is quiet, pulling immediately again
/// on key input.
///
/// Screen refreshes are published through value notifiers that only the canvas
/// subscribes to, so a tick repaints the terminal surface without rebuilding
/// the 300px tools panel (which gets a throttled derived snapshot), the search
/// field or the snippet list.
library;

import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';

import '../../bridge/native_bridge.dart';
import '../../data/host_os_registry.dart';
import '../../data/hosts.dart';
import '../../l10n/app_language.dart';
import '../../shell/window_activity.dart';
import '../../theme/app_fonts.dart';
import '../../terminal/core_settings.dart';
import '../../terminal/terminal_command_bus.dart';
import '../../theme/app_theme.dart';
import '../../widgets/primitives.dart';
import 'connection_prompt.dart';
import 'terminal_tools_panel.dart';

/// Concrete monospace families, in preference order. The generic `monospace`
/// family does not resolve on Windows (Flutter falls back to Segoe UI, a
/// proportional face), so the default is chosen per platform.
const _terminalFallbacks = <String>[
  'Cascadia Mono',
  'Consolas',
  'Courier New',
  'monospace',
];

/// Poll cadence while output is flowing (16 fps: smooth for a shell).
const _activePoll = Duration(milliseconds: 60);

/// Poll cadence after the screen has been quiet for [_quietTicksBeforeBackoff].
const _idlePoll = Duration(milliseconds: 240);

/// Consecutive unchanged polls before backing off to [_idlePoll].
const _quietTicksBeforeBackoff = 10;

/// The tools panel only redraws on this cadence (4 Hz) at most.
const _panelRefresh = Duration(milliseconds: 250);

String _terminalFontFamily(AppFonts fonts) {
  if (fonts.terminalFamily.isNotEmpty) return fonts.terminalFamily;
  if (Platform.isWindows) return 'Consolas';
  if (Platform.isMacOS) return 'Menlo';
  return 'DejaVu Sans Mono';
}

/// The live-session surface the pane drives.
///
/// Production uses [_NativeTerminalSession] on top of [NativeBridge]; widget
/// tests inject a fake through [TerminalView.sessionStarter] so polling,
/// lifecycle pauses and rendering can be exercised without the native core.
abstract interface class TerminalSession {
  /// Cheap change counter: the pane only pulls a snapshot when it advances.
  int get version;

  /// The decoded core snapshot for the current [version], or null.
  Map<String, dynamic>? snapshot();

  bool send(List<int> bytes);

  bool resize(int cols, int rows);

  bool answerPrompt(
    int promptId, {
    required bool accept,
    String? username,
    String? password,
    String? text,
  });

  void close();
}

class _NativeTerminalSession implements TerminalSession {
  _NativeTerminalSession(this._bridge, this._handle);

  final NativeBridge _bridge;
  final int _handle;

  @override
  int get version => _bridge.sessionVersion(_handle);

  @override
  Map<String, dynamic>? snapshot() {
    final raw = _bridge.sessionSnapshot(_handle);
    if (raw == null) return null;
    try {
      return jsonDecode(raw) as Map<String, dynamic>;
    } catch (_) {
      return null;
    }
  }

  @override
  bool send(List<int> bytes) =>
      _bridge.sendInput(_handle, Uint8List.fromList(bytes));

  @override
  bool resize(int cols, int rows) => _bridge.resizeSession(_handle, cols, rows);

  @override
  bool answerPrompt(
    int promptId, {
    required bool accept,
    String? username,
    String? password,
    String? text,
  }) => _bridge.answerPrompt(
    _handle,
    promptId,
    accept: accept,
    username: username,
    password: password,
    text: text,
  );

  @override
  void close() {
    _bridge.closeSession(_handle);
  }
}

/// The small derived slice of the screen snapshot the tools panel renders.
///
/// Comparing by value keeps [ValueNotifier] from notifying when a tick produced
/// no change the panel can show.
class TerminalPanelData {
  final int matchCount;
  final String? resourceState;
  final Map<String, dynamic>? resource;

  const TerminalPanelData({
    this.matchCount = 0,
    this.resourceState,
    this.resource,
  });

  @override
  bool operator ==(Object other) =>
      other is TerminalPanelData &&
      other.matchCount == matchCount &&
      other.resourceState == resourceState &&
      identical(other.resource, resource);

  @override
  int get hashCode =>
      Object.hash(matchCount, resourceState, identityHashCode(resource));
}

class TerminalView extends StatefulWidget {
  final Host host;
  final AppFonts fonts;
  final TerminalCommandBus? commandBus;
  final CoreSettingsController? settings;

  /// Whether this pane is the visible foreground workspace. Hidden session
  /// tabs and the offstage Home screen set this to false and stop polling.
  final bool active;

  /// Test seam that replaces the native bridge with a fake session.
  final TerminalSession Function()? sessionStarter;

  const TerminalView({
    super.key,
    required this.host,
    required this.fonts,
    this.commandBus,
    this.settings,
    this.active = true,
    this.sessionStarter,
  });

  @override
  State<TerminalView> createState() => _TerminalViewState();
}

class _TerminalViewState extends State<TerminalView>
    with WidgetsBindingObserver {
  TerminalSession? _session;
  Timer? _poll;
  bool _pollFast = false;

  /// Whether the app window is foreground/resumed (lifecycle observer).
  bool _foreground = true;

  Map<String, dynamic>? _screen;
  final ValueNotifier<Map<String, dynamic>?> _frame = ValueNotifier(null);
  final ValueNotifier<TerminalPanelData> _tools = ValueNotifier(
    const TerminalPanelData(),
  );
  String? _error;
  bool _closed = false;
  bool _starting = true;
  TerminalSessionStatus _status = TerminalSessionStatus.connecting;
  int _lastVersion = 0;
  int _quietTicks = 0;
  DateTime _lastPanelPublish = DateTime.fromMillisecondsSinceEpoch(0);
  String _search = '';
  List<QuickCommand> _snippets = const [];
  int _lastCommand = 0;

  // Cell width measurement is cached until the family/size changes: measuring
  // a laid-out TextPainter on every build was pure overhead.
  String? _cellFamily;
  double? _cellSize;
  double _cellWidth = 8;

  final FocusNode _focus = FocusNode(debugLabel: 'terminal');
  final FocusNode _searchFocus = FocusNode(debugLabel: 'terminal-search');

  @override
  void initState() {
    super.initState();
    HardwareKeyboard.instance.addHandler(_globalKey);
    widget.commandBus?.addListener(_onCommand);
    widget.settings?.addListener(_onSettings);
    WidgetsBinding.instance.addObserver(this);
    windowActivity.idle.addListener(_onWindowActivity);
    _foreground =
        (WidgetsBinding.instance.lifecycleState ?? AppLifecycleState.resumed) ==
        AppLifecycleState.resumed;
    _start();
    _loadSnippets();
  }

  Future<void> _loadSnippets() async {
    try {
      final snippets = await loadQuickCommandsFromCore();
      if (mounted) setState(() => _snippets = snippets);
    } catch (_) {
      // The sidebar simply stays empty; terminal input is unaffected.
    }
  }

  void _start() {
    try {
      final session = (widget.sessionStarter ?? _startNative)();
      setState(() {
        _session = session;
        _starting = false;
        _error = null;
        _closed = false;
        _status = TerminalSessionStatus.connecting;
        _lastVersion = 0;
      });
      _frame.value = null;
      _syncPolling();
      _focus.requestFocus();
    } catch (error) {
      setState(() {
        _error = error is NativeBridgeError ? error.message : '$error';
        _starting = false;
      });
    }
  }

  TerminalSession _startNative() {
    final bridge = NativeBridge.tryLoad();
    if (bridge == null) throw NativeBridgeError(NativeBridge.unavailableMessage());
    final handle = widget.host.ephemeral
        ? bridge.startAdhocSession({
            ...widget.host.toAdhocPayload(),
            'cols': 80,
            'rows': 24,
          })
        : bridge.startSession(widget.host.id, 80, 24);
    return _NativeTerminalSession(bridge, handle);
  }

  /// The pane polls only while it is visible, the app is resumed, the native
  /// window is not mid-resize/zoom/minimize, and the session is still open.
  bool get _shouldPoll =>
      widget.active &&
      _foreground &&
      windowActivity.isIdle &&
      _session != null &&
      !_closed &&
      _error == null;

  /// The native window started/finished a compositor-driven transition
  /// (live resize, zoom/maximize, minimize/restore). Re-evaluate polling: the
  /// animation frames get the whole budget while it lasts, and the first tick
  /// after it settles catches up immediately.
  void _onWindowActivity() {
    _syncPolling();
  }

  /// Starts/stops the poll timer to match [active] + app lifecycle.
  ///
  /// Becoming pollable pulls a snapshot immediately (a hidden tab may have
  /// missed seconds of output), then continues on the current cadence.
  void _syncPolling() {
    if (!_shouldPoll) {
      _poll?.cancel();
      _poll = null;
      _pollFast = false;
      return;
    }
    if (_poll != null) return;
    _quietTicks = 0;
    _pollFast = true;
    // Zero-delay instead of an inline tick: activation can happen mid-build
    // (didUpdateWidget), where publishing to the value notifiers would
    // re-enter the build phase.
    _poll = Timer(Duration.zero, _onPollTimer);
  }

  void _onPollTimer() {
    _poll = null;
    if (!mounted) return;
    _tick();
    if (!_shouldPoll) {
      _pollFast = false;
      return;
    }
    _scheduleNextPoll();
  }

  void _scheduleNextPoll() {
    _poll?.cancel();
    _pollFast = _quietTicks < _quietTicksBeforeBackoff;
    _poll = Timer(_pollFast ? _activePoll : _idlePoll, _onPollTimer);
  }

  /// Key input can produce output immediately; replace a pending idle wait with
  /// the fast cadence so echo latency stays low. A pending fast poll is left
  /// alone (resetting it on every keystroke would starve the timer).
  void _requestFastPoll() {
    if (!_shouldPoll || _poll == null) return;
    _quietTicks = 0;
    if (!_pollFast) _scheduleNextPoll();
  }

  void _tick() {
    final session = _session;
    if (session == null || _closed) return;
    final version = session.version;
    if (version == _lastVersion) {
      _quietTicks++;
      return;
    }
    _lastVersion = version;
    final parsed = session.snapshot();
    if (parsed == null || !mounted) return;
    _quietTicks = 0;
    _screen = parsed;
    _frame.value = parsed;
    hostOsRegistry.record(widget.host.id, parsed['remoteOs'] as String?);
    final closed = parsed['closed'] != null;
    final nextStatus = closed
        ? TerminalSessionStatus.disconnected
        : TerminalSessionStatus.connected;
    if (closed != _closed || nextStatus != _status) {
      setState(() {
        _closed = closed;
        _status = nextStatus;
      });
    }
    _publishPanel();
  }

  /// Publishes the throttled derived snapshot the tools panel shows (match
  /// count, resource sample). Full-screen searches are therefore capped at
  /// [_panelRefresh], never at the poll rate.
  void _publishPanel({bool force = false}) {
    final now = DateTime.now();
    if (!force && now.difference(_lastPanelPublish) < _panelRefresh) return;
    _lastPanelPublish = now;
    final query = _search.trim();
    _tools.value = TerminalPanelData(
      matchCount: query.isEmpty ? 0 : _matchCount(_screen, query),
      resourceState: _screen?['resourceState'] as String?,
      resource: _screen?['resource'] as Map<String, dynamic>?,
    );
  }

  void _onSettings() {
    if (mounted) setState(() {});
  }

  void _onSearch(String value) {
    setState(() => _search = value);
    _publishPanel(force: true);
  }

  /// Counts non-overlapping matches over the visible rows. Only called from
  /// [_publishPanel], i.e. at most [_panelRefresh] while a query is active.
  int _matchCount(Map<String, dynamic>? screen, String query) {
    final needle = query.toLowerCase();
    final rows = (screen?['rows'] as List?) ?? const [];
    var count = 0;
    for (final entry in rows) {
      final text = ((entry as Map<String, dynamic>)['text'] as String? ?? '')
          .toLowerCase();
      var index = 0;
      while ((index = text.indexOf(needle, index)) != -1) {
        count++;
        index += needle.length;
      }
    }
    return count;
  }

  /// Runs a snippet/command link in this PTY. The bus carries the target host
  /// id, so background tabs never run another tab's command.
  void _onCommand() {
    final bus = widget.commandBus;
    final command = bus?.last;
    if (bus == null || command == null) return;
    if (command.hostId != widget.host.id) return;
    if (bus.sequence == _lastCommand) return;
    _lastCommand = bus.sequence;
    _send([...utf8.encode(command.text), if (command.sendEnter) 13]);
    _focus.requestFocus();
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    final foreground = state == AppLifecycleState.resumed;
    if (foreground == _foreground) return;
    _foreground = foreground;
    _syncPolling();
  }

  @override
  void didUpdateWidget(TerminalView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.active != widget.active) _syncPolling();
    if (oldWidget.commandBus != widget.commandBus) {
      oldWidget.commandBus?.removeListener(_onCommand);
      widget.commandBus?.addListener(_onCommand);
    }
    if (oldWidget.settings != widget.settings) {
      oldWidget.settings?.removeListener(_onSettings);
      widget.settings?.addListener(_onSettings);
    }
  }

  @override
  void dispose() {
    widget.commandBus?.removeListener(_onCommand);
    widget.settings?.removeListener(_onSettings);
    HardwareKeyboard.instance.removeHandler(_globalKey);
    WidgetsBinding.instance.removeObserver(this);
    windowActivity.idle.removeListener(_onWindowActivity);
    _poll?.cancel();
    _session?.close();
    _frame.dispose();
    _tools.dispose();
    _focus.dispose();
    _searchFocus.dispose();
    super.dispose();
  }

  void _send(List<int> bytes) {
    final session = _session;
    if (session == null || bytes.isEmpty) return;
    session.send(bytes);
    _requestFastPoll();
  }

  void _resize(int cols, int rows) {
    if (cols == _cols && rows == _rows) return;
    _cols = cols;
    _rows = rows;
    _session?.resize(cols, rows);
  }

  int _cols = 0;
  int _rows = 0;

  void _answerPrompt(
    int promptId, {
    required bool accept,
    String? username,
    String? password,
    String? text,
  }) {
    _session?.answerPrompt(
      promptId,
      accept: accept,
      username: username,
      password: password,
      text: text,
    );
    _requestFastPoll();
  }

  void _reconnect() {
    _poll?.cancel();
    _poll = null;
    _pollFast = false;
    _session?.close();
    setState(() {
      _session = null;
      _screen = null;
      _closed = false;
      _starting = true;
      _status = TerminalSessionStatus.connecting;
      _lastVersion = 0;
      _quietTicks = 0;
    });
    _frame.value = null;
    _tools.value = const TerminalPanelData();
    _start();
  }

  /// Terminal input is captured by a global hardware-key handler gated on this
  /// pane's primary focus, so other shortcuts never fire mid-terminal and the
  /// encoding does not depend on the focus chain's event dispatch.
  bool _globalKey(KeyEvent event) {
    if (!_focus.hasPrimaryFocus) return false;
    if (event is! KeyDownEvent && event is! KeyRepeatEvent) return false;
    if (HardwareKeyboard.instance.isControlPressed &&
        HardwareKeyboard.instance.isShiftPressed &&
        event.logicalKey == LogicalKeyboardKey.keyF) {
      _searchFocus.requestFocus();
      return true;
    }
    final bytes = _encodeKey(event, _screen?['applicationCursor'] == true);
    if (bytes.isNotEmpty) _send(bytes);
    return true;
  }

  /// Measures the cell with the SAME style the painter uses. 'monospace' is
  /// not a real Windows family — Flutter silently falls back to Segoe UI,
  /// which is proportional and makes cells drift away from the glyphs.
  double _cellWidthFor(String fontFamily, double fontSize) {
    if (_cellFamily == fontFamily && _cellSize == fontSize) return _cellWidth;
    final measure = TextPainter(
      text: TextSpan(
        text: 'M' * 10,
        style: TextStyle(
          fontFamily: fontFamily,
          fontFamilyFallback: _terminalFallbacks,
          fontSize: fontSize,
          height: 1,
        ),
      ),
      textDirection: TextDirection.ltr,
    )..layout();
    _cellWidth = measure.width / 10;
    measure.dispose();
    _cellFamily = fontFamily;
    _cellSize = fontSize;
    return _cellWidth;
  }

  @override
  Widget build(BuildContext context) {
    final fontFamily = _terminalFontFamily(widget.fonts);
    final fontSize = widget.fonts.terminalSize;
    final cellWidth = _cellWidthFor(fontFamily, fontSize);
    final core = widget.settings?.settings ?? const CoreSettings();
    final lineHeight = fontSize * 1.25 * core.lineSpacing;
    final inset = core.padding ? 12.0 : 0.0;

    return Row(
      children: [
        Expanded(
          child: Container(
            key: const ValueKey('terminal-view'),
            color: XtColors.terminal(context),
            child: LayoutBuilder(
              builder: (context, constraints) {
                final cols = ((constraints.maxWidth - inset * 2) / cellWidth)
                    .floor()
                    .clamp(2, 1000);
                final rows = ((constraints.maxHeight - inset * 2) / lineHeight)
                    .floor()
                    .clamp(2, 1000);
                _resize(cols, rows);
                return Focus(
                  focusNode: _focus,
                  autofocus: true,
                  child: GestureDetector(
                    behavior: HitTestBehavior.opaque,
                    onTap: _focus.requestFocus,
                    child: _TerminalCanvas(
                      frame: _frame,
                      search: _search,
                      starting: _starting,
                      error: _error,
                      bold: core.bold,
                      cursorStyle: core.cursorStyle,
                      fontFamily: fontFamily,
                      fontSize: fontSize,
                      cellWidth: cellWidth,
                      lineHeight: lineHeight,
                      inset: inset,
                      onReconnect: _reconnect,
                      onAnswerPrompt: _answerPrompt,
                    ),
                  ),
                );
              },
            ),
          ),
        ),
        SizedBox(
          width: 300,
          // The panel updates on its own 4 Hz throttle; its own layer keeps
          // those rebuilds from repainting the terminal surface (and vice
          // versa).
          child: RepaintBoundary(
            child: ValueListenableBuilder<TerminalPanelData>(
              valueListenable: _tools,
              builder: (context, data, _) => TerminalToolsPanel(
                fonts: widget.fonts,
                status: _status,
                search: _search,
                searchFocus: _searchFocus,
                matchCount: data.matchCount,
                resourceState: data.resourceState,
                resource: data.resource,
                onSearch: _onSearch,
                onClear: () {
                  _send(const [0x0c]);
                  _focus.requestFocus();
                },
                onCopy: () async {
                  await copyScreenText(_screen);
                  _focus.requestFocus();
                },
                onReconnect: _reconnect,
                snippets: _snippets,
                onRunSnippet: (snippet) {
                  _send([
                    ...utf8.encode(snippet.command),
                    if (snippet.sendEnter) 13,
                  ]);
                  _focus.requestFocus();
                },
              ),
            ),
          ),
        ),
      ],
    );
  }
}

/// The terminal surface plus its connect/error/disconnect overlays and the
/// blocking connection prompt.
///
/// This widget alone subscribes to the 20 fps frame stream: a snapshot tick
/// rebuilds only this subtree, never the tools panel or the surrounding chrome.
class _TerminalCanvas extends StatefulWidget {
  final ValueListenable<Map<String, dynamic>?> frame;
  final String search;
  final bool starting;
  final String? error;
  final bool bold;
  final String cursorStyle;
  final String fontFamily;
  final double fontSize;
  final double cellWidth;
  final double lineHeight;
  final double inset;
  final VoidCallback onReconnect;
  final void Function(
    int promptId, {
    required bool accept,
    String? username,
    String? password,
    String? text,
  })
  onAnswerPrompt;

  const _TerminalCanvas({
    required this.frame,
    required this.search,
    required this.starting,
    required this.error,
    required this.bold,
    required this.cursorStyle,
    required this.fontFamily,
    required this.fontSize,
    required this.cellWidth,
    required this.lineHeight,
    required this.inset,
    required this.onReconnect,
    required this.onAnswerPrompt,
  });

  @override
  State<_TerminalCanvas> createState() => _TerminalCanvasState();
}

class _TerminalCanvasState extends State<_TerminalCanvas> {
  /// Glyph painters survive repaints; they are rebuilt only for new text/style.
  final TerminalTextCache _textCache = TerminalTextCache();

  @override
  void didUpdateWidget(_TerminalCanvas oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.fontFamily != widget.fontFamily ||
        oldWidget.fontSize != widget.fontSize ||
        oldWidget.bold != widget.bold) {
      // Old glyph sizes would never be reused; drop them instead of letting
      // the bounded cache carry dead entries.
      _textCache.clear();
    }
  }

  @override
  void dispose() {
    _textCache.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder<Map<String, dynamic>?>(
      valueListenable: widget.frame,
      builder: (context, screen, _) {
        final closed = screen?['closed'] != null;
        final prompt = screen?['prompt'] as Map<String, dynamic>?;
        return Stack(
          children: [
            Positioned.fill(
              child: Padding(
                padding: EdgeInsets.all(widget.inset),
                child: RepaintBoundary(
                  child: CustomPaint(
                    key: const ValueKey('terminal-canvas'),
                    painter: TerminalPainter(
                      screen: screen,
                      search: widget.search,
                      bold: widget.bold,
                      cursorStyle: widget.cursorStyle,
                      fontFamily: widget.fontFamily,
                      fontSize: widget.fontSize,
                      cellWidth: widget.cellWidth,
                      lineHeight: widget.lineHeight,
                      textCache: _textCache,
                    ),
                  ),
                ),
              ),
            ),
            if (widget.starting)
              Center(
                child: Text(
                  AppLanguage.tr(context, 'Connecting…'),
                  style: TextStyle(color: XtColors.textSecondary(context)),
                ),
              ),
            if (widget.error != null)
              Center(
                child: Padding(
                  padding: const EdgeInsets.all(24),
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Text(
                        AppLanguage.tr(context, 'Terminal unavailable'),
                        textAlign: TextAlign.center,
                        style: TextStyle(
                          color: XtColors.dangerText(context),
                          fontSize: 13,
                        ),
                      ),
                      const SizedBox(height: 6),
                      // Core/bridge error text stays verbatim (it names the
                      // technical cause); only the heading is translated.
                      Text(
                        _displayError(context, widget.error!),
                        textAlign: TextAlign.center,
                        style: TextStyle(
                          color: XtColors.dangerText(context),
                          fontSize: 13,
                        ),
                      ),
                      const SizedBox(height: 12),
                      XtActionButton(
                        label: AppLanguage.tr(context, 'Retry'),
                        variant: XtActionVariant.secondary,
                        onPressed: widget.onReconnect,
                      ),
                    ],
                  ),
                ),
              ),
            if (closed && widget.error == null)
              Center(
                child: _StatusCard(
                  title: AppLanguage.tr(context, 'Disconnected'),
                  detail: '${screen?['closed'] ?? ''}',
                  actionLabel: AppLanguage.tr(context, 'Reconnect'),
                  onAction: widget.onReconnect,
                ),
              ),
            if (prompt != null && !closed)
              Positioned(
                left: 12,
                right: 12,
                top: 12,
                child: ConnectionPromptBanner(
                  prompt: prompt,
                  onAnswer: (accept, {username, password, text}) {
                    widget.onAnswerPrompt(
                      promptIdOf(screen!),
                      accept: accept,
                      username: username,
                      password: password,
                      text: text,
                    );
                  },
                ),
              ),
          ],
        );
      },
    );
  }
}

String _displayError(BuildContext context, String error) =>
    error == NativeBridge.unavailableMessage() ? AppLanguage.tr(context, error) : error;

int promptIdOf(Map<String, dynamic> screen) {
  final prompt = screen['prompt'] as Map<String, dynamic>?;
  return (prompt?['id'] as num?)?.toInt() ?? 0;
}

/// Encodes one key event into the bytes a PTY expects (xterm conventions).
List<int> _encodeKey(KeyEvent event, bool applicationCursor) {
  final ctrl = HardwareKeyboard.instance.isControlPressed;
  final alt = HardwareKeyboard.instance.isAltPressed;
  final key = event.logicalKey;

  List<int>? bytes;
  if (key == LogicalKeyboardKey.enter ||
      key == LogicalKeyboardKey.numpadEnter) {
    bytes = [13];
  } else if (key == LogicalKeyboardKey.backspace) {
    bytes = [127];
  } else if (key == LogicalKeyboardKey.tab) {
    bytes = HardwareKeyboard.instance.isShiftPressed ? [27, 91, 90] : [9];
  } else if (key == LogicalKeyboardKey.escape) {
    bytes = [27];
  } else if (key == LogicalKeyboardKey.arrowUp) {
    bytes = applicationCursor ? [27, 79, 65] : [27, 91, 65];
  } else if (key == LogicalKeyboardKey.arrowDown) {
    bytes = applicationCursor ? [27, 79, 66] : [27, 91, 66];
  } else if (key == LogicalKeyboardKey.arrowRight) {
    bytes = applicationCursor ? [27, 79, 67] : [27, 91, 67];
  } else if (key == LogicalKeyboardKey.arrowLeft) {
    bytes = applicationCursor ? [27, 79, 68] : [27, 91, 68];
  } else if (key == LogicalKeyboardKey.home) {
    bytes = applicationCursor ? [27, 79, 72] : [27, 91, 72];
  } else if (key == LogicalKeyboardKey.end) {
    bytes = applicationCursor ? [27, 79, 70] : [27, 91, 70];
  } else if (key == LogicalKeyboardKey.insert) {
    bytes = [27, 91, 50, 126];
  } else if (key == LogicalKeyboardKey.delete) {
    bytes = [27, 91, 51, 126];
  } else if (key == LogicalKeyboardKey.pageUp) {
    bytes = [27, 91, 53, 126];
  } else if (key == LogicalKeyboardKey.pageDown) {
    bytes = [27, 91, 54, 126];
  } else if (key == LogicalKeyboardKey.f1) {
    bytes = [27, 79, 80];
  } else if (key == LogicalKeyboardKey.f2) {
    bytes = [27, 79, 81];
  } else if (key == LogicalKeyboardKey.f3) {
    bytes = [27, 79, 82];
  } else if (key == LogicalKeyboardKey.f4) {
    bytes = [27, 79, 83];
  } else if (key == LogicalKeyboardKey.f5) {
    bytes = [27, 91, 49, 53, 126];
  } else if (key == LogicalKeyboardKey.f6) {
    bytes = [27, 91, 49, 55, 126];
  } else if (key == LogicalKeyboardKey.f7) {
    bytes = [27, 91, 49, 56, 126];
  } else if (key == LogicalKeyboardKey.f8) {
    bytes = [27, 91, 49, 57, 126];
  } else if (key == LogicalKeyboardKey.f9) {
    bytes = [27, 91, 50, 48, 126];
  } else if (key == LogicalKeyboardKey.f10) {
    bytes = [27, 91, 50, 49, 126];
  } else if (key == LogicalKeyboardKey.f11) {
    bytes = [27, 91, 50, 51, 126];
  } else if (key == LogicalKeyboardKey.f12) {
    bytes = [27, 91, 50, 52, 126];
  } else {
    var character = event.character;
    if (character == null || character.isEmpty) {
      // Some input paths (and injected key messages) carry no character; fall
      // back to the key label so typing still reaches the PTY.
      final label = key.keyLabel;
      if (label.length == 1) {
        final letter = label.toLowerCase();
        character = letter != label && !HardwareKeyboard.instance.isShiftPressed
            ? letter
            : label;
      }
    }
    if (character != null && character.isNotEmpty) {
      if (ctrl) {
        final code = character.toLowerCase().codeUnitAt(0);
        if (code >= 97 && code <= 122) {
          bytes = [code - 96];
        } else if (character == '@') {
          bytes = [0];
        } else if (character == '[') {
          bytes = [27];
        } else if (character == r'\') {
          bytes = [28];
        } else if (character == ']') {
          bytes = [29];
        } else if (character == '^') {
          bytes = [30];
        } else if (character == '_') {
          bytes = [31];
        } else if (character == '?') {
          bytes = [127];
        }
      }
      bytes ??= utf8.encode(character);
    }
  }
  if (bytes != null && alt && bytes.length == 1 && bytes.first >= 32) {
    return [27, ...bytes];
  }
  return bytes ?? const [];
}

/// Bounded LRU cache of laid-out glyph [TextPainter]s.
///
/// Repaints (cursor blink aside, every frame while output streams) reuse the
/// laid-out painter of unchanged runs instead of re-running text layout for the
/// whole screen. The cache is bounded so a full-screen scroll cannot grow it
/// without limit; [dispose] releases every cached painter.
class TerminalTextCache {
  /// Hard cap on cached painters (a 2500x1300 screen has well under this many
  /// distinct runs; the cap mostly protects against scroll churn).
  static const int maxEntries = 512;

  final LinkedHashMap<(String, String, double, Color, bool), TextPainter>
  _entries =
      LinkedHashMap<(String, String, double, Color, bool), TextPainter>();

  /// Number of cached entries (test/diagnostic surface).
  int get length => _entries.length;

  TextPainter obtain({
    required String text,
    required String fontFamily,
    required double fontSize,
    required Color color,
    required bool bold,
  }) {
    final key = (text, fontFamily, fontSize, color, bold);
    final cached = _entries.remove(key);
    if (cached != null) {
      // Re-insert to mark it most-recently used.
      _entries[key] = cached;
      return cached;
    }
    final painter = TextPainter(
      text: TextSpan(
        text: text,
        style: TextStyle(
          fontFamily: fontFamily,
          fontFamilyFallback: _terminalFallbacks,
          fontSize: fontSize,
          height: 1,
          color: color,
          fontWeight: bold ? FontWeight.w700 : FontWeight.w400,
        ),
      ),
      textDirection: TextDirection.ltr,
    )..layout();
    _entries[key] = painter;
    if (_entries.length > maxEntries) {
      final oldest = _entries.keys.first;
      _entries.remove(oldest)?.dispose();
    }
    return painter;
  }

  void clear() {
    for (final painter in _entries.values) {
      painter.dispose();
    }
    _entries.clear();
  }

  void dispose() => clear();
}

class TerminalPainter extends CustomPainter {
  final Map<String, dynamic>? screen;
  final String search;
  final bool bold;
  final String cursorStyle;
  final String fontFamily;
  final double fontSize;
  final double cellWidth;
  final double lineHeight;
  final TerminalTextCache textCache;

  TerminalPainter({
    required this.screen,
    required this.search,
    required this.bold,
    required this.cursorStyle,
    required this.fontFamily,
    required this.fontSize,
    required this.cellWidth,
    required this.lineHeight,
    required this.textCache,
  });

  // The terminal surface and its default ink stay fixed in both themes: a
  // terminal is expected to be dark, and these are the fallback colours the
  // ANSI palette below is drawn against. They are deliberately not theme
  // tokens, unlike every surrounding piece of chrome.
  static const _defaultFg = Color(0xFFDDDDDD);
  static const _defaultBg = Color(0xFF0A0A0A);
  static const _cursorColor = Color(0xFFFFFFFF);
  static const _whiteWash = Color(0x40FFFFFF);

  // Reused across every run/rect in a paint; drawing commands copy the paint
  // values, so one instance is enough for the whole frame.
  final Paint _fillPaint = Paint()..color = _defaultBg;
  final Paint _runPaint = Paint();
  final Paint _highlightPaint = Paint()..color = _whiteWash;
  final Paint _cursorPaint = Paint()..color = _cursorColor;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.drawRect(Offset.zero & size, _fillPaint);
    final query = search.trim().toLowerCase();
    final rows = (screen?['rows'] as List?) ?? const [];
    for (var rowIndex = 0; rowIndex < rows.length; rowIndex++) {
      final row = rows[rowIndex] as Map<String, dynamic>;
      final runs = (row['runs'] as List?) ?? const [];
      if (query.isNotEmpty) {
        _highlightMatches(canvas, rowIndex, row, query);
      }
      var x = 0.0;
      for (final entry in runs) {
        final run = entry as Map<String, dynamic>;
        final cells = (run['c'] as num?)?.toInt() ?? 0;
        final style = (run['s'] as Map<String, dynamic>?) ?? const {};
        final width = cells * cellWidth;
        var fg = _color(style['fg'], _defaultFg) ?? _defaultFg;
        var bg = _color(style['bg'], null);
        if (style['i'] == true) {
          final previous = fg;
          fg = bg ?? _defaultBg;
          bg = previous;
        }
        if (bg != null && bg.a > 0) {
          _runPaint.color = bg;
          canvas.drawRect(
            Rect.fromLTWH(x, rowIndex * lineHeight, width, lineHeight),
            _runPaint,
          );
        }
        final text = (run['t'] as String?) ?? '';
        if (text.isNotEmpty) {
          final painter = textCache.obtain(
            text: text,
            fontFamily: fontFamily,
            fontSize: fontSize,
            color: fg,
            bold: style['b'] == true,
          );
          painter.paint(
            canvas,
            Offset(
              x,
              rowIndex * lineHeight + (lineHeight - painter.height) / 2,
            ),
          );
        }
        x += width;
      }
    }
    final cursor = screen?['cursor'] as List?;
    if (cursor != null && screen?['cursorHidden'] != true) {
      final row = (cursor[0] as num).toInt();
      final col = (cursor[1] as num).toInt();
      final x = col * cellWidth;
      final y = row * lineHeight;
      switch (cursorStyle) {
        case 'bar':
          canvas.drawRect(Rect.fromLTWH(x, y, 2, lineHeight), _cursorPaint);
        case 'underline':
          canvas.drawRect(
            Rect.fromLTWH(x, y + lineHeight - 2, cellWidth, 2),
            _cursorPaint,
          );
        default:
          canvas.drawRect(
            Rect.fromLTWH(x, y, cellWidth, lineHeight),
            _cursorPaint,
          );
      }
    }
  }

  void _highlightMatches(
    Canvas canvas,
    int rowIndex,
    Map<String, dynamic> row,
    String query,
  ) {
    final text = ((row['text'] as String?) ?? '').toLowerCase();
    // Search highlight is painted over terminal content, so it is a fixed
    // translucent wash rather than a theme surface.
    var index = 0;
    while ((index = text.indexOf(query, index)) != -1) {
      canvas.drawRect(
        Rect.fromLTWH(
          index * cellWidth,
          rowIndex * lineHeight,
          query.length * cellWidth,
          lineHeight,
        ),
        _highlightPaint,
      );
      index += query.length;
    }
  }

  Color? _color(dynamic value, Color? fallback) {
    if (value == null) return fallback;
    if (value is Map) {
      final rgb = value['rgb'];
      if (rgb is List && rgb.length >= 3) {
        return Color.fromARGB(
          255,
          (rgb[0] as num).toInt(),
          (rgb[1] as num).toInt(),
          (rgb[2] as num).toInt(),
        );
      }
      final index = value['idx'];
      if (index is num) return _ansiPalette[index.toInt() % 256];
    }
    return fallback;
  }

  @override
  bool shouldRepaint(TerminalPainter oldDelegate) =>
      !identical(oldDelegate.screen, screen) ||
      oldDelegate.search != search ||
      oldDelegate.bold != bold ||
      oldDelegate.cursorStyle != cursorStyle ||
      oldDelegate.fontFamily != fontFamily ||
      oldDelegate.fontSize != fontSize ||
      oldDelegate.cellWidth != cellWidth ||
      oldDelegate.lineHeight != lineHeight;

  static final List<Color> _ansiPalette = _buildPalette();

  static List<Color> _buildPalette() {
    const base = <int>[
      0xFF000000,
      0xFFCD3131,
      0xFF0DBC79,
      0xFFE5E510,
      0xFF2472C8,
      0xFFBC3FBC,
      0xFF11A8CD,
      0xFFE5E5E5,
      0xFF666666,
      0xFFF14C4C,
      0xFF23D18B,
      0xFFF5F543,
      0xFF3B8EEA,
      0xFFD670D6,
      0xFF29B8DB,
      0xFFE5E5E5,
    ];
    final palette = <Color>[for (final v in base) Color(v)];
    const steps = <int>[0, 95, 135, 175, 215, 255];
    for (final r in steps) {
      for (final g in steps) {
        for (final b in steps) {
          palette.add(Color.fromARGB(255, r, g, b));
        }
      }
    }
    for (var i = 0; i < 24; i++) {
      final level = 8 + i * 10;
      palette.add(Color.fromARGB(255, level, level, level));
    }
    return palette;
  }
}

/// The blocking connect-time prompt (host key / credentials / MFA) as a banner
/// over the terminal — the answer goes back through the bridge to the paused
/// auth flow.
class _StatusCard extends StatelessWidget {
  final String title;
  final String detail;
  final String actionLabel;
  final VoidCallback onAction;

  const _StatusCard({
    required this.title,
    required this.detail,
    required this.actionLabel,
    required this.onAction,
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return XtCard(
      padding: const EdgeInsets.all(16),
      radius: XtRadius.field,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            title,
            style: theme.typography.semiBold.copyWith(
              fontSize: 14,
              color: XtColors.textPrimary(context),
            ),
          ),
          if (detail.isNotEmpty) ...[
            const SizedBox(height: 4),
            Text(
              detail,
              textAlign: TextAlign.center,
              style: theme.typography.normal.copyWith(
                fontSize: 12,
                color: XtColors.textSecondary(context),
              ),
            ),
          ],
          const SizedBox(height: 10),
          XtActionButton(
            label: actionLabel,
            variant: XtActionVariant.primary,
            onPressed: onAction,
          ),
        ],
      ),
    );
  }
}
