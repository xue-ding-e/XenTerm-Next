import 'dart:io';

import 'package:flutter/services.dart';
import 'package:shadcn_flutter/shadcn_flutter.dart';
import 'package:flutter/scheduler.dart';
import 'package:shared_preferences/shared_preferences.dart';
import 'package:window_manager/window_manager.dart';

import 'data/hosts.dart';
import 'l10n/app_language.dart';
import 'shell/root_shell.dart';
import 'shell/window_activity.dart';
import 'terminal/core_settings.dart';
import 'theme/app_fonts.dart';
import 'theme/app_theme.dart';

/// Opt-in frame-timing probe for smoothness work. Build with
/// --dart-define=XENTERM_FRAME_STATS=true --profile; every 60 frames a
/// FRAMESTATS ... line goes to stdout and frames over 32ms are logged
/// individually, so a scripted UI run can be diffed before/after a change.
const bool _frameStatsEnabled = bool.fromEnvironment('XENTERM_FRAME_STATS');

void _installFrameStats() {
  final started = DateTime.now();
  final pending = <FrameTiming>[];
  String pct(List<double> sorted, double q) {
    if (sorted.isEmpty) return '0.0';
    final index = ((sorted.length - 1) * q).round();
    return sorted[index].toStringAsFixed(1);
  }

  void flush() {
    if (pending.isEmpty) return;
    final builds = [
      for (final frame in pending) frame.buildDuration.inMicroseconds / 1000.0,
    ]..sort();
    final rasters = [
      for (final frame in pending) frame.rasterDuration.inMicroseconds / 1000.0,
    ]..sort();
    final totals = <double>[
      for (var i = 0; i < pending.length; i++) builds[i] + rasters[i],
    ];
    final dropped = totals.where((total) => total > 16.7).length;
    final slow = totals.where((total) => total > 32).length;
    // ignore: avoid_print
    print(
      'FRAMESTATS t=${DateTime.now().difference(started).inMilliseconds} '
      'n=${pending.length} build_p50=${pct(builds, 0.5)} '
      'build_p90=${pct(builds, 0.9)} build_max=${builds.last.toStringAsFixed(1)} '
      'raster_p50=${pct(rasters, 0.5)} raster_p90=${pct(rasters, 0.9)} '
      'raster_max=${rasters.last.toStringAsFixed(1)} '
      'over16=$dropped over32=$slow',
    );
    pending.clear();
  }

  SchedulerBinding.instance.addTimingsCallback((batch) {
    for (final frame in batch) {
      final total = frame.buildDuration + frame.rasterDuration;
      if (total > const Duration(milliseconds: 32)) {
        // ignore: avoid_print
        print(
          'FRAMESTATS_SLOW t=${DateTime.now().difference(started).inMilliseconds} '
          'build=${(frame.buildDuration.inMicroseconds / 1000.0).toStringAsFixed(1)} '
          'raster=${(frame.rasterDuration.inMicroseconds / 1000.0).toStringAsFixed(1)}',
        );
      }
    }
    pending.addAll(batch);
    if (pending.length >= 60) flush();
  });
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  if (_frameStatsEnabled) _installFrameStats();
  final fonts = AppFonts();
  await fonts.load();
  await windowManager.ensureInitialized();

  // Pick the opaque window colour before the first frame: a translucent window
  // is what makes live resize / zoom / minimize cost extra compositor work on
  // macOS and Windows.
  final prefs = await SharedPreferences.getInstance();
  final restoreLight = prefs.getBool('xenterm.theme.light') ?? false;
  final windowBackground =
      restoreLight ? XtPalette.paperBase : XtPalette.black900;

  // The shell draws its own titlebar (session tabs live in it), so the native
  // frame is hidden; dragging is re-provided by the shell's drag region and
  // the platform's resize borders stay active.
  final options = WindowOptions(
    title: 'XenTerm',
    titleBarStyle: TitleBarStyle.hidden,
    minimumSize: const Size(940, 600),
    backgroundColor: windowBackground,
  );
  await windowManager.waitUntilReadyToShow(options, () async {
    await windowManager.show();
    await windowManager.focus();
  });

  // Some platforms (macOS with a hidden titlebar) force the window translucent
  // when the titlebar style is applied; restore opacity after that.
  await configureWindowSurface(windowBackground);
  WindowActivityObserver.install(windowActivity);

  runApp(XenTermApp(fonts: fonts));
}

