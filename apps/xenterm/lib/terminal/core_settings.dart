/// Shared, observable core settings for the Flutter shell.
///
/// `CoreSettings` is the Rust core's persisted configuration; the terminal
/// renderer and Settings screen both read this controller, so a change made in
/// Settings immediately re-renders open terminals and is written to
/// `sessions.db`.
library;

import 'package:flutter/foundation.dart';

import '../bridge/native_bridge.dart';

class CoreSettingsController extends ChangeNotifier {
  CoreSettings settings = const CoreSettings();
  bool loaded = false;
  String? error;

  Future<void> load() async {
    try {
      settings = await loadSettingsFromCore();
      loaded = true;
      error = null;
    } catch (failure) {
      // The bridge is absent in widget tests and on unsupported hosts; the
      // in-memory defaults still drive the UI.
      error = failure is NativeBridgeError ? failure.message : '$failure';
    }
    notifyListeners();
  }

  /// Applies [values] optimistically and persists them. Returns an error text
  /// when the core rejected the write; the previous values are restored then.
  Future<String?> update(Map<String, dynamic> values) async {
    final previous = settings;
    settings = _merge(previous, values);
    loaded = true;
    notifyListeners();
    try {
      final result = await saveSettingsToCore(values);
      if (!result.ok) {
        settings = previous;
        notifyListeners();
        return result.error ?? 'Could not save the setting';
      }
      return null;
    } catch (failure) {
      settings = previous;
      notifyListeners();
      return '$failure';
    }
  }

  static CoreSettings _merge(
    CoreSettings current,
    Map<String, dynamic> values,
  ) {
    return CoreSettings(
      sessionLogs: values['session_logs'] as bool? ?? current.sessionLogs,
      cursorStyle: values['cursor_style'] as String? ?? current.cursorStyle,
      lineSpacing:
          (values['line_spacing'] as num?)?.toDouble() ?? current.lineSpacing,
      padding: values['padding'] as bool? ?? current.padding,
      bold: values['bold'] as bool? ?? current.bold,
      sftpFollowCd:
          values['sftp_follow_cd'] as bool? ?? current.sftpFollowCd,
      downloadDir: values['download_dir'] as String? ?? current.downloadDir,
    );
  }
}
