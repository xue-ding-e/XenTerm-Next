import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:window_manager/window_manager.dart';

/// Whether the native window is currently in a compositor-driven transition
/// (live resize, zoom/maximize, minimize/restore).
///
/// While the OS animates the window frame, Flutter has to relayout and repaint
/// the whole surface on every frame. Any background work we add on top of that
/// (terminal polling, SFTP listings, streamed snapshots) competes for the same
/// frame budget and shows up as dropped frames — most visibly on macOS, where
/// the zoom/minimize animations are long and smooth enough for jank to be
/// obvious.
///
/// Pages that poll should treat this as "do not start work now" and skip a
/// frame or two; everything resumes as soon as the window settles.
class WindowActivity {
  /// False while the window is being resized/zoomed or is minimized.
  final ValueNotifier<bool> idle = ValueNotifier<bool>(true);

  /// How long after the last resize event the window is considered settled.
  /// macOS zooms in ~200ms and live resize emits an event per frame, so this
  /// covers the tail of the animation without delaying normal work for long.
  static const settleDelay = Duration(milliseconds: 220);

  Timer? _settleTimer;
  bool _minimized = false;

  bool get isIdle => idle.value;

  /// A resize/zoom event arrived; pause polling until the frame trail ends.
  void resize() {
    if (_minimized) return;
    idle.value = false;
    _settleTimer?.cancel();
    _settleTimer = Timer(settleDelay, () {
      if (!_minimized) idle.value = true;
    });
  }

  /// The window was minimized: stay paused until [restore].
  void minimize() {
    _minimized = true;
    _settleTimer?.cancel();
    idle.value = false;
  }

  /// The window came back from the Dock; resume after one settle delay so the
  /// restore animation gets the same treatment as a resize.
  void restore() {
    _minimized = false;
    resize();
  }

  /// Re-enable polling (used on focus and by tests).
  void wake() {
    _minimized = false;
    _settleTimer?.cancel();
    idle.value = true;
  }

  void dispose() {
    _settleTimer?.cancel();
    idle.dispose();
  }
}

/// Process-wide window activity signal. Installed from `main()`; pages listen
/// to [WindowActivity.idle] and skip polling while it is false.
final WindowActivity windowActivity = WindowActivity();

/// Bridges [WindowActivity] to window_manager's global events.
///
/// Installed once from `main()`; widget tests never mount a real window, so the
/// platform channel is unavailable there and the observer is simply absent.
class WindowActivityObserver with WindowListener {
  final WindowActivity activity;

  WindowActivityObserver(this.activity);

  static WindowActivityObserver? install(WindowActivity activity) {
    try {
      final observer = WindowActivityObserver(activity);
      windowManager.addListener(observer);
      return observer;
    } catch (_) {
      // No plugin (widget tests, unsupported host): polling simply never
      // pauses, which is the safe default.
      return null;
    }
  }

  @override
  void onWindowFocus() => activity.wake();

  @override
  void onWindowResize() => activity.resize();

  @override
  void onWindowResized() => activity.resize();

  @override
  void onWindowMaximize() => activity.resize();

  @override
  void onWindowUnmaximize() => activity.resize();

  @override
  void onWindowMinimize() => activity.minimize();

  @override
  void onWindowRestore() => activity.restore();

  @override
  void onWindowEnterFullScreen() => activity.resize();

  @override
  void onWindowLeaveFullScreen() => activity.resize();

  void dispose() {
    try {
      windowManager.removeListener(this);
    } catch (_) {
      // Already detached or no plugin; nothing to clean up.
    }
  }
}
