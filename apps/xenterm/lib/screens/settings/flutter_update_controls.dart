import 'dart:async';
import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart' as material show SelectableText;
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../../bridge/native_bridge.dart' show coreVersionSync;
import '../../data/flutter_updates.dart';
import '../../l10n/app_language.dart';
import '../../widgets/primitives.dart';

enum _Status {
  idle,
  checking,
  current,
  unavailable,
  checkFailed,
  available,
  downloading,
  downloadFailed,
  downloaded,
  installerStarted,
}

class FlutterUpdateController extends ChangeNotifier {
  FlutterUpdateController(this._prefs, {FlutterUpdateSource? source})
    : _source = source ?? FlutterUpdateSource();

  static const _preference = 'xenterm.flutter.checkUpdatesOnStartup';
  final SharedPreferences _prefs;
  final FlutterUpdateSource _source;
  _Status _status = _Status.idle;
  FlutterUpdate? _offer;
  File? _saved;
  bool _disposed = false;

  bool get checkOnStartup => _prefs.getBool(_preference) ?? true;
  bool get busy =>
      _status == _Status.checking || _status == _Status.downloading;
  String? get availableVersion =>
      _status == _Status.available || _status == _Status.downloadFailed
      ? _offer?.version
      : null;

  void start() {
    if (checkOnStartup) unawaited(check());
  }

  Future<void> setCheckOnStartup(bool value) async {
    await _prefs.setBool(_preference, value);
    if (!_disposed) notifyListeners();
  }

  void _show(_Status status) {
    if (_disposed) return;
    _status = status;
    notifyListeners();
  }

  Future<void> check() async {
    if (busy) return;
    _offer = null;
    final version = coreVersionSync();
    final target = FlutterUpdateTarget.current();
    if (!FlutterUpdateSource.supports(version, target)) {
      _show(_Status.unavailable);
      return;
    }
    _show(_Status.checking);
    try {
      _offer = await _source.check(version!, target!);
      _show(_offer == null ? _Status.current : _Status.available);
    } catch (error) {
      debugPrint('Flutter update check failed: $error');
      _show(_Status.checkFailed);
    }
  }

  Future<void> download() async {
    final update = _offer;
    if (update == null || availableVersion == null) return;
    _show(_Status.downloading);
    try {
      final installer = update.target == FlutterUpdateTarget.windowsMsi;
      final folder = installer ? null : await getDirectoryPath();
      if (_disposed) return;
      if (!installer && folder == null) {
        _show(_Status.available);
        return;
      }
      final package = await update.download(
        folder == null ? null : Directory(folder),
      );
      if (_disposed) return;
      if (installer) {
        await update.install(package);
        _show(_Status.installerStarted);
      } else {
        _saved = package;
        _show(_Status.downloaded);
      }
    } catch (error) {
      debugPrint('Flutter update download failed: $error');
      _show(_Status.downloadFailed);
    }
  }

  @override
  void dispose() {
    _disposed = true;
    super.dispose();
  }
}

class FlutterUpdateControls extends StatelessWidget {
  const FlutterUpdateControls({super.key, required this.updates});

  final FlutterUpdateController updates;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: updates,
    builder: (context, _) {
      final message = switch (updates._status) {
        _Status.idle => null,
        _Status.checking => 'Checking for Flutter updates…',
        _Status.current => 'No newer Flutter build is available.',
        _Status.unavailable =>
          'Flutter Beta updates are unavailable for this build or platform.',
        _Status.checkFailed =>
          "Couldn't check for updates. Check access to GitHub and try again.",
        _Status.available => 'Flutter update available: {version}',
        _Status.downloading => 'Preparing and verifying the update…',
        _Status.downloadFailed =>
          "Couldn't prepare the update. Please try again.",
        _Status.downloaded => 'Verified and saved to {path}',
        _Status.installerStarted => 'The Flutter installer is open. Follow the Windows Installer prompts.',
      };
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          XtSwitchRow(
            label: 'Check for updates on startup',
            value: updates.checkOnStartup,
            onChanged: updates.setCheckOnStartup,
          ),
          const SizedBox(height: 12),
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              XtActionButton(
                key: const ValueKey('flutter-check-updates'),
                label: AppLanguage.tr(context, 'Check for updates'),
                variant: XtActionVariant.secondary,
                onPressed: updates.busy ? null : updates.check,
              ),
              if (updates.availableVersion != null)
                XtActionButton(
                  label: AppLanguage.tr(
                    context,
                    updates._offer?.target == FlutterUpdateTarget.windowsMsi
                        ? 'Download and install'
                        : 'Download update',
                  ),
                  variant: XtActionVariant.primary,
                  onPressed: updates.download,
                ),
            ],
          ),
          if (message != null) ...[
            const SizedBox(height: 10),
            material.SelectableText(
              AppLanguage.tr(context, message, {
                'version': updates._offer?.version ?? '',
                'path': updates._saved?.path ?? '',
              }),
            ),
          ],
        ],
      );
    },
  );
}