/// Applies the opaque window colour natively where window_manager cannot:
/// macOS marks windows with a hidden titlebar as non-opaque, and that flag is
/// what makes the compositor blend every frame.
Future<void> configureWindowSurface(Color color) async {
  try {
    await windowManager.setBackgroundColor(color);
  } catch (_) {
    // Not supported on this host; the Flutter surface still covers the window.
  }
  if (!Platform.isMacOS) return;
  try {
    await const MethodChannel('xenterm/window').invokeMethod<void>(
      'setOpaque',
      <String, int>{
        'r': (color.r * 255.0).round(),
        'g': (color.g * 255.0).round(),
        'b': (color.b * 255.0).round(),
      },
    );
  } catch (_) {
    // Older builds without the channel keep the platform default.
  }
}

class XenTermApp extends StatefulWidget {
  final AppFonts? fonts;
  /// Overrides the hosts loader (widget tests use the sample fixture; the app
  /// loads the real sessions from the Rust core).
  final Future<List<Host>> Function()? hostsLoader;
  final CoreSettingsController? settings;
  const XenTermApp({
    super.key,
    this.fonts,
    this.hostsLoader,
    this.settings,
  });

  @override
  State<XenTermApp> createState() => _XenTermAppState();
}

class _XenTermAppState extends State<XenTermApp> {
  static const _themeKey = 'xenterm.theme.light';
  static const _languageKey = 'xenterm.language';
  bool _lightTheme = false;
  String _language = 'en';
  late final AppFonts _fonts = widget.fonts ?? AppFonts();
  late final CoreSettingsController _settings =
      widget.settings ?? CoreSettingsController();

  @override
  void initState() {
    super.initState();
    _fonts.addListener(_changed);
    if (widget.settings == null) _settings.load();
    _restoreTheme();
    _restoreLanguage();
  }

  Future<void> _restoreTheme() async {
    final prefs = await SharedPreferences.getInstance();
    final light = prefs.getBool(_themeKey);
    if (light != null && mounted) setState(() => _lightTheme = light);
  }

  Future<void> _restoreLanguage() async {
    final prefs = await SharedPreferences.getInstance();
    final code = prefs.getString(_languageKey);
    if (mounted && AppLanguage.supported.containsKey(code)) {
      setState(() => _language = code!);
    }
  }

  void _setLanguage(String code) {
    if (!AppLanguage.supported.containsKey(code)) return;
    setState(() => _language = code);
    SharedPreferences.getInstance().then(
      (prefs) => prefs.setString(_languageKey, code),
    );
  }

  void _setTheme(bool light) {
    setState(() => _lightTheme = light);
    SharedPreferences.getInstance().then(
      (prefs) => prefs.setBool(_themeKey, light),
    );
    configureWindowSurface(light ? XtPalette.paperBase : XtPalette.black900);
  }

  void _changed() => setState(() {});

  @override
  void dispose() {
    _fonts.removeListener(_changed);
    if (widget.fonts == null) _fonts.dispose();
    if (widget.settings == null) _settings.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return ShadcnApp(
      title: 'XenTerm',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        colorScheme: _lightTheme ? xtLightScheme() : xtDarkScheme(),
        typography: const Typography.geist().scale(_fonts.uiSize / 14),
        // The app's own radii (8/10/15) are applied at the call sites via
        // XtRadius; the multiplier only styles library internals.
        radius: 0.5,
      ),
      home: AppLanguage(
        code: _language,
        onChanged: _setLanguage,
        child: DefaultTextEditingShortcuts(
          child: DefaultTextStyle.merge(
            style: TextStyle(
              fontFamily: _fonts.uiFamily.isEmpty ? null : _fonts.uiFamily,
              fontSize: _fonts.uiSize,
            ),
            child: RootShell(
              lightTheme: _lightTheme,
              onThemeChanged: _setTheme,
              fonts: _fonts,
              settings: _settings,
              hostsLoader: widget.hostsLoader,
            ),
          ),
        ),
      ),
    );
  }
}
